//! `neuron prompt` runs nobody attends (stdin is not a terminal), against the mock
//! Anthropic service:
//! - a model that keeps calling tools nobody can approve, varying the command each time
//!   so the repeated-call guard never fires, is stopped after five denials instead of
//!   spending iterations and quota;
//! - what stdin carries reaches the model with the prompt, and a pipe that never sends
//!   anything does not hang the run;
//! - `neuron --resume SESSION prompt TEXT` sends a saved session another prompt;
//! - a bare `neuron` with nothing on stdin, piped slash commands and piped `--no-tui`
//!   input are refused before any request, with or without a byte order mark;
//! - a slash command Git Bash turned into a path is a slash command again.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mock_anthropic_service::{MockAnthropicService, SCENARIO_PREFIX};
use serde_json::Value;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

const STOP_MESSAGE: &str = "stopped: 5 tool calls were denied in a non-interactive run (bash \u{00d7}5); re-run with --permission-mode danger-full-access or allow the tools in settings";

#[test]
fn a_run_stops_after_five_denied_tool_calls_with_valid_json() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let workspace = Workspace::new("denials-json");

    let output = run(
        &workspace,
        &server.base_url(),
        &["--output-format=json"],
        "bash_denial_loop",
        Stdin::Null,
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stdout}\n{stderr}");
    // stdout is one JSON answer: the error, and what the turn did.
    let answer: Value = serde_json::from_str(stdout.trim()).expect("stdout is JSON");
    assert_eq!(answer["type"], "error", "{answer}");
    assert!(
        answer["error"]
            .as_str()
            .is_some_and(|error| error.starts_with(STOP_MESSAGE)),
        "{answer}"
    );
    let results = answer["tool_results"].as_array().expect("tool results");
    assert_eq!(results.len(), 5, "{answer}");
    assert!(results.iter().all(|result| result["is_error"] == true));
    // stderr carries JSON only, ending with the same error.
    let lines = json_lines(&stderr);
    let last = lines.last().expect("a stderr line");
    assert_eq!(last["type"], "error", "{stderr}");
    assert_eq!(last["error"], answer["error"], "{stderr}");
    assert_eq!(last["denied_tools"].as_array().map(Vec::len), Some(5));

    let requests = runtime.block_on(server.captured_requests());
    let model_calls = requests
        .iter()
        .filter(|request| request.path == "/v1/messages")
        .count();
    assert_eq!(model_calls, 5, "no model call after the fifth denial");
    assert_no_command_ran(&workspace.root);
    workspace.remove();
}

#[test]
fn a_text_run_says_why_it_stopped() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let workspace = Workspace::new("denials-text");

    let output = run(
        &workspace,
        &server.base_url(),
        &[],
        "bash_denial_loop",
        Stdin::Null,
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stdout}\n{stderr}");
    assert!(stderr.contains(STOP_MESSAGE), "{stderr}");
    assert!(!stdout.contains("Interrupted"), "{stdout}");
    assert_no_command_ran(&workspace.root);
    workspace.remove();
}

/// `cat log | neuron prompt "..."`, in a mode that is not danger-full-access.
#[test]
fn piped_stdin_reaches_the_model_after_the_prompt() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let workspace = Workspace::new("stdin-piped");
    let log = "2026-10-24 ERROR piped-log-marker db timeout\n";

    let output = run(
        &workspace,
        &server.base_url(),
        &["--output-format=json"],
        "streaming_text",
        Stdin::Data(log),
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    let requests = runtime.block_on(server.captured_requests());
    let request = requests
        .iter()
        .find(|request| request.path == "/v1/messages")
        .expect("a model call");
    let body: Value = serde_json::from_str(&request.raw_body).expect("JSON body");
    let prompt = body["messages"][0]["content"][0]["text"]
        .as_str()
        .expect("prompt text");
    assert_eq!(
        prompt,
        format!("{SCENARIO_PREFIX}streaming_text\n\n{}", log.trim())
    );
    workspace.remove();
}

/// A harness that leaves stdin open and never writes: the run goes on without it.
#[test]
fn a_silent_stdin_pipe_does_not_hang_the_run() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let workspace = Workspace::new("stdin-silent");

    let started = Instant::now();
    let output = run(
        &workspace,
        &server.base_url(),
        &["--output-format=json"],
        "streaming_text",
        Stdin::OpenAndSilent,
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(started.elapsed() < Duration::from_secs(60));
    let notes = json_lines(&stderr);
    assert!(
        notes.iter().any(|line| line["type"] == "notice"
            && line["notice"]
                .as_str()
                .is_some_and(|notice| notice.contains("nothing arrived on it"))),
        "{stderr}"
    );
    let answer: Value = serde_json::from_slice(&output.stdout).expect("stdout is JSON");
    assert_eq!(answer["outcome"], "completed", "{answer}");
    workspace.remove();
}

