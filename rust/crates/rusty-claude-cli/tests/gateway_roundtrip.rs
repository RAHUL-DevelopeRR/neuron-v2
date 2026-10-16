//! End-to-end checks of the `neuron` binary against a fake Zero-X gateway:
//! catalog fetch, chat round trips, actionable errors and retry policy,
//! `neuron usage`, the `neuron auth` lifecycle and provider precedence.
//!
//! Every run uses a throwaway HOME / `NEURON_HOME` and the file credential
//! store, so the developer's real login and keychain are never touched.

use std::fmt::Write as _;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const SESSION_TOKEN: &str = "ses_012345678901234567890123456789";
const STORED_TOKEN: &str = "ses_storedlogin0123456789abcdefgh";

#[derive(Debug, Clone)]
struct Recorded {
    method: String,
    path: String,
    authorization: Option<String>,
    body: String,
}

impl Recorded {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap()
    }
}

struct Reply {
    status: &'static str,
    content_type: &'static str,
    headers: Vec<(&'static str, String)>,
    body: String,
}

impl Reply {
    fn json(status: &'static str, body: &serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            headers: Vec::new(),
            body: body.to_string(),
        }
    }

    fn sse(events: &[serde_json::Value]) -> Self {
        let mut body = String::new();
        for event in events {
            write!(body, "data: {event}\r\n\r\n").unwrap();
        }
        body.push_str("data: [DONE]\n\n");
        Self {
            status: "200 OK",
            content_type: "text/event-stream",
            headers: Vec::new(),
            body,
        }
    }

    fn with_header(mut self, name: &'static str, value: &str) -> Self {
        self.headers.push((name, value.to_string()));
        self
    }
}

type Responder = dyn Fn(&Recorded, &[Recorded]) -> Reply + Send + Sync;

