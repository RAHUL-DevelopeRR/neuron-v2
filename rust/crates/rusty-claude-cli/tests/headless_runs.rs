//! `neuron prompt` runs nobody attends (stdin is not a terminal), against the mock
//! Anthropic service:
//! - a model that keeps calling tools nobody can approve, varying the command each time
//!   so the repeated-call guard never fires, is stopped after five denials instead of
//!   spending iterations and quota;
//! - a denial names the `permissions.allow` rules that would let the call run before
//!   full access, and those rules in user settings do let it run, a command that needs
//!   two programs approved included;
//! - what stdin carries reaches the model with the prompt, and a pipe that never sends
//!   anything does not hang the run;
//! - `neuron --resume SESSION prompt TEXT` sends a saved session another prompt; a
//!   session another run is continuing is refused, so runs at once lose no turn;
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

/// The allow rule for the loop's `printf ... | tee loop-N.txt` comes first; full access
/// only for a disposable environment.
const STOP_MESSAGE: &str = "stopped: 5 tool calls were denied in a non-interactive run (bash \u{00d7}5); to allow them in scripts, add \"bash(tee:*)\" to permissions.allow in your user settings (~/.neuron/settings.json), or, only in a disposable environment, re-run with --permission-mode danger-full-access";

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

/// A web search on a one-line question was answered with "re-run with
/// --permission-mode danger-full-access" first, to the user and to the model, which
/// repeated it. The allow rule comes first now; full access only as a last resort.
#[test]
fn a_denied_web_search_names_the_allow_rule_first() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let workspace = Workspace::new("web-search-denied");

    let output = run(
        &workspace,
        &server.base_url(),
        &[],
        "web_search_denied",
        Stdin::Null,
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stdout}\n{stderr}");
    let denial = "tool 'WebSearch' was not run: nobody could approve it in a non-interactive run. To allow it in scripts, add \"WebSearch\" to permissions.allow in your user settings (~/.neuron/settings.json), or, only in a disposable environment, re-run with --permission-mode danger-full-access";
    // The user is told at once, and the model gets the same reason as the result.
    assert!(stderr.contains(denial), "{stderr}");
    assert!(stdout.contains(denial), "{stdout}");
    let warning = stderr
        .lines()
        .find(|line| line.starts_with("warning: non-interactive run:"))
        .expect("an end-of-run warning");
    let rule = warning.find("\"WebSearch\"").expect(warning);
    let full_access = warning.find("danger-full-access").expect(warning);
    assert!(rule < full_access, "{warning}");
    workspace.remove();
}

/// The rule a denial names, in user settings, lets the same call run without a
/// terminal (the workspace is not trusted, so only user settings count).
#[test]
fn the_named_allow_rule_lets_the_call_run_in_a_script() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");

    let denied = Workspace::new("rule-named");
    let output = run(
        &denied,
        &server.base_url(),
        &[],
        "bash_permission_prompt_denied",
        Stdin::Null,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("add \"bash(tee:*)\" to permissions.allow"),
        "{stderr}"
    );
    assert!(!denied.root.join("denied.txt").exists());
    denied.remove();

    let allowed = Workspace::new("rule-allowed");
    fs::write(
        allowed.config_home.join("settings.json"),
        r#"{"defaultMcpServers":false,"permissions":{"allow":["bash(tee:*)"]}}"#,
    )
    .expect("settings");
    let output = run(
        &allowed,
        &server.base_url(),
        &[],
        "bash_permission_prompt_denied",
        Stdin::Null,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(!stderr.contains("was not run"), "{stderr}");
    assert_eq!(
        fs::read_to_string(allowed.root.join("denied.txt")).expect("the command ran"),
        "should not run"
    );
    allowed.remove();
}