/// A script continues a saved session: the model sees the earlier turn, the new turn is
/// saved to the same file, and stdout is the JSON answer alone.
#[test]
fn a_saved_session_takes_another_prompt_without_a_terminal() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let workspace = Workspace::new("resume-prompt");

    let first = run(
        &workspace,
        &server.base_url(),
        &[],
        "streaming_text",
        Stdin::Null,
    );
    assert_eq!(
        first.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let sessions = session_files(&workspace.root);
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    let first_size = fs::metadata(&sessions[0]).expect("session file").len();

    let second_prompt = format!("second-turn {SCENARIO_PREFIX}streaming_text");
    let second = neuron(&workspace, &server.base_url())
        .args(["--output-format", "json", "--resume", "latest", "prompt"])
        .arg(&second_prompt)
        .stdin(Stdio::null())
        .output()
        .expect("neuron should run");

    let stdout = String::from_utf8_lossy(&second.stdout);
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert_eq!(second.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(!stdout.contains("Restored"), "{stdout}");
    let answer: Value = serde_json::from_str(stdout.trim()).expect("stdout is one JSON answer");
    assert_eq!(answer["outcome"], "completed", "{answer}");

    let requests = runtime.block_on(server.captured_requests());
    let model_calls = requests
        .iter()
        .filter(|request| request.path == "/v1/messages")
        .collect::<Vec<_>>();
    assert_eq!(model_calls.len(), 2);
    let body: Value = serde_json::from_str(&model_calls[1].raw_body).expect("JSON body");
    let messages = body["messages"].as_array().expect("messages");
    let roles = messages
        .iter()
        .map(|message| message["role"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(roles, ["user", "assistant", "user"], "{body}");
    assert_eq!(
        messages[0]["content"][0]["text"],
        format!("{SCENARIO_PREFIX}streaming_text"),
        "{body}"
    );
    assert_eq!(messages[2]["content"][0]["text"], second_prompt, "{body}");

    // The same file, grown; no other session was left behind.
    assert_eq!(session_files(&workspace.root), sessions);
    assert!(fs::metadata(&sessions[0]).expect("session file").len() > first_size);
    workspace.remove();
}

/// `neuron </dev/null` in a folder nobody trusted yet: no trust question nobody can
/// answer, no request, and no empty session left for `--resume latest`.
#[test]
fn a_bare_run_without_a_terminal_has_nothing_to_do() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    // A pipe that brings only a byte order mark (Windows PowerShell's) brings nothing.
    for stdin in [
        Stdin::Null,
        Stdin::Data("\u{feff}"),
        Stdin::Data("\u{feff}\r\n"),
    ] {
        let workspace = Workspace::new("bare-no-terminal");

        let output = run_bare(&workspace, &server.base_url(), &[], stdin);

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{stdout}\n{stderr}");
        assert!(
            stderr.contains(
                "nothing to do: no prompt was given and standard input is not a terminal"
            ),
            "{stderr}"
        );
        assert!(stderr.contains("neuron prompt \"...\""), "{stderr}");
        assert!(!stdout.contains("Press 1") && !stderr.contains("Press 1"));
        assert!(session_files(&workspace.root).is_empty());
        workspace.remove();
    }
    assert!(runtime.block_on(server.captured_requests()).is_empty());
}

/// `printf '/help\n/exit' | neuron`: piped text would reach the model as one prompt, so
/// slash commands are refused before any request, as is piped text with `--no-tui`
/// (the line REPL, which reads a terminal).
#[test]
fn piped_repl_input_is_refused_before_any_request() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");

    // Windows PowerShell pipes, and files saved as UTF-8 with BOM, start with U+FEFF.
    for (extra, stdin, says) in [
        (&[][..], "/help\n/exit\n", "slash commands can't be piped"),
        (
            &[][..],
            "\u{feff}/help\r\n/exit\r\n",
            "slash commands can't be piped",
        ),
        (&["--no-tui"][..], "fix the bug\n", "leave out --no-tui"),
        (
            &["--no-tui"][..],
            "\u{feff}fix the bug\r\n",
            "leave out --no-tui",
        ),
    ] {
        let workspace = Workspace::new("piped-repl-input");
        let output = run_bare(&workspace, &server.base_url(), extra, Stdin::Data(stdin));

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{stdout}\n{stderr}");
        assert!(stderr.contains(says), "{stderr}");
        assert!(session_files(&workspace.root).is_empty());
        workspace.remove();
    }
    assert!(runtime.block_on(server.captured_requests()).is_empty());
}

/// Git Bash turned `neuron /model` into `neuron "C:/Program Files/Git/model"`: Neuron
/// treats it as /model and says so, except in JSON mode, where stderr carries only JSON.
#[test]
fn a_git_bash_slash_command_is_noted_except_in_json_mode() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");

    for (extra, json) in [(&[][..], false), (&["--output-format", "json"][..], true)] {
        let workspace = Workspace::new("git-bash-slash");
        let mut command = neuron(&workspace, &server.base_url());
        command
            .env("MSYSTEM", "MINGW64")
            .args(extra)
            .arg("C:/Program Files/Git/model");
        let output = output_with_stdin(command, Stdin::Null);

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{stdout}\n{stderr}");
        // /model is interactive-only: refused as a slash command, not sent as a prompt.
        assert!(stderr.contains("slash command /model"), "{stderr}");
        assert_eq!(
            stderr.contains("Git Bash turned /model into a path"),
            !json,
            "{stderr}"
        );
        if json {
            for line in stderr.lines().filter(|line| !line.trim().is_empty()) {
                let parsed: Value = serde_json::from_str(line)
                    .unwrap_or_else(|error| panic!("stderr line is not JSON ({error}): {line}"));
                assert_eq!(parsed["type"], "error", "{line}");
            }
        }
        workspace.remove();
    }
    assert!(runtime.block_on(server.captured_requests()).is_empty());
}

