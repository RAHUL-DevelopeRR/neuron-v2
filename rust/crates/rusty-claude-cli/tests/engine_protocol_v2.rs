//! Engine protocol v2 end to end: the built `neuron engine --stdio` signed in to a fake
//! Zero-X gateway, driven over JSON lines. Covers the features `ready` lists and every
//! v2 command: the account (stored profile, then `/api/account`, then quota headers), typed
//! gateway errors, image support per model, report events, sessions of every project and
//! the trust preview.
//!
//! Every run uses a throwaway home, Neuron home and file credential store.

use std::fmt::Write as _;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const SESSION_TOKEN: &str = "ses_v2protocol0123456789abcdefghij";
const EVENT_TIMEOUT: Duration = Duration::from_secs(60);
const MODEL: &str = "gemini/gemini-2.5-flash";

/// A gateway answering the routes the engine uses. The first chat request is refused
/// with the daily quota used up; later ones answer with text and quota headers.
struct FakeGateway {
    base: String,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FakeGateway {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let base = format!("http://{}/v1", listener.local_addr().expect("addr"));
        let stop = Arc::new(AtomicBool::new(false));
        let chats = Arc::new(AtomicUsize::new(0));
        let handle = {
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => answer(stream, &chats),
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("fake gateway accept failed: {error}"),
                    }
                }
            })
        };
        Self {
            base,
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for FakeGateway {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn answer(stream: TcpStream, chats: &AtomicUsize) {
    stream.set_nonblocking(false).expect("blocking");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut length = 0;
    loop {
        line.clear();
        reader.read_line(&mut line).expect("header");
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().expect("length");
            }
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).expect("body");

    let json_reply = |status: &str, value: &Value| {
        (
            status.to_string(),
            "application/json",
            Vec::new(),
            value.to_string(),
        )
    };
    let (status, content_type, headers, body) = match (method.as_str(), path.as_str()) {
        ("GET", "/v1/models") => json_reply("200 OK", &catalog()),
        ("GET", "/api/account") => json_reply("200 OK", &account()),
        ("GET", "/auth/session") => json_reply(
            "200 OK",
            &json!({"plan": "pro", "email": "dev@example.com", "expires_at": "2099-01-01T00:00:00Z"}),
        ),
        ("POST", "/v1/chat/completions") if chats.fetch_add(1, Ordering::SeqCst) == 0 => {
            json_reply(
                "429 Too Many Requests",
                &json!({"error": {
                    "message": "Daily request limit reached for the Pro plan",
                    "type": "insufficient_quota",
                    "code": "insufficient_quota",
                    "param": null,
                    "request_id": "req_v2_quota",
                    "upgrade_url": "https://zero-x.live/pricing",
                    "resets_at": "2099-01-01T00:00:00Z"
                }}),
            )
        }
        ("POST", "/v1/chat/completions") => {
            let mut stream_body = String::new();
            for event in [
                json!({"id": "t", "model": MODEL, "choices": [{"delta": {"content": "Hello from the gateway."}}]}),
                json!({"id": "t", "choices": [{"delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 10, "completion_tokens": 5}}),
            ] {
                write!(stream_body, "data: {event}\r\n\r\n").expect("write");
            }
            stream_body.push_str("data: [DONE]\n\n");
            (
                "200 OK".to_string(),
                "text/event-stream",
                vec![
                    ("x-neuron-quota-remaining-requests", "1900"),
                    ("x-neuron-quota-remaining-tokens", "9000000"),
                ],
                stream_body,
            )
        }
        _ => json_reply(
            "404 Not Found",
            &json!({"error": {"message": "not found", "code": "invalid_request"}}),
        ),
    };
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        write!(response, "{name}: {value}\r\n").expect("write");
    }
    response.push_str("\r\n");
    response.push_str(&body);
    let mut stream = stream;
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn catalog() -> Value {
    json!({
        "object": "list",
        "data": [
            {"id": "auto", "object": "model", "created": 0, "owned_by": "zero-x", "display_name": "Auto", "context_window": 131_072, "max_output_tokens": 8_192, "capabilities": {"tools": true, "streaming": true, "vision": false}, "tiers": ["free"], "available": true, "unavailable_reason": null, "pricing": null},
            {"id": MODEL, "object": "model", "created": 0, "owned_by": "gemini", "display_name": "Gemini 2.5 Flash", "context_window": 1_048_576, "max_output_tokens": 65_536, "capabilities": {"tools": true, "streaming": true, "vision": true}, "tiers": ["free", "paid"], "available": true, "unavailable_reason": null, "pricing": null}
        ]
    })
}

fn account() -> Value {
    json!({
        "user": {"id": "u1", "email": "dev@example.com", "name": "Dev Example", "image_url": null},
        "plan": {"id": "pro", "name": "Pro"},
        "limits": {"daily_requests": 2000, "daily_tokens": 10_000_000, "rpm": 30, "concurrency": 4},
        "usage_today": {"requests": 37, "tokens": 120_431, "resets_at": "2099-01-01T00:00:00Z"},
        "credits": {"balance_usd": 7.42, "monthly_grant_usd": 8},
        "subscription": null,
        "flags": {"suspended": false}
    })
}

fn unique_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("neuron-v2-{label}-{}-{nanos}", std::process::id()))
}

struct Engine {
    child: Child,
    stdin: Option<ChildStdin>,
    events: Receiver<Value>,
}

impl Engine {
    fn start(workspace: &Path, home: &Path, gateway: &FakeGateway) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_neuron"));
        command
            .current_dir(workspace)
            .env_clear()
            .env("NEURON_API_BASE", &gateway.base)
            .env("NEURON_CONFIG_HOME", home.join("config"))
            .env("NEURON_HOME", home.join("neuron"))
            .env("NEURON_CREDENTIAL_STORE", "file")
            .env("NEURON_NO_BROWSER", "1")
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("NO_COLOR", "1")
            .args(["engine", "--stdio", "--model", MODEL])
            .args(["--permission-mode", "read-only"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in ["SystemRoot", "PATH", "ComSpec"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let mut child = command.spawn().expect("neuron engine starts");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");
        thread::spawn(move || for _ in BufReader::new(stderr).lines() {});
        let (events_tx, events) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.expect("stdout is UTF-8");
                assert!(
                    !line.contains(SESSION_TOKEN),
                    "the token reached stdout: {line}"
                );
                let event: Value = serde_json::from_str(&line)
                    .unwrap_or_else(|error| panic!("only JSON events: {line:?} ({error})"));
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
        let stdin = self.stdin.as_mut().expect("stdin");
        writeln!(stdin, "{message}").expect("write");
        stdin.flush().expect("flush");
    }

    fn command(&mut self, command: &Value) {
        let mut message = command.clone();
        message["kind"] = json!("command");
        self.send(&message);
    }

    fn next(&self) -> Value {
        self.events
            .recv_timeout(EVENT_TIMEOUT)
            .expect("the engine reports in time")
    }

    /// Events up to and including the first that `last` accepts.
    fn until(&self, last: impl Fn(&Value) -> bool) -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let event = self.next();
            let done = last(&event);
            seen.push(event);
            if done {
                return seen;
            }
        }
    }

    fn until_type(&self, kind: &str) -> Vec<Value> {
        self.until(|event| event["type"] == kind)
    }

    fn end(mut self) {
        drop(self.stdin.take());
        let status = self.child.wait().expect("exit");
        assert!(status.success(), "exit status {status:?}");
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn engine_speaks_protocol_v2_against_a_zero_x_gateway() {
    let gateway = FakeGateway::start();
    let workspace = unique_dir("workspace");
    let home = unique_dir("home");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::create_dir_all(home.join("config")).expect("config");
    fs::create_dir_all(home.join("neuron")).expect("neuron home");
    fs::write(
        home.join("config").join("settings.json"),
        r#"{"defaultMcpServers":false}"#,
    )
    .expect("settings");
    // A stored login, with the profile an earlier session kept.
    fs::write(
        home.join("neuron").join("credentials.json"),
        json!({
            "version": 1, "token": SESSION_TOKEN, "expires_at": 4_070_908_800_u64,
            "plan": "Pro", "email": "stored@example.com", "name": "Stored Name", "saved_at": 0
        })
        .to_string(),
    )
    .expect("credentials");
    // What trusting the workspace would turn on.
    fs::create_dir_all(workspace.join(".neuron")).expect("workspace settings dir");
    fs::write(
        workspace.join(".neuron").join("settings.json"),
        r#"{"hooks":{"PreToolUse":["npm run lint"]},"permissions":{"allow":["bash(npm test)"]}}"#,
    )
    .expect("workspace settings");

    let mut engine = Engine::start(&workspace, &home, &gateway);

    // Ready lists every v2 feature.
    let ready = engine.next();
    assert_eq!(ready["type"], "ready", "{ready}");
    let features: Vec<&str> = ready["features"]
        .as_array()
        .expect("features")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for feature in [
        "reports",
        "account",
        "error_details",
        "session_scope",
        "trust_preview",
        "model_images",
    ] {
        assert!(features.contains(&feature), "{feature} in {features:?}");
    }

    // The footer: signed in, and the model reads images.
    let status = engine.next();
    assert_eq!(status["type"], "status", "{status}");
    assert_eq!(status["signed_in"], true, "{status}");
    assert_eq!(status["supports_images"], true, "{status}");

    // The account: the stored profile at once, then `/api/account`.
    let stored = engine.until_type("account").pop().expect("an account");
    assert_eq!(stored["signed_in"], true);
    let read = engine
        .until(|event| event["type"] == "account" && !event["account"]["limits"].is_null())
        .pop()
        .expect("the account read");
    let account = &read["account"];
    assert_eq!(account["name"], "Dev Example", "{read}");
    assert_eq!(account["email"], "dev@example.com");
    assert_eq!(account["plan"], "pro");
    assert_eq!(account["plan_name"], "Pro");
    assert_eq!(account["limits"]["requests"], 2000);
    assert_eq!(account["usage_today"]["requests"], 37);
    assert_eq!(account["resets_at"], "2099-01-01T00:00:00Z");
    assert_eq!(account["credit_balance_cents"], 742);
    assert!(account["usage_updated_at"].is_u64());
    assert_eq!(account["usage_error"], Value::Null);
    let stored_after =
        fs::read_to_string(home.join("neuron").join("credentials.json")).expect("credentials");
    assert!(
        stored_after.contains("Dev Example"),
        "the profile is kept with the login"
    );

    engine.command(&json!({
        "type": "capabilities",
        "features": ["questions", "reports"],
        "client": "terminal",
    }));

    // Models say which read images.
    engine.command(&json!({"type": "list_models"}));
    let models = engine.until_type("models").pop().expect("models");
    let flash = models["models"]
        .as_array()
        .expect("models")
        .iter()
        .find(|model| model["id"] == MODEL)
        .expect("the flash model");
    assert_eq!(flash["supports_images"], true, "{models}");

    // The quota is used up: a typed error, never retried.
    engine.command(&json!({"type": "submit", "text": "hello", "attachments": []}));
    let failed = engine.until_type("turn_finished");
    let error = failed
        .iter()
        .find(|event| event["type"] == "error")
        .unwrap_or_else(|| panic!("an error: {failed:?}"));
    assert_eq!(error["detail"]["code"], "insufficient_quota", "{error}");
    assert_eq!(error["detail"]["status"], 429);
    assert_eq!(error["detail"]["resets_at"], "2099-01-01T00:00:00Z");
    assert_eq!(
        error["detail"]["upgrade_url"],
        "https://zero-x.live/pricing"
    );
    assert!(error["message"]
        .as_str()
        .is_some_and(|message| message.contains("Daily request limit")));

    // A turn that works: the quota headers move today's usage.
    engine.command(&json!({"type": "submit", "text": "again", "attachments": []}));
    let turn = engine.until_type("turn_finished");
    assert!(
        turn.iter().any(|event| event["type"] == "text_delta"),
        "{turn:?}"
    );
    let updated = turn
        .iter()
        .rfind(|event| event["type"] == "account")
        .unwrap_or_else(|| panic!("an account update: {turn:?}"));
    assert_eq!(updated["account"]["usage_today"]["requests"], 100);
    assert_eq!(updated["account"]["usage_today"]["tokens"], 1_000_000);

    // /usage as a report panel.
    engine.command(&json!({"type": "slash", "command": "usage", "args": ""}));
    let usage = engine.until_type("turn_finished");
    let report = usage
        .iter()
        .find(|event| event["type"] == "report")
        .unwrap_or_else(|| panic!("a report: {usage:?}"));
    assert_eq!(report["command"], "usage");
    assert!(report["sections"][0]["markdown"]
        .as_str()
        .is_some_and(|text| text.contains("| Requests | 37 | 2,000 |")));
    assert_eq!(report["links"][0]["url"], "https://zero-x.live/pricing");

    // Every project's sessions: this one, in this workspace, recorded by the UI's name.
    engine.command(&json!({"type": "list_sessions", "scope": "all"}));
    let sessions = engine.until_type("sessions").pop().expect("sessions");
    assert_eq!(sessions["scope"], "all");
    let current = sessions["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|session| session["current"] == true)
        .unwrap_or_else(|| panic!("the current session: {sessions}"));
    // The engine reports its working directory, which is canonical on macOS
    // (`/private/var/...` for the test's `/var/...`).
    assert_eq!(
        current["project"]
            .as_str()
            .map(|project| fs::canonicalize(project).ok()),
        Some(fs::canonicalize(&workspace).ok())
    );
    assert_eq!(current["run_status"], "idle");
    assert_eq!(current["client"], "terminal");

    // The trust preview: listed, and nothing started.
    engine.command(&json!({"type": "trust_preview"}));
    let preview = engine.until_type("trust_preview").pop().expect("a preview");
    assert_eq!(preview["trusted"], false);
    assert_eq!(preview["hooks"][0]["command"], "npm run lint");
    assert_eq!(preview["permission_rules"][0]["rule"], "bash(npm test)");

    // refresh_account reads the account again.
    engine.command(&json!({"type": "refresh_account"}));
    let refreshed = engine.until_type("account").pop().expect("an account");
    assert_eq!(refreshed["account"]["name"], "Dev Example");

    engine.end();
    drop(gateway);
    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_dir_all(&home);
}
