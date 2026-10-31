//! `neuron engine --stdio` end to end: the built binary driven over stdin, every stdout
//! line a JSON event. Turns run against the mock Anthropic service; the sign-in runs
//! against a fake Zero-X gateway, with a credential store of the test's own.

use std::collections::VecDeque;
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
            // The scenarios script each model request; Neuron's completion check adds a
            // round after edits and is covered by the runtime's tests.
            .env("NEURON_COMPLETION_CHECK", "0")
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
                    matches!(
                        event["type"].as_str(),
                        Some("status" | "notice" | "account")
                    ),
                    "nothing else after the input ended: {event}"
                ),
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => panic!("stdout did not close"),
            }
        }
    }
}

/// The events without notices, footer updates and account updates (an account read in
/// the background may arrive at any point).
fn main_events(events: &[Value]) -> Vec<Value> {
    events
        .iter()
        .filter(|event| {
            !matches!(
                event["type"].as_str(),
                Some("notice" | "status" | "account")
            )
        })
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
    // No Zero-X login: the account says so, and nothing is read.
    assert_eq!(
        engine.next(),
        json!({"type": "account", "signed_in": false, "account": null})
    );

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

/// SSE bodies for the next model calls, in order; after them, a refusal.
type Replies = Arc<Mutex<VecDeque<String>>>;

impl FakeGateway {
    fn start() -> Self {
        Self::with_replies(Vec::new())
    }

    /// Model calls get `replies`, one each, then the recorded refusal.
    fn with_replies(replies: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a local port");
        let origin = format!("http://{}", listener.local_addr().expect("address"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        let page = format!("{origin}/device");
        let replies: Replies = Arc::new(Mutex::new(replies.into()));
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let seen = Arc::clone(&seen);
                let page = page.clone();
                let replies = Arc::clone(&replies);
                thread::spawn(move || answer(stream, &seen, &page, &replies));
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

fn answer(
    stream: TcpStream,
    seen: &Mutex<Vec<String>>,
    page: &str,
    replies: &Mutex<VecDeque<String>>,
) {
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

    // A model call: the next scripted reply, else the Claude adapter's recorded stream of
    // a refusal mid-answer.
    if (method.as_str(), path.as_str()) == ("POST", "/v1/chat/completions") && signed {
        let scripted = replies.lock().expect("replies").pop_front();
        let body = scripted.as_deref().unwrap_or(include_str!(
            "../../api/tests/fixtures/gateway_claude/refusal_mid_text.sse"
        ));
        let mut stream = stream;
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        return;
    }

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

/// A refusal reaches the UI as a notice of the `refusal` kind after the text streamed
/// before it, not as assistant text and not as an error: the turn finishes normally.
#[test]
fn stdio_engine_reports_a_refusal_as_a_refusal_notice() {
    let gateway = FakeGateway::start();
    let (workspace, home) = workspace_and_home("refusal");
    let mut engine = StdioEngine::start(
        &workspace,
        &home,
        &[
            ("NEURON_API_BASE", &gateway.base),
            ("NEURON_TOKEN", SESSION),
        ],
        &[],
        Some(SESSION),
    );
    assert_eq!(engine.next()["type"], "ready");
    let status = engine.next_of("status");
    assert_eq!(status["signed_in"], true, "{status}");

    engine.send(&submit("how do I pick this lock?"));
    let turn = engine.until("turn_finished");

    let text = turn
        .iter()
        .filter(|event| event["type"] == "text_delta")
        .filter_map(|event| event["text"].as_str())
        .collect::<String>();
    assert_eq!(
        text,
        "Here is how the lock mechanism works. First, the pins"
    );
    let refusals = turn
        .iter()
        .filter(|event| event["type"] == "notice" && event["kind"] == "refusal")
        .collect::<Vec<_>>();
    assert_eq!(refusals.len(), 1, "{turn:?}");
    // The gateway's note names the model; the line the UI shows does not.
    assert_eq!(
        refusals[0]["message"],
        "The model declined to continue this request (safety category: cyber). Rephrase it or choose another model."
    );
    let refusal_at = turn
        .iter()
        .position(|event| event["kind"] == "refusal")
        .expect("refusal");
    let last_text_at = turn
        .iter()
        .rposition(|event| event["type"] == "text_delta")
        .expect("text");
    assert!(
        last_text_at < refusal_at,
        "the notice comes after the text: {turn:?}"
    );
    assert!(
        turn.iter().all(|event| event["type"] != "error"),
        "a refusal is not a failure: {turn:?}"
    );
    // Plain notices keep their old shape: no `kind` field.
    assert!(
        turn.iter()
            .filter(|event| event["type"] == "notice" && event["kind"] != "refusal")
            .all(|event| event.get("kind").is_none()),
        "{turn:?}"
    );
    assert!(gateway
        .requests()
        .iter()
        .any(|request| request == "POST /v1/chat/completions (signed)"));

    engine.end_input();
    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_dir_all(&home);
}

fn sse(chunk: &Value) -> String {
    format!("data: {chunk}\n\n")
}

/// A model writing `site/style.css` with `write_file`, in the pieces the gateway forwards.
fn write_file_reply() -> String {
    let call = |delta: Value| {
        sse(&json!({
            "id": "chatcmpl-1", "object": "chat.completion.chunk", "model": "auto",
            "choices": [{"index": 0, "delta": {"tool_calls": [delta]}, "finish_reason": null}]
        }))
    };
    [
        call(json!({"index": 0, "id": "call_css", "type": "function",
                    "function": {"name": "write_file", "arguments": ""}})),
        call(json!({"index": 0, "function": {"arguments": "{\"path\":\"site/style.css\","}})),
        call(json!({"index": 0, "function": {"arguments": "\"content\":\"body {\\n  margin: 0;\\n"}})),
        call(json!({"index": 0, "function": {"arguments": "}\\n\"}"}})),
        sse(&json!({
            "id": "chatcmpl-1", "object": "chat.completion.chunk", "model": "auto",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
            "usage": {"prompt_tokens": 900, "completion_tokens": 40, "total_tokens": 940}
        })),
        "data: [DONE]\n\n".to_string(),
    ]
    .concat()
}

/// A model answering `text` after the tool's result.
fn text_reply(text: &str) -> String {
    [
        sse(&json!({
            "id": "chatcmpl-2", "object": "chat.completion.chunk", "model": "auto",
            "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": null}]
        })),
        sse(&json!({
            "id": "chatcmpl-2", "object": "chat.completion.chunk", "model": "auto",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1000, "completion_tokens": 5, "total_tokens": 1005}
        })),
        "data: [DONE]\n\n".to_string(),
    ]
    .concat()
}

/// A UI that declared `live_progress` sees a tool call while the model writes it: its
/// start, its progress with the path, then the call, its result, each request's tokens,
/// and the turn's usage.
#[test]
#[allow(clippy::too_many_lines)]
fn stdio_engine_shows_a_tool_call_while_the_model_writes_it() {
    let gateway = FakeGateway::with_replies(vec![write_file_reply(), text_reply("Written.")]);
    let (workspace, home) = workspace_and_home("live-progress");
    let mut engine = StdioEngine::start(
        &workspace,
        &home,
        &[
            ("NEURON_API_BASE", &gateway.base),
            ("NEURON_TOKEN", SESSION),
        ],
        &["--permission-mode", "workspace-write"],
        Some(SESSION),
    );
    let ready = engine.next();
    assert_eq!(ready["type"], "ready", "{ready}");
    let features = ready["features"].as_array().expect("features");
    for feature in ["capabilities", "live_progress"] {
        assert!(features.iter().any(|listed| listed == feature), "{ready}");
    }
    assert_eq!(engine.next_of("status")["signed_in"], true);

    engine.send(&json!({
        "kind": "command", "type": "capabilities",
        "features": ["live_progress"], "client": "terminal",
    }));
    engine.send(&submit("write the stylesheet"));
    let turn = main_events(&engine.until("turn_finished"));
    let types = turn
        .iter()
        .map(|event| event["type"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    let first = |kind: &str| {
        types
            .iter()
            .position(|listed| *listed == kind)
            .unwrap_or_else(|| panic!("no {kind}: {turn:?}"))
    };
    let last = |kind: &str| {
        types
            .iter()
            .rposition(|listed| *listed == kind)
            .unwrap_or_else(|| panic!("no {kind}: {turn:?}"))
    };

    let input_started = first("tool_input_started");
    assert_eq!(turn[input_started]["id"], "call_css");
    assert_eq!(turn[input_started]["name"], "write_file");
    let with_path = turn
        .iter()
        .position(|event| {
            event["type"] == "tool_input_progress" && event["target"] == "site/style.css"
        })
        .unwrap_or_else(|| panic!("no progress with the path: {turn:?}"));
    let written = &turn[last("tool_input_progress")];
    assert_eq!(written["id"], "call_css");
    assert_eq!(written["lines"], 3, "{written}");
    assert_eq!(written["preview"], "body {\n  margin: 0;\n}", "{written}");
    let call_started = first("tool_call_started");
    assert_eq!(turn[call_started]["id"], "call_css");
    let result = first("tool_result");
    assert_eq!(turn[result]["id"], "call_css");
    assert_eq!(turn[result]["is_error"], false, "{turn:?}");
    assert!(
        input_started < with_path && with_path < call_started && call_started < result,
        "{types:?}"
    );

    // Each model request's tokens as reported, then the turn's totals.
    let requests = turn
        .iter()
        .filter(|event| event["type"] == "request_usage")
        .map(|event| event["usage"].clone())
        .collect::<Vec<_>>();
    assert_eq!(requests, vec![counts(900, 40), counts(1000, 5)], "{turn:?}");
    assert!(first("request_usage") > call_started, "{types:?}");
    assert!(first("output_progress") < result, "{types:?}");
    let usage = last("usage");
    assert!(last("request_usage") < usage, "{types:?}");
    assert_eq!(turn[usage]["turn"], counts(1900, 45), "{turn:?}");
    assert_eq!(types.last(), Some(&"turn_finished"));
    assert_eq!(
        fs::read_to_string(workspace.join("site").join("style.css")).expect("the file"),
        "body {\n  margin: 0;\n}\n"
    );

    engine.end_input();
    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_dir_all(&home);
}

/// `git` in `cwd` (the test's own environment); its stdout.
fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The types of `events`, in order.
fn types(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| event["type"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Source control from the UI, in a repository whose stylesheet the agent rewrites: the
/// change shows during the turn (with the footer's git counts), `/diff` shows it against
/// `HEAD`, `/commit` drafts a message with the model and commits nothing, the `commit`
/// command commits, `/rewind` lists the prompt's checkpoint and the `rewind` command
/// previews, then puts the stylesheet back and takes the prompt out. A UI that did not
/// declare `git_views` gets notices instead.
#[test]
#[allow(clippy::too_many_lines)]
fn stdio_engine_tracks_changes_diffs_commits_and_rewinds() {
    let gateway = FakeGateway::with_replies(vec![
        write_file_reply(),
        text_reply("Written."),
        text_reply("```\nfeat(site): drop the page margin\n\nThe page starts at the edge.\n```"),
    ]);
    let (workspace, home) = workspace_and_home("source-control");
    let style = workspace.join("site").join("style.css");
    fs::create_dir_all(style.parent().expect("site")).expect("site folder");
    fs::write(&style, "body {\n  margin: 8px;\n}\n").expect("stylesheet");
    git(&workspace, &["init", "-q", "-b", "main"]);
    git(&workspace, &["config", "user.email", "tests@example.com"]);
    git(&workspace, &["config", "user.name", "Neuron Tests"]);
    git(&workspace, &["config", "commit.gpgsign", "false"]);
    git(&workspace, &["config", "core.autocrlf", "false"]);
    git(&workspace, &["add", "-A"]);
    git(&workspace, &["commit", "-q", "-m", "Add the site"]);
    let mut engine = StdioEngine::start(
        &workspace,
        &home,
        &[
            ("NEURON_API_BASE", &gateway.base),
            ("NEURON_TOKEN", SESSION),
        ],
        &["--permission-mode", "workspace-write"],
        Some(SESSION),
    );
    let ready = engine.next();
    let features = ready["features"].as_array().expect("features");
    for feature in ["git_views", "checkpoints"] {
        assert!(features.iter().any(|listed| listed == feature), "{ready}");
    }
    let status = engine.next_of("status");
    assert_eq!(status["git"]["branch"], "main", "{status}");
    assert_eq!(status["git"]["changed"], 0, "{status}");
    engine.send(&json!({
        "kind": "command", "type": "capabilities",
        "features": ["live_progress", "git_views"], "client": "terminal",
    }));

    // The write shows in the changes once its result is in, and the footer counts it.
    engine.send(&submit("write the stylesheet"));
    let turn = engine.until("turn_finished");
    let restyled =
        json!([{"path": "site/style.css", "status": "modified", "added": 1, "removed": 1}]);
    let kinds = types(&turn);
    let result = kinds
        .iter()
        .position(|kind| kind == "tool_result")
        .expect("a result");
    let changes = turn
        .iter()
        .position(|event| event["type"] == "changes" && event["files"] == restyled)
        .unwrap_or_else(|| panic!("the change: {turn:?}"));
    assert!(result < changes, "{kinds:?}");
    let footer = turn[changes..]
        .iter()
        .find(|event| event["type"] == "status")
        .unwrap_or_else(|| panic!("the footer during the turn: {turn:?}"));
    assert_eq!(footer["git"]["unstaged"], 1, "{footer}");
    engine.next_of("status");

    // `/diff`: against HEAD, with the hunk.
    engine.slash("diff");
    let diff = main_events(&engine.until("turn_finished"));
    assert_eq!(types(&diff), ["diff", "turn_finished"], "{diff:?}");
    assert_eq!(
        diff[0],
        json!({
            "type": "diff",
            "title": "Changes against HEAD on main",
            "files": [{
                "path": "site/style.css", "status": "modified", "added": 1, "removed": 1,
                "patch": "@@ -1,3 +1,3 @@\n body {\n-  margin: 8px;\n+  margin: 0;\n }\n",
                "binary": false,
            }],
            "truncated": false,
        })
    );
    engine.next_of("status");

    // `/commit`: the model's draft, and nothing committed yet.
    engine.slash("commit");
    let proposal = main_events(&engine.until("turn_finished"));
    assert_eq!(
        types(&proposal),
        ["commit_proposal", "turn_finished"],
        "{proposal:?}"
    );
    assert_eq!(
        proposal[0],
        json!({
            "type": "commit_proposal",
            "message": "feat(site): drop the page margin\n\nThe page starts at the edge.",
            "branch": "main",
            "files": restyled,
            "stage_all": true,
        })
    );
    assert_eq!(
        git(&workspace, &["rev-list", "--count", "HEAD"]).trim(),
        "1"
    );
    engine.next_of("status");

    // The `commit` command commits, with the message as given.
    engine.send(&json!({
        "kind": "command", "type": "commit",
        "message": "Drop the page margin\n\nThe page starts at the edge.",
    }));
    let committed = engine.until("turn_finished");
    let hash = git(&workspace, &["rev-parse", "--short", "HEAD"]);
    assert!(
        notices(&committed).contains(&format!(
            "Committed {} on main: Drop the page margin (1 file)",
            hash.trim()
        )),
        "{committed:?}"
    );
    let footer = committed
        .iter()
        .find(|event| event["type"] == "status")
        .unwrap_or_else(|| panic!("the footer after the commit: {committed:?}"));
    assert_eq!(footer["git"]["changed"], 0, "{footer}");
    assert_eq!(
        types(&main_events(&committed)),
        ["changes", "turn_finished"]
    );
    assert_eq!(
        git(&workspace, &["log", "-1", "--format=%B"]),
        "Drop the page margin\n\nThe page starts at the edge.\n\n"
    );
    engine.next_of("status");

    // `/rewind`: the prompt's checkpoint.
    engine.slash("rewind");
    let listed = main_events(&engine.until("turn_finished"));
    assert_eq!(
        types(&listed),
        ["checkpoints", "turn_finished"],
        "{listed:?}"
    );
    let items = listed[0]["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "{listed:?}");
    assert_eq!(items[0]["prompt_preview"], "write the stylesheet");
    assert_eq!(items[0]["files"], json!(["site/style.css"]));
    let checkpoint = items[0]["id"].as_u64().expect("an id");
    engine.next_of("status");

    // A dry run, then the rewind itself.
    engine.send(&json!({
        "kind": "command", "type": "rewind",
        "checkpoint": checkpoint, "mode": "code_only", "dry_run": true,
    }));
    let preview = main_events(&engine.until("turn_finished"));
    assert_eq!(types(&preview), ["rewound", "turn_finished"], "{preview:?}");
    assert_eq!(preview[0]["dry_run"], true);
    assert_eq!(preview[0]["restored"], json!(["site/style.css"]));
    assert_eq!(preview[0]["removed_turns"], 0);
    assert_eq!(preview[0]["prompt"], Value::Null);
    assert!(fs::read_to_string(&style)
        .expect("style")
        .contains("margin: 0;"));
    engine.next_of("status");

    engine.send(&json!({
        "kind": "command", "type": "rewind",
        "checkpoint": checkpoint, "mode": "code_and_chat",
    }));
    let rewound = engine.until("turn_finished");
    assert_eq!(
        types(&main_events(&rewound)),
        ["rewound", "changes", "turn_finished"],
        "{rewound:?}"
    );
    let report = &main_events(&rewound)[0];
    assert_eq!(report["mode"], "code_and_chat");
    assert_eq!(report["dry_run"], false);
    assert_eq!(report["restored"], json!(["site/style.css"]));
    assert_eq!(report["removed_turns"], 1);
    assert_eq!(report["prompt"], "write the stylesheet");
    assert_eq!(main_events(&rewound)[1]["files"], json!([]));
    assert_eq!(
        fs::read_to_string(&style).expect("style"),
        "body {\n  margin: 8px;\n}\n"
    );
    let footer = rewound
        .iter()
        .find(|event| event["type"] == "status")
        .unwrap_or_else(|| panic!("the footer after the rewind: {rewound:?}"));
    assert_eq!(footer["git"]["unstaged"], 1, "{footer}");
    engine.next_of("status");

    // Without git views: notices only.
    engine.send(&json!({
        "kind": "command", "type": "capabilities",
        "features": ["live_progress"], "client": "terminal",
    }));
    engine.slash("diff");
    let diff = engine.until("turn_finished");
    assert_eq!(types(&main_events(&diff)), ["turn_finished"], "{diff:?}");
    assert!(
        notices(&diff).iter().any(|notice| notice
            .starts_with("Changes against HEAD on main\n1 file, +1 -1")
            && notice.contains("+  margin: 8px;")),
        "{diff:?}"
    );
    engine.next_of("status");
    engine.slash("rewind");
    let listed = engine.until("turn_finished");
    assert_eq!(
        types(&main_events(&listed)),
        ["turn_finished"],
        "{listed:?}"
    );
    assert!(
        notices(&listed)
            .iter()
            .any(|notice| notice.starts_with("No checkpoints yet")),
        "{listed:?}"
    );
    engine.next_of("status");
    engine.send(&json!({
        "kind": "command", "type": "slash", "command": "commit",
        "args": "Restore the page margin",
    }));
    let committed = engine.until("turn_finished");
    assert_eq!(
        types(&main_events(&committed)),
        ["turn_finished"],
        "{committed:?}"
    );
    assert!(
        notices(&committed)
            .iter()
            .any(|notice| notice.ends_with(": Restore the page margin (1 file)")),
        "{committed:?}"
    );
    assert_eq!(
        git(&workspace, &["rev-list", "--count", "HEAD"]).trim(),
        "3"
    );
    assert_eq!(
        gateway
            .requests()
            .iter()
            .filter(|request| *request == "POST /v1/chat/completions (signed)")
            .count(),
        3,
        "two for the turn, one for the draft"
    );

    engine.end_input();
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
