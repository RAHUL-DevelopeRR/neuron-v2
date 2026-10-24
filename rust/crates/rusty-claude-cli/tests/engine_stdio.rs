//! `neuron engine --stdio` end to end: the built binary driven over stdin, every stdout
//! line a JSON event. Turns run against the mock Anthropic service; the sign-in runs
//! against a fake Zero-X gateway, with a credential store of the test's own.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mock_anthropic_service::{MockAnthropicService, SCENARIO_PREFIX};
use serde_json::{json, Value};

/// Generous: a turn talks to the mock service over HTTP and may start a shell, and a
/// device sign-in polls every five seconds.
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

/// A workspace and a home of the test's own: config, credential store and no MCP servers.
fn workspace_and_home(label: &str) -> (PathBuf, PathBuf) {
    let workspace = unique_dir(&format!("{label}-workspace"));
    let home = unique_dir(&format!("{label}-home"));
    fs::create_dir_all(&workspace).expect("workspace");
    fs::create_dir_all(home.join("config")).expect("config home");
    fs::write(
        home.join("config").join("settings.json"),
        r#"{"defaultMcpServers":false}"#,
    )
    .expect("settings");
    (workspace, home)
}

/// The engine process and its event lines, read on a thread so a hang fails the test
/// instead of blocking it.
struct StdioEngine {
    child: Child,
    stdin: Option<ChildStdin>,
    events: Receiver<Value>,
}

impl StdioEngine {
    /// `neuron engine --stdio <args>` in a cleared environment plus `env`. A stdout line
    /// that is not a JSON event, or that contains `never_on_stdout`, fails the test.
    fn start(
        workspace: &Path,
        home: &Path,
        env: &[(&str, &str)],
        args: &[&str],
        never_on_stdout: Option<&'static str>,
    ) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_neuron"));
        command
            .current_dir(workspace)
            .env_clear()
            .env("NEURON_CONFIG_HOME", home.join("config"))
            .env("NEURON_HOME", home.join("neuron"))
            .env("NEURON_CREDENTIAL_STORE", "file")
            .env("NEURON_NO_BROWSER", "1")
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("NO_COLOR", "1")
            .env("PATH", "/usr/bin:/bin")
            .envs(env.iter().copied())
            .args(["engine", "--stdio"])
            .args(args)
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
                if let Some(secret) = never_on_stdout {
                    assert!(!line.contains(secret), "a secret reached stdout: {line}");
                }
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

