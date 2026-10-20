//! `neuron engine --stdio` end to end: the built binary against the mock Anthropic
//! service, driven over stdin, every stdout line a JSON event.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mock_anthropic_service::{MockAnthropicService, SCENARIO_PREFIX};
use serde_json::{json, Value};

/// Generous: a turn talks to the mock service over HTTP and may start a shell.
const EVENT_TIMEOUT: Duration = Duration::from_secs(60);

fn unique_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "neuron-engine-stdio-{label}-{}-{nanos}",
        std::process::id()
    ))
}

/// The engine process and its event lines, read on a thread so a hang fails the test
/// instead of blocking it.
struct StdioEngine {
    child: Child,
    stdin: Option<ChildStdin>,
    events: Receiver<Value>,
}

impl StdioEngine {
    fn start(workspace: &Path, home: &Path, base_url: &str) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_neuron"));
        command
            .current_dir(workspace)
            .env_clear()
            .env("ANTHROPIC_API_KEY", "test-engine-stdio-key")
            .env("ANTHROPIC_BASE_URL", base_url)
            .env("NEURON_CONFIG_HOME", home.join("config"))
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("NO_COLOR", "1")
            .env("PATH", "/usr/bin:/bin")
            .args([
                "engine",
                "--stdio",
                "--model",
                "sonnet",
                "--permission-mode",
                "workspace-write",
                "--allowedTools",
                "bash",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Windows networking needs SystemRoot even in a cleared environment, and the bash
        // tool needs the real PATH to find a POSIX shell (Git for Windows).
        if let Some(system_root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system_root);
        }
        #[cfg(windows)]
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        let mut child = command.spawn().expect("neuron engine starts");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        // Drained so the process never blocks on a full stderr pipe.
        thread::spawn(move || for _ in BufReader::new(stderr).lines() {});
        let (events_tx, events) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.expect("stdout is UTF-8");
                let event: Value = serde_json::from_str(&line).unwrap_or_else(|error| {
                    panic!("stdout carries only JSON events: {line:?} ({error})")
                });
                assert!(event["type"].is_string(), "an event has a type: {line}");
                if events_tx.send(event).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            events,
        }
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin is open");
        writeln!(stdin, "{message}").expect("the engine reads its input");
        stdin.flush().expect("input flushed");
    }

    fn send_line(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("stdin is open");
        writeln!(stdin, "{line}").expect("the engine reads its input");
        stdin.flush().expect("input flushed");
    }

    fn next(&self) -> Value {
        self.events
            .recv_timeout(EVENT_TIMEOUT)
            .expect("the engine reports in time")
    }

    /// Events up to and including the first whose type is `last`.
    fn until(&self, last: &str) -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let event = self.next();
            let done = event["type"] == last;
            seen.push(event);
            if done {
                return seen;
            }
        }
    }
}

/// The events without notices and footer updates.
fn main_events(events: &[Value]) -> Vec<Value> {
    events
        .iter()
        .filter(|event| !matches!(event["type"].as_str(), Some("notice" | "status")))
        .cloned()
        .collect()
}

fn submit(scenario: &str) -> Value {
    json!({
        "kind": "command",
        "type": "submit",
        "text": format!("{SCENARIO_PREFIX}{scenario}"),
        "attachments": [],
    })
}

fn counts(input: u64, output: u64) -> Value {
    json!({"input": input, "output": output, "cache_read": 0, "cache_write": 0})
}