/// A command that needs two programs approved names a rule for each, and the two
/// rules together, in user settings, let it run: prefix rules count together.
#[test]
fn the_rules_named_for_a_compound_command_let_it_run_together() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");

    let denied = Workspace::new("compound-named");
    let output = run(
        &denied,
        &server.base_url(),
        &[],
        "compound_command_denied",
        Stdin::Null,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("add \"bash(mkdir:*)\", \"bash(tee:*)\" to permissions.allow"),
        "{stderr}"
    );
    assert!(!denied.root.join("made").exists());
    denied.remove();

    let allowed = Workspace::new("compound-allowed");
    fs::write(
        allowed.config_home.join("settings.json"),
        r#"{"defaultMcpServers":false,"permissions":{"allow":["bash(mkdir:*)","bash(tee:*)"]}}"#,
    )
    .expect("settings");
    let output = run(
        &allowed,
        &server.base_url(),
        &[],
        "compound_command_denied",
        Stdin::Null,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(!stderr.contains("was not run"), "{stderr}");
    assert_eq!(
        fs::read_to_string(allowed.root.join("made").join("out.txt")).expect("the command ran"),
        "both ran"
    );
    allowed.remove();
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

/// A script continues a saved session: the model sees the earlier turns, each turn is
/// saved to the same file, and stdout is the JSON answer alone, naming the session so the
/// script can continue it by id. Without `--model` the session's model goes on; `-p` with
/// nothing after it takes the prompt from stdin, as `neuron -p` does.
#[test]
#[allow(clippy::too_many_lines)]
fn a_saved_session_takes_another_prompt_without_a_terminal() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let workspace = Workspace::new("resume-prompt");

    let first = run(
        &workspace,
        &server.base_url(),
        &["--output-format=json"],
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
    let session_id = sessions[0]
        .file_stem()
        .and_then(|stem| stem.to_str())
        .expect("session id")
        .to_string();
    let first_answer: Value = serde_json::from_slice(&first.stdout).expect("stdout is JSON");
    assert_eq!(first_answer["session_id"], session_id, "{first_answer}");
    let first_size = fs::metadata(&sessions[0]).expect("session file").len();

    let second_prompt = format!("second-turn {SCENARIO_PREFIX}streaming_text");
    let second = neuron(&workspace, &server.base_url())
        .args(["--output-format", "json", "--resume", &session_id, "prompt"])
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
    assert_eq!(answer["session_id"], session_id, "{answer}");

    let third_prompt = format!("third-turn {SCENARIO_PREFIX}streaming_text");
    let mut third = neuron(&workspace, &server.base_url())
        .args(["--model", "opus", "--resume", "latest", "-p"])
        .stdin(Stdio::piped())
        .spawn()
        .expect("neuron should run");
    third
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(third_prompt.as_bytes())
        .expect("stdin written");
    let third = third.wait_with_output().expect("neuron should finish");
    assert_eq!(
        third.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&third.stderr)
    );

    let requests = runtime.block_on(server.captured_requests());
    let bodies = requests
        .iter()
        .filter(|request| request.path == "/v1/messages")
        .map(|request| serde_json::from_str::<Value>(&request.raw_body).expect("JSON body"))
        .collect::<Vec<_>>();
    assert_eq!(bodies.len(), 3);
    let roles = |body: &Value| {
        body["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|message| message["role"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        roles(&bodies[1]),
        ["user", "assistant", "user"],
        "{}",
        bodies[1]
    );
    assert_eq!(
        bodies[1]["messages"][0]["content"][0]["text"],
        format!("{SCENARIO_PREFIX}streaming_text"),
        "{}",
        bodies[1]
    );
    assert_eq!(
        bodies[1]["messages"][2]["content"][0]["text"], second_prompt,
        "{}",
        bodies[1]
    );
    assert_eq!(
        roles(&bodies[2]),
        ["user", "assistant", "user", "assistant", "user"],
        "{}",
        bodies[2]
    );
    assert_eq!(
        bodies[2]["messages"][4]["content"][0]["text"], third_prompt,
        "{}",
        bodies[2]
    );
    // The session's model, until `--model` names another.
    assert_eq!(bodies[1]["model"], bodies[0]["model"]);
    assert!(
        bodies[2]["model"]
            .as_str()
            .is_some_and(|model| model.contains("opus")),
        "{}",
        bodies[2]["model"]
    );

    // The same file, grown; no other session was left behind.
    assert_eq!(session_files(&workspace.root), sessions);
    assert!(fs::metadata(&sessions[0]).expect("session file").len() > first_size);
    workspace.remove();
}

/// While another run continues a session (here the test holds its lock, as that run
/// would), a run that would continue it too is refused, before any request, instead of
/// overwriting that run's turns when it saves; nothing is left behind. Resume commands
/// that rewrite the session are refused too; reading it is not.
#[test]
fn a_session_in_use_is_refused_and_left_as_it_was() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let workspace = Workspace::new("resume-in-use");
    let first = run(
        &workspace,
        &server.base_url(),
        &[],
        "streaming_text",
        Stdin::Null,
    );
    assert_eq!(first.status.code(), Some(0));
    let sessions = session_files(&workspace.root);
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    let saved = fs::read(&sessions[0]).expect("session file");

    let mut lock_name = sessions[0].file_name().expect("file name").to_os_string();
    lock_name.push(".lock");
    let held = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(sessions[0].with_file_name(lock_name))
        .expect("lock file");
    held.try_lock().expect("the test holds the session");

    let prompt = format!("second-turn {SCENARIO_PREFIX}streaming_text");
    for (args, json) in [
        (
            &[
                "--output-format",
                "json",
                "--resume",
                "latest",
                "prompt",
                &prompt,
            ][..],
            true,
        ),
        (&["--resume", "latest", "-p", &prompt], false),
        (&["--resume", "latest", "/compact"], false),
    ] {
        let output = neuron(&workspace, &server.base_url())
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("neuron should run");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{args:?}\n{stdout}\n{stderr}"
        );
        assert!(
            stderr.contains("is in use by another neuron run"),
            "{args:?}\n{stderr}"
        );
        if json {
            assert!(stdout.trim().is_empty(), "{stdout}");
            for line in json_lines(&stderr) {
                assert_eq!(line["type"], "error", "{stderr}");
            }
        }
    }
    let status = neuron(&workspace, &server.base_url())
        .args(["--resume", "latest", "/status"])
        .stdin(Stdio::null())
        .output()
        .expect("neuron should run");
    assert_eq!(
        status.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );

    let model_calls = runtime
        .block_on(server.captured_requests())
        .iter()
        .filter(|request| request.path == "/v1/messages")
        .count();
    assert_eq!(model_calls, 1, "only the first run reached the model");
    assert_eq!(fs::read(&sessions[0]).expect("session file"), saved);
    assert_eq!(session_files(&workspace.root), sessions);

    // Once that run is over, the session takes the next prompt.
    drop(held);
    let next = neuron(&workspace, &server.base_url())
        .args(["--resume", "latest", "prompt"])
        .arg(&prompt)
        .stdin(Stdio::null())
        .output()
        .expect("neuron should run");
    assert_eq!(
        next.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&next.stderr)
    );
    assert_eq!(session_files(&workspace.root), sessions);
    workspace.remove();
}