struct FakeGateway {
    base: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FakeGateway {
    /// `responder` sees the request and everything recorded before it.
    fn spawn(responder: impl Fn(&Recorded, &[Recorded]) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let responder: Arc<Responder> = Arc::new(responder);
        let handle = {
            let requests = Arc::clone(&requests);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => serve(stream, &requests, responder.as_ref()),
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
            requests,
            stop,
            handle: Some(handle),
        }
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }

    fn count(&self, method: &str, path: &str) -> usize {
        self.requests()
            .iter()
            .filter(|request| request.method == method && request.path == path)
            .count()
    }

    fn last_chat(&self) -> Recorded {
        self.requests()
            .into_iter()
            .rfind(|request| request.path == "/v1/chat/completions")
            .expect("a chat request")
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

fn serve(stream: TcpStream, requests: &Mutex<Vec<Recorded>>, responder: &Responder) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut length = 0;
    let mut authorization = None;
    loop {
        line.clear();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" || line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').unwrap();
        let value = value.trim().to_string();
        match name.to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse().unwrap(),
            "authorization" => authorization = Some(value),
            _ => {}
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).unwrap();
    let recorded = Recorded {
        method,
        path,
        authorization,
        body: String::from_utf8(body).unwrap(),
    };
    let reply = {
        let mut log = requests.lock().unwrap();
        let reply = responder(&recorded, &log);
        log.push(recorded);
        reply
    };
    let mut response = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reply.content_type,
        reply.body.len()
    );
    for (name, value) in &reply.headers {
        write!(response, "{name}: {value}\r\n").unwrap();
    }
    response.push_str("\r\n");
    response.push_str(&reply.body);
    let mut stream = stream;
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn catalog() -> serde_json::Value {
    serde_json::json!({
        "object": "list",
        "data": [
            {"id": "auto", "object": "model", "created": 0, "owned_by": "zero-x", "display_name": "Auto", "context_window": null, "max_output_tokens": null, "capabilities": {"tools": true, "streaming": true}, "tiers": ["free"], "available": true, "unavailable_reason": null, "pricing": null},
            {"id": "gemini/gemini-2.5-flash", "object": "model", "created": 0, "owned_by": "gemini", "display_name": "Gemini 2.5 Flash", "context_window": 1_048_576, "max_output_tokens": 65_536, "capabilities": {"tools": true, "streaming": true}, "tiers": ["free", "paid"], "available": true, "unavailable_reason": null, "pricing": null}
        ]
    })
}

fn account() -> serde_json::Value {
    serde_json::json!({
        "user": {"id": "u1", "email": "dev@example.com", "name": "Dev", "image_url": null},
        "plan": {"id": "free", "name": "Free"},
        "limits": {"daily_requests": 100, "daily_tokens": 500_000, "rpm": 10, "concurrency": 2},
        "usage_today": {"requests": 42, "tokens": 12_345, "resets_at": "2099-01-01T00:00:00Z"},
        "credits": {"balance_usd": 0, "monthly_grant_usd": 0},
        "subscription": null,
        "flags": {"suspended": false}
    })
}

fn text_reply(text: &str) -> Reply {
    Reply::sse(&[
        serde_json::json!({"id":"t","model":"auto","choices":[{"delta":{"content":text}}]}),
        serde_json::json!({"id":"t","choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5}}),
    ])
}

/// Default account routes; `chat` answers `POST /v1/chat/completions` and
/// receives the zero-based index of that chat request.
fn gateway_with_chat(
    chat: impl Fn(&Recorded, usize) -> Reply + Send + Sync + 'static,
) -> FakeGateway {
    FakeGateway::spawn(move |request, earlier| {
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/v1/models") => Reply::json("200 OK", &catalog()),
            ("GET", "/api/account") => Reply::json("200 OK", &account()),
            ("GET", "/auth/session") => Reply::json(
                "200 OK",
                &serde_json::json!({"plan": "free", "email": "dev@example.com", "expires_at": "2099-01-01T00:00:00Z"}),
            ),
            ("DELETE", "/auth/session") => {
                Reply::json("200 OK", &serde_json::json!({"status": "signed_out"}))
            }
            ("POST", "/v1/chat/completions") => {
                let index = earlier
                    .iter()
                    .filter(|earlier| earlier.path == "/v1/chat/completions")
                    .count();
                chat(request, index)
            }
            _ => Reply::json(
                "404 Not Found",
                &serde_json::json!({"error": {"message": "not found", "code": "invalid_request"}}),
            ),
        }
    })
}

struct Workspace {
    root: PathBuf,
}

impl Workspace {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "neuron-gateway-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        for dir in ["home", "config", "neuron-home"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(
            root.join("config/settings.json"),
            r#"{"defaultMcpServers":false}"#,
        )
        .unwrap();
        Self { root }
    }

    fn neuron_home(&self) -> PathBuf {
        self.root.join("neuron-home")
    }

    fn command(&self, gateway: &FakeGateway) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_neuron"));
        command
            .env_clear()
            .current_dir(&self.root)
            .env("NEURON_API_BASE", &gateway.base)
            .env("HOME", self.root.join("home"))
            .env("USERPROFILE", self.root.join("home"))
            .env("NEURON_HOME", self.neuron_home())
            .env("NEURON_CREDENTIAL_STORE", "file")
            .env("CLAW_CONFIG_HOME", self.root.join("config"))
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in ["SystemRoot", "PATH", "ComSpec"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
    }

    fn store_login(&self, token: &str) {
        fs::write(
            self.neuron_home().join("credentials.json"),
            serde_json::json!({"version": 1, "token": token, "expires_at": 4_070_908_800_u64, "plan": "free", "saved_at": 0}).to_string(),
        )
        .unwrap();
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn run(mut command: Command, stdin: Option<&str>) -> Output {
    if stdin.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn().unwrap();
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "neuron timed out; stdout: {}; stderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn prompt_args(prompt: &str) -> [&str; 7] {
    [
        "--compact",
        "--permission-mode",
        "read-only",
        "--allowedTools",
        "read_file",
        "prompt",
        prompt,
    ]
}

fn assert_bearer(request: &Recorded, token: &str) {
    assert_eq!(
        request.authorization.as_deref(),
        Some(format!("Bearer {token}").as_str()),
        "{} {} must use the expected credential",
        request.method,
        request.path
    );
}

/// File (relative to the workspace, the hook's working directory) that the
/// `PreToolUse` hook dumps its environment into. Relative and unquoted
/// because `cmd /C` mangles quoted arguments.
const HOOK_ENV_DUMP: &str = "hook-env.txt";

fn env_dump_hook() -> String {
    if cfg!(windows) {
        format!("set > {HOOK_ENV_DUMP}")
    } else {
        format!("env > {HOOK_ENV_DUMP}")
    }
}

#[test]
fn gateway_default_model_preserves_parallel_tool_calls_and_file_results() {
    let gateway = gateway_with_chat(|request, index| {
        let body = request.json();
        assert_eq!(body["model"], "auto");
        assert!(body["max_tokens"].as_u64().unwrap() <= 8192);
        if index == 0 {
            assert!(body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["function"]["name"] == "read_file"));
            Reply::sse(&[
                serde_json::json!({"id":"test","model":"auto","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_read","function":{"name":"read_file","arguments":"{\"path\":\""}},{"index":1,"id":"call_second","function":{"name":"read_file","arguments":"{\"path\":\""}}]}}]}),
                serde_json::json!({"id":"test","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"fixture.txt\"}"}},{"index":1,"function":{"arguments":"second.txt\"}"}}]}}]}),
                serde_json::json!({"id":"test","choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
            ])
        } else {
            let messages = body["messages"].as_array().unwrap();
            for (id, content) in [
                ("call_read", "gateway fixture"),
                ("call_second", "second fixture"),
            ] {
                assert!(messages.iter().any(|message| message["role"] == "tool"
                    && message["tool_call_id"] == id
                    && message["content"]
                        .as_str()
                        .unwrap_or_default()
                        .contains(content)));
            }
            text_reply("Gateway tool roundtrip complete.")
        }
    });
    let workspace = Workspace::new("roundtrip");
    let env_dump = workspace.root.join(HOOK_ENV_DUMP);
    fs::write(
        workspace.root.join("config/settings.json"),
        serde_json::json!({
            "defaultMcpServers": false,
            "hooks": {"PreToolUse": [env_dump_hook()]}
        })
        .to_string(),
    )
    .unwrap();
    fs::write(workspace.root.join("fixture.txt"), "gateway fixture\n").unwrap();
    fs::write(workspace.root.join("second.txt"), "second fixture\n").unwrap();

    let mut command = workspace.command(&gateway);
    command
        .env("NEURON_TOKEN", SESSION_TOKEN)
        .args(prompt_args("Read fixture.txt"));
    let output = run(command, None);

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "Gateway tool roundtrip complete.");
    assert_eq!(
        gateway.count("GET", "/v1/models"),
        1,
        "catalog fetched once"
    );
    assert_eq!(gateway.count("POST", "/v1/chat/completions"), 2);
    for request in gateway.requests() {
        assert_bearer(&request, SESSION_TOKEN);
    }
    // One startup line naming provider and credential source, never the secret.
    let errors = stderr(&output);
    assert!(
        errors.contains("Provider: Zero-X gateway · credential: NEURON_TOKEN"),
        "{errors}"
    );
    assert!(!errors.contains(SESSION_TOKEN), "{errors}");
    // The catalog is cached on disk for the next process.
    let cache = fs::read_to_string(workspace.neuron_home().join("catalog.json")).unwrap();
    assert!(cache.contains("gemini/gemini-2.5-flash"));
    // The credential is not exported into subprocess environments. The dump
    // does carry variables inherited from the parent, so an exported
    // OPENAI_API_KEY would show up here.
    let dumped = fs::read_to_string(&env_dump).expect("PreToolUse hook should have run");
    assert!(
        dumped
            .lines()
            .any(|line| line.to_ascii_uppercase().starts_with("NEURON_HOME=")),
        "hook environment dump should include inherited variables: {dumped}"
    );
    for line in dumped.lines() {
        let upper = line.to_ascii_uppercase();
        assert!(
            !upper.starts_with("OPENAI_API_KEY=") && !upper.starts_with("OPENAI_BASE_URL="),
            "credential leaked into hook environment: {line}"
        );
    }
}