#[derive(Clone, Copy)]
enum Stdin {
    Null,
    Data(&'static str),
    /// A pipe kept open, with nothing written, until the run ends.
    OpenAndSilent,
}

/// `neuron --permission-mode workspace-write [extra] <scenario>`.
fn run(
    workspace: &Workspace,
    base_url: &str,
    extra: &[&str],
    scenario: &str,
    stdin: Stdin,
) -> Output {
    let mut command = neuron(workspace, base_url);
    command
        .args(["--model", "sonnet", "--permission-mode", "workspace-write"])
        .args(extra)
        .arg(format!("{SCENARIO_PREFIX}{scenario}"));
    output_with_stdin(command, stdin)
}

/// A bare `neuron --permission-mode workspace-write [extra]`: options, no prompt.
fn run_bare(workspace: &Workspace, base_url: &str, extra: &[&str], stdin: Stdin) -> Output {
    let mut command = neuron(workspace, base_url);
    command
        .args(["--model", "sonnet", "--permission-mode", "workspace-write"])
        .args(extra);
    output_with_stdin(command, stdin)
}

fn output_with_stdin(mut command: Command, stdin: Stdin) -> Output {
    match stdin {
        Stdin::Null => command
            .stdin(Stdio::null())
            .output()
            .expect("neuron should run"),
        Stdin::Data(data) => {
            let mut child = command
                .stdin(Stdio::piped())
                .spawn()
                .expect("neuron should run");
            child
                .stdin
                .take()
                .expect("stdin is piped")
                .write_all(data.as_bytes())
                .expect("stdin written");
            child.wait_with_output().expect("neuron should finish")
        }
        Stdin::OpenAndSilent => {
            let mut child = command
                .stdin(Stdio::piped())
                .spawn()
                .expect("neuron should run");
            let open = child.stdin.take();
            let output = child.wait_with_output().expect("neuron should finish");
            drop(open);
            output
        }
    }
}

/// `neuron` in `workspace`, talking to the mock service, with stdout and stderr captured.
fn neuron(workspace: &Workspace, base_url: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_neuron"));
    command
        .current_dir(&workspace.root)
        .env_clear()
        .env("ANTHROPIC_API_KEY", "test-parity-key")
        .env("ANTHROPIC_BASE_URL", base_url)
        .env("CLAW_CONFIG_HOME", &workspace.config_home)
        .env("HOME", &workspace.home)
        .env("USERPROFILE", &workspace.home)
        .env("NO_COLOR", "1")
        .env("PATH", "/usr/bin:/bin")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Windows networking needs SystemRoot even in a cleared environment.
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", system_root);
    }
    #[cfg(windows)]
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command
}

/// The saved sessions of the workspace at `root`.
fn session_files(root: &Path) -> Vec<PathBuf> {
    let mut files = fs::read_dir(root.join(".neuron").join("sessions"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|workspace| fs::read_dir(workspace.path()).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect::<Vec<_>>();
    files.sort();
    files
}

/// Every line of `text` as JSON.
fn json_lines(text: &str) -> Vec<Value> {
    text.lines()
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|_| panic!("stderr line is not JSON: {line}\n{text}"))
        })
        .collect()
}

fn assert_no_command_ran(root: &Path) {
    let ran = fs::read_dir(root)
        .expect("workspace")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("loop-"))
        .count();
    assert_eq!(ran, 0, "a denied command ran");
}

struct Workspace {
    root: PathBuf,
    config_home: PathBuf,
    home: PathBuf,
}

impl Workspace {
    fn new(label: &str) -> Self {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_millis();
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "neuron-headless-{label}-{}-{millis}-{counter}",
            std::process::id()
        ));
        let workspace = Self {
            config_home: root.join("config-home"),
            home: root.join("home"),
            root,
        };
        fs::create_dir_all(&workspace.config_home).expect("config home");
        fs::create_dir_all(&workspace.home).expect("home");
        // Keep the default MCP servers (spawned through npx) out of the test.
        fs::write(
            workspace.config_home.join("settings.json"),
            r#"{"defaultMcpServers":false}"#,
        )
        .expect("settings");
        workspace
    }

    fn remove(self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