#[test]
#[allow(clippy::too_many_lines)]
fn stdio_engine_runs_turns_over_json_lines() {
    let tokio = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = tokio
        .block_on(MockAnthropicService::spawn())
        .expect("the mock service starts");
    let workspace = unique_dir("workspace");
    let home = unique_dir("home");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::create_dir_all(home.join("config")).expect("config home");
    fs::write(
        home.join("config").join("settings.json"),
        r#"{"defaultMcpServers":false}"#,
    )
    .expect("settings");

    let mut engine = StdioEngine::start(&workspace, &home, &server.base_url());

    // The protocol version first, then the footer.
    let ready = engine.next();
    assert_eq!(ready["type"], "ready", "{ready}");
    assert_eq!(ready["protocol"], 1);
    assert_eq!(ready["version"], env!("CARGO_PKG_VERSION"));
    let status = engine.next();
    assert_eq!(status["type"], "status", "{status}");
    assert_eq!(status["model"], "claude-sonnet-4-6");
    assert_eq!(status["permission_mode"], "workspace-write");

    // A line that is not a message is skipped with a notice; the engine keeps going.
    engine.send_line("{\"kind\":\"nonsense\"}");
    loop {
        let event = engine.next();
        assert_eq!(event["type"], "notice", "only notices meanwhile: {event}");
        if event["message"]
            .as_str()
            .is_some_and(|message| message.contains("skipped an input line"))
        {
            break;
        }
    }

    // Streamed text, then the turn's usage.
    engine.send(&submit("streaming_text"));
    let turn = engine.until("turn_finished");
    assert_eq!(
        main_events(&turn),
        vec![
            json!({"type": "text_delta", "text": "Mock streaming "}),
            json!({"type": "text_delta", "text": "says hello from the parity harness."}),
            json!({"type": "text_block_end"}),
            json!({
                "type": "usage",
                "turn": counts(11, 8),
                "cumulative": counts(11, 8),
                "estimated": false,
            }),
            json!({"type": "turn_finished"}),
        ]
    );
    assert_eq!(
        engine.until("status").len(),
        1,
        "the footer follows the turn"
    );

    // A permission request answered over stdin.
    engine.send(&submit("bash_permission_prompt_approved"));
    let asked = main_events(&engine.until("permission_requested"));
    assert_eq!(asked.len(), 2, "{asked:?}");
    assert_eq!(asked[0]["type"], "tool_call_started");
    assert_eq!(asked[0]["name"], "bash");
    assert_eq!(asked[1]["request"]["tool"], "bash");
    assert_eq!(asked[1]["request"]["required_mode"], "danger-full-access");
    let id = asked[1]["id"].as_u64().expect("a request id");
    engine.send(&json!({
        "kind": "permission_reply",
        "id": id,
        "decision": {"kind": "allow_once"},
    }));
    let turn = main_events(&engine.until("turn_finished"));
    let types = turn
        .iter()
        .map(|event| event["type"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(
        types,
        [
            "tool_result",
            "text_delta",
            "text_block_end",
            "usage",
            "turn_finished"
        ],
        "{turn:?}"
    );
    assert_eq!(turn[0]["is_error"], false);
    assert_eq!(
        turn[1]["text"],
        "bash approved and executed: approved via prompt"
    );
    assert!(workspace.join("approved.txt").exists());

    // End of input: the engine saves and the process exits cleanly.
    drop(engine.stdin.take());
    let deadline = std::time::Instant::now() + EVENT_TIMEOUT;
    let exit = loop {
        if let Some(exit) = engine.child.try_wait().expect("process state") {
            break exit;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the engine did not exit after its input ended"
        );
        thread::sleep(Duration::from_millis(50));
    };
    assert!(exit.success(), "exit status {exit:?}");
    loop {
        match engine.events.recv_timeout(EVENT_TIMEOUT) {
            Ok(event) => assert!(
                matches!(event["type"].as_str(), Some("status" | "notice")),
                "nothing else after the input ended: {event}"
            ),
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => panic!("stdout did not close"),
        }
    }

    drop(server);
    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn a_launch_that_fails_reports_one_error_event() {
    let workspace = unique_dir("bad-launch");
    let home = unique_dir("bad-launch-home");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::create_dir_all(home.join("config")).expect("config home");
    let mut command = Command::new(env!("CARGO_BIN_EXE_neuron"));
    command
        .current_dir(&workspace)
        .env_clear()
        .env("NEURON_CONFIG_HOME", home.join("config"))
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .args(["engine", "--stdio", "--permission-mode", "bogus"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", system_root);
    }
    let output = command.output().expect("neuron engine runs");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("UTF-8");
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "one event: {stdout}");
    let event: Value = serde_json::from_str(lines[0]).expect("a JSON event");
    assert_eq!(event["type"], "error", "{event}");
    assert!(
        event["message"]
            .as_str()
            .is_some_and(|message| message.contains("unsupported permission mode 'bogus'")),
        "{event}"
    );
    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_dir_all(&home);
}