#[test]
fn catalog_cache_avoids_refetching_within_ten_minutes() {
    let gateway = gateway_with_chat(|_, _| text_reply("cached"));
    let workspace = Workspace::new("catalog-cache");
    for _ in 0..2 {
        let mut command = workspace.command(&gateway);
        command
            .env("NEURON_TOKEN", SESSION_TOKEN)
            .args(prompt_args("hello"));
        let output = run(command, None);
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(stdout(&output).trim(), "cached");
    }
    assert_eq!(gateway.count("GET", "/v1/models"), 1);
    assert_eq!(gateway.count("POST", "/v1/chat/completions"), 2);
}

#[test]
fn insufficient_quota_is_reported_with_reset_and_upgrade_and_never_retried() {
    let gateway = gateway_with_chat(|_, _| {
        Reply::json(
            "429 Too Many Requests",
            &serde_json::json!({"error": {
                "message": "Daily request limit reached for the Free plan",
                "type": "insufficient_quota",
                "code": "insufficient_quota",
                "param": null,
                "request_id": "req_quota_e2e",
                "upgrade_url": "https://zero-x.live/pricing",
                "resets_at": "2099-01-01T00:00:00Z"
            }}),
        )
        .with_header("Retry-After", "0")
    });
    let workspace = Workspace::new("quota");
    let mut command = workspace.command(&gateway);
    command
        .env("NEURON_TOKEN", SESSION_TOKEN)
        .args(prompt_args("hello"));
    let output = run(command, None);

    assert!(!output.status.success());
    let errors = stderr(&output);
    assert!(
        errors.contains("Daily request limit reached for the Free plan"),
        "{errors}"
    );
    assert!(
        errors.contains("resets at 2099-01-01T00:00:00Z"),
        "{errors}"
    );
    assert!(
        errors.contains("upgrade: https://zero-x.live/pricing"),
        "{errors}"
    );
    assert!(errors.contains("request req_quota_e2e"), "{errors}");
    assert!(!errors.contains("retrying"), "{errors}");
    assert_eq!(gateway.count("POST", "/v1/chat/completions"), 1);
}