/// Runs that continue one session at the same time: each adds its turn or is refused as
/// the session is in use. None overwrites another's turn or lands between a prompt and
/// its answer (each run used to save its whole session at the end, so the last one won).
#[test]
fn parallel_runs_on_one_session_lose_no_turn() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let workspace = Workspace::new("resume-parallel");
    let first = run(
        &workspace,
        &server.base_url(),
        &[],
        "streaming_text",
        Stdin::Null,
    );
    assert_eq!(first.status.code(), Some(0));
    let sessions = session_files(&workspace.root);
    assert_eq!(sessions.len(), 1, "{sessions:?}");

    let prompts = (0..6)
        .map(|turn| format!("turn-{turn} {SCENARIO_PREFIX}streaming_text"))
        .collect::<Vec<_>>();
    let runs = prompts
        .iter()
        .map(|prompt| {
            neuron(&workspace, &server.base_url())
                .args(["--output-format", "json", "--resume", "latest", "prompt"])
                .arg(prompt)
                .stdin(Stdio::null())
                .spawn()
                .expect("neuron should run")
        })
        .collect::<Vec<_>>();
    let mut completed = vec![format!("{SCENARIO_PREFIX}streaming_text")];
    for (prompt, run) in prompts.iter().zip(runs) {
        let output = run.wait_with_output().expect("neuron should finish");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        match output.status.code() {
            Some(0) => {
                let answer: Value = serde_json::from_str(stdout.trim()).expect("one JSON answer");
                assert_eq!(answer["outcome"], "completed", "{answer}");
                completed.push(prompt.clone());
            }
            Some(1) => assert!(
                stderr.contains("is in use by another neuron run"),
                "{stderr}"
            ),
            other => panic!("{prompt}: exit {other:?}\n{stdout}\n{stderr}"),
        }
    }
    assert!(
        completed.len() > 1,
        "at least one run continued the session"
    );

    // Every completed turn is saved, each prompt followed by its answer.
    let messages = saved_messages(&sessions[0]);
    let roles = messages
        .iter()
        .map(|(role, _)| role.as_str())
        .collect::<Vec<_>>();
    let expected_roles = ["user", "assistant"].repeat(completed.len());
    assert_eq!(roles, expected_roles, "{messages:?}");
    let mut prompts_saved = messages
        .iter()
        .filter(|(role, _)| role == "user")
        .map(|(_, text)| text.clone())
        .collect::<Vec<_>>();
    prompts_saved.sort();
    completed.sort();
    assert_eq!(prompts_saved, completed);
    assert_eq!(session_files(&workspace.root), sessions);
    workspace.remove();
}