    fn slash(&mut self, command: &str) {
        self.send(&json!({"kind": "command", "type": "slash", "command": command, "args": ""}));
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

    /// The next event of type `kind`, skipping the others.
    fn next_of(&self, kind: &str) -> Value {
        self.until(kind).pop().expect("an event")
    }

    /// Closes stdin and waits for a clean exit and the end of stdout.
    fn end_input(mut self) {
        drop(self.stdin.take());
        let deadline = std::time::Instant::now() + EVENT_TIMEOUT;
        let exit = loop {
            if let Some(exit) = self.child.try_wait().expect("process state") {
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
            match self.events.recv_timeout(EVENT_TIMEOUT) {
                Ok(event) => assert!(
                    matches!(event["type"].as_str(), Some("status" | "notice")),
                    "nothing else after the input ended: {event}"
                ),
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => panic!("stdout did not close"),
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

fn notices(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter(|event| event["type"] == "notice")
        .filter_map(|event| event["message"].as_str().map(str::to_string))
        .collect()
}

fn submit(text: &str) -> Value {
    json!({
        "kind": "command",
        "type": "submit",
        "text": text,
        "attachments": [],
    })
}

fn scenario(name: &str) -> Value {
    submit(&format!("{SCENARIO_PREFIX}{name}"))
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
    let base_url = server.base_url();
    let (workspace, home) = workspace_and_home("turns");

    let mut engine = StdioEngine::start(
        &workspace,
        &home,
        &[
            ("ANTHROPIC_API_KEY", "test-engine-stdio-key"),
            ("ANTHROPIC_BASE_URL", &base_url),
        ],
        &[
            "--model",
            "sonnet",
            "--permission-mode",
            "workspace-write",
            "--allowedTools",
            "bash",
        ],
        None,
    );

    // The protocol version first, then the footer.
    let ready = engine.next();
    assert_eq!(ready["type"], "ready", "{ready}");
    assert_eq!(ready["protocol"], 1);
    assert_eq!(ready["version"], env!("CARGO_PKG_VERSION"));
    let status = engine.next();
    assert_eq!(status["type"], "status", "{status}");
    assert_eq!(status["model"], "claude-sonnet-4-6");
    assert_eq!(status["permission_mode"], "workspace-write");
    assert_eq!(status["signed_in"], false, "no Zero-X login: {status}");
    assert_eq!(
        status["sign_in_required"], false,
        "an Anthropic key needs none"
    );
    assert_eq!(status["account"], Value::Null);

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
    engine.send(&scenario("streaming_text"));
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
    engine.send(&scenario("bash_permission_prompt_approved"));
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
    assert_eq!(turn[0]["id"], asked[0]["id"], "the result names its call");
    assert_eq!(turn[0]["is_error"], false);
    assert_eq!(
        turn[1]["text"],
        "bash approved and executed: approved via prompt"
    );
    assert!(workspace.join("approved.txt").exists());

    // End of input: the engine saves and the process exits cleanly.
    engine.end_input();

    drop(server);
    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_dir_all(&home);
}

/// The session token the fake gateway hands out: it must never reach stdout.
const SESSION: &str = "ses_abcdefghijklmnopqrstuvwxyz012345";
const DEVICE_CODE: &str = "zdc_0123456789abcdefghijABCDEFGHIJ_-01234567890";
const USER_CODE: &str = "BCDF-GHJK";
const EMAIL: &str = "dev@example.com";

/// The account endpoints of a Zero-X gateway, enough for a device sign-in, the model
/// catalog and a sign-out. Records each request as `METHOD /path`, plus ` (signed)` when it
/// carried the session.
struct FakeGateway {
    /// What `NEURON_API_BASE` points at.
    base: String,
    origin: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl FakeGateway {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a local port");
        let origin = format!("http://{}", listener.local_addr().expect("address"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        let page = format!("{origin}/device");
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let seen = Arc::clone(&seen);
                let page = page.clone();
                thread::spawn(move || answer(stream, &seen, &page));
            }
        });
        Self {
            base: format!("{origin}/v1"),
            origin,
            requests,
        }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests").clone()
    }
}

fn answer(stream: TcpStream, seen: &Mutex<Vec<String>>, page: &str) {
    let mut reader = BufReader::new(stream.try_clone().expect("stream"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut content_length = 0;
    let mut signed = false;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 {
            break;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
            if name.eq_ignore_ascii_case("authorization") {
                signed = value.trim() == format!("Bearer {SESSION}");
            }
        }
    }
    let mut body = vec![0; content_length];
    let _ = reader.read_exact(&mut body);
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    seen.lock().expect("requests").push(format!(
        "{method} {path}{}",
        if signed { " (signed)" } else { "" }
    ));

    let (status, reply) = match (method.as_str(), path.as_str()) {
        ("POST", "/auth/device/code") => (
            200,
            json!({
                "device_code": DEVICE_CODE,
                "user_code": USER_CODE,
                "verification_uri": page,
                "verification_uri_complete": format!("{page}?code={USER_CODE}"),
                "expires_in": 600,
                "interval": 5,
            }),
        ),
        // Approved at the first poll.
        ("POST", "/auth/device/token") => (
            200,
            json!({"access_token": SESSION, "email": EMAIL, "plan": "pro"}),
        ),
        ("GET", "/auth/session") if signed => (
            200,
            json!({"plan": "pro", "email": EMAIL, "ttl_remaining_seconds": 86_400}),
        ),
        ("GET", "/v1/models") if signed => (
            200,
            json!({"data": [
                {"id": "zx-fast", "display_name": "Fast", "context_window": 131_072,
                 "capabilities": {"tools": true}, "tiers": ["free"], "available": true},
                {"id": "zx-big", "display_name": "Big", "context_window": 262_144,
                 "tiers": ["pro"], "available": false, "unavailable_reason": "Pro plan only"},
            ]}),
        ),
        ("DELETE", "/auth/session") if signed => (204, Value::Null),
        _ => (
            404,
            json!({"error": {"code": "not_found", "message": "not found"}}),
        ),
    };
    let text = if reply.is_null() {
        String::new()
    } else {
        reply.to_string()
    };
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        _ => "Not Found",
    };
    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
        text.len()
    );
}

/// Nobody is signed in: the engine starts anyway, `/login` signs in from the UI with the
/// device code (an interrupt cancels it), the session starts on the gateway, and `/logout`
/// signs out. The session token never reaches stdout.
#[test]
#[allow(clippy::too_many_lines)]
fn stdio_engine_signs_in_from_the_ui() {
    let gateway = FakeGateway::start();
    let (workspace, home) = workspace_and_home("sign-in");
    let mut engine = StdioEngine::start(
        &workspace,
        &home,
        &[("NEURON_API_BASE", &gateway.base)],
        &[],
        Some(SESSION),
    );

    // Signed out: ready, why, and a footer that says so.
    assert_eq!(engine.next()["type"], "ready");
    let status = engine.next_of("status");
    assert_eq!(status["signed_in"], false, "{status}");
    assert_eq!(status["sign_in_required"], true, "{status}");
    assert_eq!(status["session_id"], "");
    assert_eq!(status["account"], Value::Null);

    // Prompts wait for the sign-in; so do commands that need a session.
    engine.send(&submit("hello"));
    let refused = main_events(&engine.until("turn_finished"));
    assert_eq!(refused.len(), 2, "{refused:?}");
    assert_eq!(refused[0]["type"], "error");
    assert!(
        refused[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("/login")),
        "{refused:?}"
    );
    engine.next_of("status");

    // An interrupt cancels a sign-in that waits for its approval (the first poll is five
    // seconds away).
    engine.slash("login");
    engine.next_of("sign_in_code");
    engine.send(&json!({"kind": "command", "type": "interrupt"}));
    let cancelled = main_events(&engine.until("turn_finished"));
    assert_eq!(
        cancelled,
        vec![
            json!({"type": "error", "message": "Sign-in cancelled."}),
            json!({"type": "turn_finished"}),
        ]
    );
    assert_eq!(engine.next_of("status")["signed_in"], false);

    // The device code for the sign-in screen, then the approval.
    engine.slash("login");
    let code = engine.next_of("sign_in_code");
    assert_eq!(code["url"], format!("{}/device", gateway.origin));
    assert_eq!(
        code["url_complete"],
        format!("{}/device?code={USER_CODE}", gateway.origin)
    );
    assert_eq!(code["user_code"], USER_CODE);
    assert_eq!(code["expires_in_secs"], 600);
    assert_eq!(code["browser_opened"], false);
    let signing_in = engine.until("turn_finished");
    assert_eq!(
        main_events(&signing_in),
        vec![
            json!({"type": "session_starting"}),
            json!({"type": "turn_finished"}),
        ],
        "{signing_in:?}"
    );
    assert!(
        notices(&signing_in)
            .iter()
            .any(|notice| notice.starts_with("Signed in to Zero-X (plan pro)")),
        "{signing_in:?}"
    );
    let status = engine.next_of("status");
    assert_eq!(status["signed_in"], true, "{status}");
    assert_eq!(status["sign_in_required"], false, "{status}");
    assert_eq!(status["account"]["email"], EMAIL);
    assert_eq!(status["account"]["plan"], "pro");
    assert!(
        status["account"]["source"]
            .as_str()
            .is_some_and(|source| source.starts_with("stored login")),
        "{status}"
    );
    // The gateway's default model, shown without the `openai/` routing prefix.
    assert_eq!(
        status["model"], "auto",
        "the session runs on the gateway: {status}"
    );
    assert!(
        status["session_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "{status}"
    );

    // The gateway's models, with availability for this plan.
    engine.send(&json!({"kind": "command", "type": "list_models"}));
    let models = engine.next_of("models");
    assert_eq!(models["models"][0]["id"], "zx-fast", "{models}");
    assert_eq!(models["models"][0]["available"], true);
    assert_eq!(models["models"][1]["id"], "zx-big");
    assert_eq!(models["models"][1]["available"], false);
    assert_eq!(models["models"][1]["unavailable_reason"], "Pro plan only");

    // Signing out says so on the event stream, never as stray stdout.
    engine.slash("logout");
    let signing_out = engine.until("turn_finished");
    assert_eq!(
        main_events(&signing_out),
        vec![json!({"type": "turn_finished"})],
        "{signing_out:?}"
    );
    assert!(
        notices(&signing_out)
            .iter()
            .any(|notice| notice == "Signed out of Zero-X."),
        "{signing_out:?}"
    );
    let status = engine.next_of("status");
    assert_eq!(status["signed_in"], false, "{status}");
    assert_eq!(
        status["sign_in_required"], true,
        "the gateway session needs a login again: {status}"
    );
    assert_eq!(status["account"], Value::Null);

    engine.end_input();
    let requests = gateway.requests();
    for expected in [
        "POST /auth/device/code",
        "POST /auth/device/token",
        "GET /auth/session (signed)",
        "GET /v1/models (signed)",
        "DELETE /auth/session (signed)",
    ] {
        assert!(
            requests.iter().any(|request| request == expected),
            "{expected}: {requests:?}"
        );
    }
    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn a_launch_that_fails_reports_one_error_event() {
    let (workspace, home) = workspace_and_home("bad-launch");
    let mut command = Command::new(env!("CARGO_BIN_EXE_neuron"));
    command
        .current_dir(&workspace)
        .env_clear()
        .env("NEURON_CONFIG_HOME", home.join("config"))
        .env("NEURON_HOME", home.join("neuron"))
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