#[test]
fn capacity_exhausted_honours_retry_after_then_completes() {
    let gateway = gateway_with_chat(|_, index| {
        if index == 0 {
            Reply::json(
                "503 Service Unavailable",
                &serde_json::json!({"error": {
                    "message": "The shared free pool is busy",
                    "type": "server_error",
                    "code": "capacity_exhausted",
                    "retry_after_seconds": 1,
                    "upgrade_url": "https://zero-x.live/pricing"
                }}),
            )
            .with_header("Retry-After", "1")
        } else {
            text_reply("Recovered after the pool freed up.")
        }
    });
    let workspace = Workspace::new("capacity");
    let mut command = workspace.command(&gateway);
    command
        .env("NEURON_TOKEN", SESSION_TOKEN)
        .args(prompt_args("hello"));
    let started = Instant::now();
    let output = run(command, None);

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "Recovered after the pool freed up.");
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(gateway.count("POST", "/v1/chat/completions"), 2);
    let errors = stderr(&output);
    assert!(
        errors.contains("Zero-X capacity_exhausted; retrying in 1s (retry 1 of 2)"),
        "{errors}"
    );
}

#[test]
fn expired_session_answers_401_with_the_login_instruction() {
    let gateway = FakeGateway::spawn(|request, _| match request.path.as_str() {
        "/v1/models" => Reply::json("200 OK", &catalog()),
        _ => Reply::json(
            "401 Unauthorized",
            &serde_json::json!({"error": {"message": "Session expired", "type": "authentication_error", "code": "invalid_session", "request_id": "req_401"}}),
        ),
    });
    let workspace = Workspace::new("expired");
    let mut command = workspace.command(&gateway);
    command
        .env("NEURON_TOKEN", SESSION_TOKEN)
        .args(prompt_args("hello"));
    let output = run(command, None);

    assert!(!output.status.success());
    let errors = stderr(&output);
    assert!(
        errors.contains("Your Zero-X session expired or was revoked — run neuron auth login"),
        "{errors}"
    );
    assert_eq!(gateway.count("POST", "/v1/chat/completions"), 1);
}