/// A session of another workspace is refused before any request, and the refused run
/// leaves no session of its own behind.
#[test]
fn a_session_of_another_workspace_is_refused_without_a_trace() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let owner = Workspace::new("resume-owner");
    let first = run(
        &owner,
        &server.base_url(),
        &[],
        "streaming_text",
        Stdin::Null,
    );
    assert_eq!(first.status.code(), Some(0));
    let sessions = session_files(&owner.root);
    assert_eq!(sessions.len(), 1, "{sessions:?}");

    let other = Workspace::new("resume-other");
    let output = neuron(&other, &server.base_url())
        .args(["--output-format", "json", "--resume"])
        .arg(&sessions[0])
        .arg("prompt")
        .arg(format!("second-turn {SCENARIO_PREFIX}streaming_text"))
        .stdin(Stdio::null())
        .output()
        .expect("neuron should run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("session workspace mismatch"), "{stderr}");
    assert_eq!(session_files(&other.root), Vec::<std::path::PathBuf>::new());
    let model_calls = runtime
        .block_on(server.captured_requests())
        .iter()
        .filter(|request| request.path == "/v1/messages")
        .count();
    assert_eq!(model_calls, 1);
    owner.remove();
    other.remove();
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
        assert_eq!(
            session_files(&workspace.root),
            Vec::<std::path::PathBuf>::new()
        );
        workspace.remove();
    }
    assert_eq!(
        runtime.block_on(server.captured_requests()),
        Vec::<mock_anthropic_service::CapturedRequest>::new()
    );
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
        assert_eq!(
            session_files(&workspace.root),
            Vec::<std::path::PathBuf>::new()
        );
        workspace.remove();
    }
    assert_eq!(
        runtime.block_on(server.captured_requests()),
        Vec::<mock_anthropic_service::CapturedRequest>::new()
    );
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
    assert_eq!(
        runtime.block_on(server.captured_requests()),
        Vec::<mock_anthropic_service::CapturedRequest>::new()
    );
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

/// The messages saved in the session file at `path`: each one's role and first text.
fn saved_messages(path: &Path) -> Vec<(String, String)> {
    fs::read_to_string(path)
        .expect("session file")
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| record["type"] == "message")
        .map(|record| {
            let message = &record["message"];
            (
                message["role"].as_str().unwrap_or_default().to_string(),
                message["blocks"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
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
