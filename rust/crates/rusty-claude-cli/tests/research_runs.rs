//! `neuron research` against the mock model service and a mock Zero-X gateway:
//! - without `--allow-web` (and no allow rules) it refuses before any model call or
//!   search;
//! - with `--allow-web`, every search goes to `POST /v1/search` with
//!   `purpose: "research"` and one `x-neuron-research-run` id for the whole run;
//!   results on private addresses are refused by the fetch guard, and the report
//!   saved under `.neuron/research/` says that no page could be read;
//! - in the full-screen UI's engine, `/research` sent as a prompt asks to start, then
//!   runs as the turn: searches and reads as tool rows, the plan as the task list, the
//!   report as the turn's text.

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
const TEST_TOKEN: &str = "ses_researchruns0123456789abcdef";

/// Results on a loopback and a private address: the fetch guard refuses both
/// without touching the network.
const SEARCH_RESULTS: &str = r#"{"results":[{"title":"Local","url":"http://127.0.0.1:9/runtimes","snippet":"loopback"},{"title":"Private","url":"http://10.0.0.1/runtimes","snippet":"private"}],"provider":"searxng","cached":false,"quota":{"remaining":2,"limit":3,"resets_at":"2026-11-05T00:00:00Z"}}"#;

#[test]
fn research_refuses_to_start_without_a_web_grant() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let gateway = MockGateway::spawn();
    let workspace = Workspace::new("no-grant");

    let output = research(&workspace, &server.base_url(), &gateway, &[]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("add --allow-web"), "{stderr}");
    assert!(stderr.contains("neuron --allow-web research"), "{stderr}");
    assert!(gateway.searches().is_empty());
    assert!(runtime.block_on(server.captured_requests()).is_empty());
    assert!(!workspace.root.join(".neuron").join("research").exists());
    workspace.remove();
}