#[test]
fn neuron_usage_renders_the_account_from_the_gateway() {
    let gateway = gateway_with_chat(|_, _| text_reply("unused"));
    let workspace = Workspace::new("usage");
    let mut command = workspace.command(&gateway);
    command
        .env("NEURON_TOKEN", SESSION_TOKEN)
        .env("NEURON_PRICING_URL", "https://example.test/plans")
        .arg("usage");
    let output = run(command, None);

    assert!(output.status.success(), "{}", stderr(&output));
    let report = stdout(&output);
    assert!(report.contains("Zero-X usage"), "{report}");
    assert!(
        report.contains("Account          dev@example.com"),
        "{report}"
    );
    assert!(report.contains("Plan             Free"), "{report}");
    assert!(report.contains("Requests today   42 / 100"), "{report}");
    assert!(
        report.contains("Tokens today     12,345 / 500,000"),
        "{report}"
    );
    assert!(
        report.contains("Resets           2099-01-01 00:00 UTC"),
        "{report}"
    );
    assert!(
        report.contains("Upgrade          https://example.test/plans"),
        "{report}"
    );
    let requests = gateway.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/api/account");
    assert_bearer(&requests[0], SESSION_TOKEN);

    let mut upgrade = workspace.command(&gateway);
    upgrade
        .env("NEURON_PRICING_URL", "https://example.test/plans")
        .args(["upgrade", "--no-browser"]);
    let output = run(upgrade, None);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("https://example.test/plans"));
}

#[test]
fn auth_login_with_token_status_and_logout_lifecycle() {
    let gateway = gateway_with_chat(|_, _| text_reply("unused"));
    let workspace = Workspace::new("auth");
    let credentials = workspace.neuron_home().join("credentials.json");

    let mut login = workspace.command(&gateway);
    login.args(["auth", "login", "--with-token"]);
    let output = run(login, Some(&format!("{SESSION_TOKEN}\n")));
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("Signed in to Zero-X (plan free)"),
        "{}",
        stderr(&output)
    );
    let stored: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&credentials).unwrap()).unwrap();
    assert_eq!(stored["token"], SESSION_TOKEN);
    assert_eq!(stored["expires_at"], 4_070_908_800_u64);
    assert_eq!(stored["plan"], "free");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&credentials).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    let mut status = workspace.command(&gateway);
    status.args(["auth", "status"]);
    let output = run(status, None);
    assert!(output.status.success(), "{}", stderr(&output));
    let report = stdout(&output);
    assert!(
        report.contains("Signed in        yes (stored login, credentials file)"),
        "{report}"
    );
    assert!(report.contains("Session          valid"), "{report}");
    assert!(
        report.contains("Session expires  2099-01-01 00:00 UTC"),
        "{report}"
    );
    assert!(report.contains("Requests today   42 / 100"), "{report}");
    assert!(!report.contains(SESSION_TOKEN));

    let mut logout = workspace.command(&gateway);
    logout.args(["auth", "logout"]);
    let output = run(logout, None);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("Signed out of Zero-X."));
    assert!(!credentials.exists());
    assert_eq!(gateway.count("DELETE", "/auth/session"), 1);

    let mut status = workspace.command(&gateway);
    status.args(["auth", "status"]);
    let output = run(status, None);
    assert!(output.status.success());
    assert!(stdout(&output).contains("Signed out of Zero-X"));
}

#[test]
fn corrupt_credentials_warn_and_logout_still_succeeds() {
    let gateway = gateway_with_chat(|_, _| text_reply("unused"));
    let workspace = Workspace::new("corrupt");
    let credentials = workspace.neuron_home().join("credentials.json");
    fs::write(&credentials, "{definitely not a credential").unwrap();

    let mut status = workspace.command(&gateway);
    status.args(["auth", "status"]);
    let output = run(status, None);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("Signed out of Zero-X"));
    assert!(stderr(&output).contains("Ignoring unreadable credential file"));

    let mut logout = workspace.command(&gateway);
    logout.args(["auth", "logout"]);
    let output = run(logout, None);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!credentials.exists());

    let mut doctor = workspace.command(&gateway);
    doctor.args(["--output-format", "json", "doctor"]);
    let output = run(doctor, None);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let auth = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "auth")
        .unwrap()
        .clone();
    assert_eq!(auth["gateway_session"], "signed_out");
}

