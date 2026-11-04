//! Web search in runs nobody attends, against the mock model service and a mock
//! Zero-X gateway:
//! - `--allow-web` lets a script's `WebSearch` run without a prompt; signed in, it goes
//!   to the gateway's `POST /v1/search` with the session token, and the snippets reach
//!   the model inside the external-content envelope;
//! - without `--allow-web` the same search is refused, so the grant is what changed;
//! - `neuron plugins list` and unknown `neuron mcp`/`neuron plugins` words never reach
//!   the model.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mock_anthropic_service::{MockAnthropicService, SCENARIO_PREFIX};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A Zero-X session token that passes the client's format check.
const TEST_TOKEN: &str = "ses_webrunstest0123456789abcdef";

const SEARCH_RESULTS: &str = r#"{"results":[{"title":"Node.js 24 LTS","url":"https://nodejs.org/en/blog/release/v24.11.0","snippet":"Node.js 24 enters long-term support.","published":"2026-10-28"}],"provider":"searxng","cached":false,"quota":{"remaining":39,"limit":40,"resets_at":"2026-11-05T00:00:00Z"}}"#;

#[test]
fn allow_web_lets_a_script_search_through_the_gateway() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let gateway = MockGateway::spawn();
    let workspace = Workspace::new("allow-web");

    let output = run(
        &workspace,
        &server.base_url(),
        &gateway,
        &["--allow-web"],
        "web_search_denied",
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(!stderr.contains("was not run"), "{stderr}");
    // The scenario answers with what the search returned.
    assert!(
        stdout.contains("Node.js 24 enters long-term support."),
        "{stdout}"
    );
    assert!(stdout.contains("<external-content id="), "{stdout}");

    let searches = gateway.searches();
    assert_eq!(searches.len(), 1, "{searches:?}");
    let request = &searches[0];
    assert!(
        request
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {TEST_TOKEN}")),
        "{request}"
    );
    assert!(
        request.contains(r#""query":"latest node lts""#),
        "{request}"
    );
    assert!(request.contains(r#""purpose":"search""#), "{request}");
    workspace.remove();
}

#[test]
fn without_allow_web_the_same_search_is_refused() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let gateway = MockGateway::spawn();
    let workspace = Workspace::new("no-allow-web");

    let output = run(
        &workspace,
        &server.base_url(),
        &gateway,
        &[],
        "web_search_denied",
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("tool 'WebSearch' was not run"), "{stderr}");
    assert!(gateway.searches().is_empty());
    workspace.remove();
}

#[test]
fn plugins_list_and_unknown_subcommands_make_no_model_call() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let gateway = MockGateway::spawn();
    let workspace = Workspace::new("plugins-local");

    let output = neuron(&workspace, &server.base_url(), &gateway)
        .args(["plugins", "list"])
        .output()
        .expect("neuron should run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(stdout.contains("Plugins"), "{stdout}");

    for words in [&["plugins", "lst"][..], &["mcp", "lst"][..]] {
        let output = neuron(&workspace, &server.base_url(), &gateway)
            .args(words)
            .output()
            .expect("neuron should run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_ne!(output.status.code(), Some(0), "{words:?}: {stderr}");
        assert!(
            stderr.contains("does not take `lst`"),
            "{words:?}: {stderr}"
        );
        assert!(stderr.contains(&format!("neuron {}", words[0])), "{stderr}");
    }

    assert_eq!(
        runtime.block_on(server.captured_requests()),
        Vec::<mock_anthropic_service::CapturedRequest>::new()
    );
    workspace.remove();
}

/// `neuron --provider anthropic --model sonnet --permission-mode workspace-write
/// [extra] <scenario>`, signed in to the mock gateway.
fn run(
    workspace: &Workspace,
    base_url: &str,
    gateway: &MockGateway,
    extra: &[&str],
    scenario: &str,
) -> Output {
    neuron(workspace, base_url, gateway)
        .args([
            "--provider",
            "anthropic",
            "--model",
            "sonnet",
            "--permission-mode",
            "workspace-write",
        ])
        .args(extra)
        .arg(format!("{SCENARIO_PREFIX}{scenario}"))
        .stdin(Stdio::null())
        .output()
        .expect("neuron should run")
}

/// `neuron` in `workspace`: the model is the mock service, the Zero-X gateway (for
/// searches) the mock gateway, with a stored login of [`TEST_TOKEN`].
fn neuron(workspace: &Workspace, base_url: &str, gateway: &MockGateway) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_neuron"));
    command
        .current_dir(&workspace.root)
        .env_clear()
        .env("ANTHROPIC_API_KEY", "test-parity-key")
        .env("ANTHROPIC_BASE_URL", base_url)
        .env("NEURON_API_BASE", format!("{}/v1", gateway.origin))
        // A stored login (`NEURON_TOKEN` would also make the gateway the model
        // provider), in a file, never the OS keychain.
        .env("NEURON_HOME", workspace.neuron_home())
        .env("NEURON_CREDENTIAL_STORE", "file")
        .env("CLAW_CONFIG_HOME", &workspace.config_home)
        .env("HOME", &workspace.home)
        .env("USERPROFILE", &workspace.home)
        .env("NO_COLOR", "1")
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
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

/// A gateway that answers `POST /v1/search` with [`SEARCH_RESULTS`] and anything else
/// with 404, keeping the raw search requests.
struct MockGateway {
    origin: String,
    searches: Arc<Mutex<Vec<String>>>,
}

impl MockGateway {
    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock gateway");
        let origin = format!("http://{}", listener.local_addr().expect("local addr"));
        let searches = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&searches);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                let request = read_request(&mut stream);
                let (status, body) = if request.starts_with("POST /v1/search ") {
                    seen.lock().unwrap().push(request);
                    ("200 OK", SEARCH_RESULTS)
                } else {
                    ("404 Not Found", r#"{"error":"not_found"}"#)
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self { origin, searches }
    }

    fn searches(&self) -> Vec<String> {
        self.searches.lock().unwrap().clone()
    }
}

/// Headers and the `Content-Length` body of one request.
fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut data = Vec::new();
    let mut buffer = [0_u8; 4096];
    while let Ok(size) = stream.read(&mut buffer) {
        if size == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..size]);
        let text = String::from_utf8_lossy(&data);
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text[..end]
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if data.len() >= end + 4 + length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&data).to_string()
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
            "neuron-web-{label}-{}-{millis}-{counter}",
            std::process::id()
        ));
        let workspace = Self {
            config_home: root.join("config-home"),
            home: root.join("home"),
            root,
        };
        fs::create_dir_all(&workspace.config_home).expect("config home");
        fs::create_dir_all(&workspace.home).expect("home");
        fs::create_dir_all(workspace.neuron_home()).expect("neuron home");
        fs::write(
            workspace.neuron_home().join("credentials.json"),
            serde_json::json!({
                "version": 1, "token": TEST_TOKEN, "expires_at": 4_070_908_800_u64,
                "plan": "free", "saved_at": 0
            })
            .to_string(),
        )
        .expect("stored login");
        // Keep the default MCP servers (spawned through npx) out of the test.
        fs::write(
            workspace.config_home.join("settings.json"),
            r#"{"defaultMcpServers":false}"#,
        )
        .expect("settings");
        workspace
    }

    fn neuron_home(&self) -> PathBuf {
        self.root.join("neuron-home")
    }

    fn remove(self) {
        let _ = fs::remove_dir_all(self.root);
    }
}