#[test]
fn research_searches_with_one_run_id_and_saves_the_report() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let gateway = MockGateway::spawn();
    let workspace = Workspace::new("allow-web");

    let output = research(&workspace, &server.base_url(), &gateway, &["--allow-web"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // No page could be read: the report says so and the exit code is 1.
    assert_eq!(output.status.code(), Some(1), "{stdout}\n{stderr}");
    assert!(stdout.starts_with("# Research: "), "{stdout}");
    assert!(stdout.contains("No page could be read"), "{stdout}");
    assert!(stderr.contains("search: rust async runtimes"), "{stderr}");
    assert!(
        stderr.contains("read: http://127.0.0.1:9/runtimes"),
        "{stderr}"
    );
    assert!(stderr.contains("! not read:"), "{stderr}");

    // Both planned sub-questions were searched (the quick follow-up check found
    // nothing to add), each with the research purpose and the same run id.
    let searches = gateway.searches();
    assert_eq!(searches.len(), 2, "{searches:?}");
    let run_ids = searches
        .iter()
        .map(|request| {
            let lowered = request.to_ascii_lowercase();
            assert!(
                lowered.contains(&format!("authorization: bearer {TEST_TOKEN}")),
                "{request}"
            );
            assert!(request.contains(r#""purpose":"research""#), "{request}");
            lowered
                .lines()
                .find_map(|line| line.strip_prefix("x-neuron-research-run: "))
                .map(str::to_string)
                .expect("a run id header")
        })
        .collect::<Vec<_>>();
    assert!(run_ids[0].starts_with("rr_"), "{run_ids:?}");
    assert_eq!(run_ids[0], run_ids[1]);
    assert!(searches[0].contains(r#""query":"rust async runtimes""#));
    assert!(searches[1].contains(r#""query":"tokio release notes""#));

    // The report is saved under the workspace, and the model never saw a page.
    let saved = fs::read_dir(workspace.root.join(".neuron").join("research"))
        .expect("research folder")
        .map(|entry| entry.expect("entry").path())
        .collect::<Vec<_>>();
    assert_eq!(saved.len(), 1, "{saved:?}");
    let report = fs::read_to_string(&saved[0]).expect("report");
    assert_eq!(report.trim_end(), stdout.trim_end());
    assert!(report.contains(&run_ids[0].to_string()) || report.contains("Research run `rr_"));
    let requests = runtime.block_on(server.captured_requests());
    assert!(requests
        .iter()
        .all(|request| !request.raw_body.contains("loopback")));
    workspace.remove();
}

#[test]
fn research_json_output_names_the_stop_reason_and_path() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let gateway = MockGateway::spawn();
    let workspace = Workspace::new("json");

    let output = research(
        &workspace,
        &server.base_url(),
        &gateway,
        &["--allow-web", "--output-format", "json"],
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stdout}\n{stderr}");
    assert!(stderr.trim().is_empty(), "{stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(value["kind"], "research");
    assert_eq!(value["pages_read"], 0);
    assert_eq!(value["pages_rejected"], 2);
    assert_eq!(value["searches"], 2);
    assert!(value["path"]
        .as_str()
        .is_some_and(|path| path.contains("research")));
    assert!(value["run_id"].as_str().unwrap().starts_with("rr_"));
    workspace.remove();
}

/// `neuron engine --stdio` (the full-screen UI's engine) with a stored login, the model
/// on the mock service and searches on the mock gateway. Returns the process, a writer
/// for its input and its events.
fn engine(
    workspace: &Workspace,
    base_url: &str,
    gateway: &MockGateway,
) -> (
    std::process::Child,
    std::process::ChildStdin,
    std::sync::mpsc::Receiver<serde_json::Value>,
) {
    use std::io::BufRead;
    let mut child = neuron(workspace, base_url, gateway)
        .args([
            "--provider",
            "anthropic",
            "engine",
            "--stdio",
            "--model",
            "sonnet",
        ])
        .stdin(Stdio::piped())
        .spawn()
        .expect("the engine starts");
    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");
    thread::spawn(move || for _ in std::io::BufReader::new(stderr).lines() {});
    let (events_tx, events) = std::sync::mpsc::channel();
    thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            let event: serde_json::Value =
                serde_json::from_str(&line).expect("stdout carries JSON events");
            if events_tx.send(event).is_err() {
                return;
            }
        }
    });
    (child, stdin, events)
}

fn send(stdin: &mut std::process::ChildStdin, message: &serde_json::Value) {
    writeln!(stdin, "{message}").expect("the engine reads its input");
    stdin.flush().expect("flushed");
}

fn submit(text: &str) -> serde_json::Value {
    serde_json::json!({"kind": "command", "type": "submit", "text": text, "attachments": []})
}

/// Events up to and including the first that `last` accepts.
fn until(
    events: &std::sync::mpsc::Receiver<serde_json::Value>,
    last: impl Fn(&serde_json::Value) -> bool,
) -> Vec<serde_json::Value> {
    let mut seen = Vec::new();
    loop {
        let event = events
            .recv_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| panic!("the engine reports in time; so far: {seen:?}"));
        let done = last(&event);
        seen.push(event);
        if done {
            return seen;
        }
    }
}

fn is_question(event: &serde_json::Value) -> bool {
    event["type"] == "notice"
        && event["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("Question: Research (quick depth)"))
}

#[test]
fn the_engine_runs_research_as_a_turn_with_tool_rows_and_a_task_list() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let gateway = MockGateway::spawn();
    let workspace = Workspace::new("engine");
    let (mut child, mut stdin, events) = engine(&workspace, &server.base_url(), &gateway);

    let ready = until(&events, |event| event["type"] == "ready");
    let features = ready.last().unwrap()["features"].clone();
    assert!(
        features
            .as_array()
            .is_some_and(|features| features.iter().any(|feature| feature == "research")),
        "{features}"
    );
    let question = format!("{SCENARIO_PREFIX}research_run which Rust async runtime?");

    // Declined: nothing is searched.
    send(
        &mut stdin,
        &submit(&format!("/research --depth quick {question}")),
    );
    until(&events, is_question);
    send(&mut stdin, &submit("2"));
    let declined = until(&events, |event| event["type"] == "turn_finished");
    assert!(
        declined
            .iter()
            .any(|event| event["message"] == "Research not started."),
        "{declined:?}"
    );
    assert!(gateway.searches().is_empty());

    // Started: searches and reads are the turn's tool rows, the plan its task list,
    // and the report its text.
    send(
        &mut stdin,
        &submit(&format!("/research --depth quick {question}")),
    );
    until(&events, is_question);
    send(&mut stdin, &submit("1"));
    let turn = until(&events, |event| event["type"] == "turn_finished");
    let calls = turn
        .iter()
        .filter(|event| event["type"] == "tool_call_started")
        .map(|event| event["name"].as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        calls,
        vec!["WebSearch", "WebFetch", "WebFetch", "WebSearch"],
        "{turn:?}"
    );
    let first = turn
        .iter()
        .find(|event| event["type"] == "tool_call_started")
        .unwrap();
    assert_eq!(first["input"], r#"{"query":"rust async runtimes"}"#);
    let results = turn
        .iter()
        .filter(|event| event["type"] == "tool_result")
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 4);
    assert!(results[1]["is_error"].as_bool().unwrap());
    let started_ids = turn
        .iter()
        .filter(|event| event["type"] == "tool_call_started")
        .map(|event| event["id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        results
            .iter()
            .map(|event| event["id"].clone())
            .collect::<Vec<_>>(),
        started_ids
    );
    let todos = turn
        .iter()
        .filter(|event| event["type"] == "todos")
        .last()
        .expect("a task list");
    let items = todos["items"].as_array().unwrap();
    assert_eq!(items.last().unwrap()["content"], "Write the cited report");
    assert!(items.iter().all(|item| item["status"] == "completed"));
    let text = turn
        .iter()
        .filter(|event| event["type"] == "text_delta")
        .map(|event| event["text"].as_str().unwrap_or_default())
        .collect::<String>();
    assert!(text.starts_with("# Research: "), "{text}");
    assert!(text.contains("No page could be read"), "{text}");
    assert!(turn.iter().any(|event| event["type"] == "notice"
        && event["message"]
            .as_str()
            .is_some_and(|message| message.contains("Saved to .neuron"))));
    assert_eq!(gateway.searches().len(), 2);
    assert!(gateway
        .searches()
        .iter()
        .all(|request| request.contains(r#""purpose":"research""#)));
    assert_eq!(
        fs::read_dir(workspace.root.join(".neuron").join("research"))
            .expect("research folder")
            .count(),
        1
    );

    drop(stdin);
    let _ = child.wait();
    workspace.remove();
}

/// `neuron --provider anthropic --model sonnet [extra] research --depth quick
/// "<scenario> which runtime"`, signed in to the mock gateway.
fn research(
    workspace: &Workspace,
    base_url: &str,
    gateway: &MockGateway,
    extra: &[&str],
) -> Output {
    neuron(workspace, base_url, gateway)
        .args(["--provider", "anthropic", "--model", "sonnet"])
        .args(extra)
        .args(["research", "--depth", "quick"])
        .arg(format!(
            "{SCENARIO_PREFIX}research_run which Rust async runtime?"
        ))
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
            "neuron-research-{label}-{}-{millis}-{counter}",
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