#[test]
fn stored_login_wins_over_ambient_openai_key_unless_provider_is_explicit() {
    let gateway = gateway_with_chat(|_, _| text_reply("answered"));
    let workspace = Workspace::new("precedence");
    workspace.store_login(STORED_TOKEN);

    let mut stored = workspace.command(&gateway);
    stored
        .env("OPENAI_API_KEY", "sk-ambient-openai-key")
        .args(prompt_args("hello"));
    let output = run(stored, None);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output)
            .contains("Provider: Zero-X gateway · credential: stored login, credentials file"),
        "{}",
        stderr(&output)
    );
    assert_bearer(&gateway.last_chat(), STORED_TOKEN);

    // `--provider openai` points the OpenAI client at OPENAI_BASE_URL with
    // the ambient key instead.
    let mut explicit = workspace.command(&gateway);
    explicit
        .env("OPENAI_API_KEY", "sk-ambient-openai-key")
        .env("OPENAI_BASE_URL", &gateway.base)
        .args(["--provider", "openai"])
        .args(prompt_args("hello"));
    let output = run(explicit, None);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output)
            .contains("Provider: OpenAI · credential: OPENAI_API_KEY · selected by --provider"),
        "{}",
        stderr(&output)
    );
    assert_bearer(&gateway.last_chat(), "sk-ambient-openai-key");
}

#[test]
#[ignore = "needs runtime/src/config_validate.rs to accept the top-level \"provider\" key (cross-package request); every strict ConfigLoader::load rejects it today"]
fn settings_provider_selects_the_provider_and_typos_name_the_file() {
    let gateway = gateway_with_chat(|_, _| text_reply("answered"));
    let workspace = Workspace::new("settings-provider");
    workspace.store_login(STORED_TOKEN);
    let settings = workspace.root.join("config/settings.json");

    // The settings choice beats the stored login and is named on startup.
    fs::write(
        &settings,
        r#"{"defaultMcpServers":false,"provider":"openai"}"#,
    )
    .unwrap();
    let mut chosen = workspace.command(&gateway);
    chosen
        .env("OPENAI_API_KEY", "sk-ambient-openai-key")
        .env("OPENAI_BASE_URL", &gateway.base)
        .args(prompt_args("hello"));
    let output = run(chosen, None);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).contains(
            "Provider: OpenAI · credential: OPENAI_API_KEY · selected by settings \"provider\""
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(stdout(&output).trim(), "answered");
    assert_bearer(&gateway.last_chat(), "sk-ambient-openai-key");

    // A typo is an actionable error naming the file, not a silent fallback
    // to the stored login.
    fs::write(
        &settings,
        r#"{"defaultMcpServers":false,"provider":"opnai"}"#,
    )
    .unwrap();
    let chats_before = gateway.count("POST", "/v1/chat/completions");
    let mut typo = workspace.command(&gateway);
    typo.args(prompt_args("hello"));
    let output = run(typo, None);
    assert!(!output.status.success());
    let errors = stderr(&output);
    assert!(errors.contains("unknown provider 'opnai'"), "{errors}");
    assert!(errors.contains("settings.json"), "{errors}");
    assert!(errors.contains("expected one of zero-x|openai"), "{errors}");
    assert!(!errors.contains("Provider: Zero-X"), "{errors}");
    assert_eq!(gateway.count("POST", "/v1/chat/completions"), chats_before);
}

#[test]
fn doctor_sends_the_stored_login_to_the_server_only_when_asked() {
    let gateway = gateway_with_chat(|_, _| text_reply("unused"));
    let workspace = Workspace::new("doctor-online");
    workspace.store_login(STORED_TOKEN);
    let auth_check = |output: &Output| {
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {}", stdout(output)));
        report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["name"] == "auth")
            .unwrap()
            .clone()
    };

    // Piped output (scripts, test harnesses): signed in, not sent anywhere.
    let mut offline = workspace.command(&gateway);
    offline.args(["--output-format", "json", "doctor"]);
    let auth = auth_check(&run(offline, None));
    assert_eq!(auth["gateway_session"], "not_checked");
    assert_eq!(gateway.count("GET", "/auth/session"), 0);

    let mut online = workspace.command(&gateway);
    online
        .env("NEURON_DOCTOR_ONLINE", "1")
        .args(["--output-format", "json", "doctor"]);
    let auth = auth_check(&run(online, None));
    assert_eq!(auth["gateway_session"], "valid");
    assert_eq!(gateway.count("GET", "/auth/session"), 1);
    let session_check = gateway
        .requests()
        .into_iter()
        .find(|request| request.path == "/auth/session")
        .unwrap();
    assert_bearer(&session_check, STORED_TOKEN);
}

#[test]
fn expired_stored_login_prints_the_login_instruction_once() {
    let gateway = gateway_with_chat(|_, _| text_reply("unused"));
    let workspace = Workspace::new("expired-stored");
    fs::write(
        workspace.neuron_home().join("credentials.json"),
        serde_json::json!({"version": 1, "token": STORED_TOKEN, "expires_at": 1_000_u64, "plan": "free", "saved_at": 0}).to_string(),
    )
    .unwrap();
    let mut command = workspace.command(&gateway);
    command.args(prompt_args("hello"));
    let output = run(command, None);

    assert!(!output.status.success());
    let errors = stderr(&output);
    assert_eq!(
        errors
            .matches("Your Zero-X session expired or was revoked — run neuron auth login")
            .count(),
        1,
        "{errors}"
    );
    assert_eq!(gateway.count("POST", "/v1/chat/completions"), 0);
}

#[test]
fn prompt_subcommand_text_may_mention_the_provider_flag() {
    let gateway = gateway_with_chat(|_, _| text_reply("explained"));
    let workspace = Workspace::new("prompt-words");
    let mut command = workspace.command(&gateway);
    command.env("NEURON_TOKEN", SESSION_TOKEN).args([
        "--compact",
        "--permission-mode",
        "read-only",
        "prompt",
        "explain",
        "the",
        "--provider",
        "flag",
    ]);
    let output = run(command, None);

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "explained");
    let body = gateway.last_chat().body;
    assert!(body.contains("explain the --provider flag"), "{body}");
}

#[test]
fn catalog_cache_is_per_credential_and_dropped_by_upgrade() {
    const OTHER_TOKEN: &str = "ses_othercaller0123456789abcdefgh";
    let gateway = gateway_with_chat(|_, _| text_reply("cached"));
    let workspace = Workspace::new("catalog-key");
    let prompt_as = |token: &str| {
        let mut command = workspace.command(&gateway);
        command
            .env("NEURON_TOKEN", token)
            .args(prompt_args("hello"));
        let output = run(command, None);
        assert!(output.status.success(), "{}", stderr(&output));
    };

    prompt_as(SESSION_TOKEN);
    prompt_as(SESSION_TOKEN);
    assert_eq!(gateway.count("GET", "/v1/models"), 1);
    // Availability is per caller: another credential fetches its own list.
    prompt_as(OTHER_TOKEN);
    assert_eq!(gateway.count("GET", "/v1/models"), 2);
    let cache = fs::read_to_string(workspace.neuron_home().join("catalog.json")).unwrap();
    assert!(!cache.contains(OTHER_TOKEN) && !cache.contains(SESSION_TOKEN));

    // Upgrading changes the plan, so the cached availability is dropped.
    let mut upgrade = workspace.command(&gateway);
    upgrade.args(["upgrade", "--no-browser"]);
    assert!(run(upgrade, None).status.success());
    prompt_as(OTHER_TOKEN);
    assert_eq!(gateway.count("GET", "/v1/models"), 3);
}
