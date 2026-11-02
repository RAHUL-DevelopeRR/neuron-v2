//! `--output-format stream-json`: newline-delimited events of a headless run.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use mock_anthropic_service::{MockAnthropicService, SCENARIO_PREFIX};
use serde_json::Value;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[test]
#[ignore = "on Windows the binary reads the real profile's credential store until the USERPROFILE-scoped store (WP-A1) lands; passes with it, run explicitly"]
fn stream_json_emits_init_tool_events_and_a_final_result() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    let server = runtime
        .block_on(MockAnthropicService::spawn())
        .expect("mock service should start");
    let base_url = server.base_url();

    let workspace = unique_temp_dir("stream-json");
    let config_home = workspace.join("config-home");
    let home = workspace.join("home");
    fs::create_dir_all(&config_home).expect("config home should exist");
    fs::create_dir_all(&home).expect("home should exist");
    fs::write(workspace.join("fixture.txt"), "alpha parity line\n").expect("fixture should write");

    let prompt = format!("{SCENARIO_PREFIX}read_file_roundtrip");
    let output = run_neuron(
        &workspace,
        &config_home,
        &home,
        &base_url,
        &[
            "--model",
            "sonnet",
            "--permission-mode",
            "read-only",
            "--allowedTools",
            "read_file",
            "--output-format",
            "stream-json",
            &prompt,
        ],
    );
    assert!(
        output.status.success(),
        "stream-json run should succeed\nstdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let lines = stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("every line is JSON"))
        .collect::<Vec<_>>();
    let types = lines
        .iter()
        .map(|line| line["type"].as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();

    assert_eq!(types.first().map(String::as_str), Some("init"), "{types:?}");
    assert_eq!(types.last().map(String::as_str), Some("result"), "{types:?}");
    let init = &lines[0];
    assert!(init["session_id"].as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(init["tools"], serde_json::json!(["read_file"]));
    assert!(types.iter().any(|kind| kind == "tool_call_started"), "{types:?}");
    let tool_result = lines
        .iter()
        .find(|line| line["type"] == "tool_result")
        .expect("a tool_result event");
    assert_eq!(tool_result["name"], "read_file");
    assert_eq!(tool_result["truncated"], false);
    let result = lines.last().unwrap();
    assert_eq!(
        result["message"],
        "read_file roundtrip complete: alpha parity line"
    );
    assert_eq!(result["session_id"], init["session_id"]);
    assert!(result["usage"]["input_tokens"].is_u64());
    assert!(result["tool_uses"].is_array());

    fs::remove_dir_all(&workspace).expect("workspace cleanup should succeed");
}
fn run_neuron(
    cwd: &std::path::Path,
    config_home: &std::path::Path,
    home: &std::path::Path,
    base_url: &str,
    args: &[&str],
) -> Output {
    // Keep the default MCP servers (spawned through npx) out of the harness.
    fs::write(
        config_home.join("settings.json"),
        r#"{"defaultMcpServers":false}"#,
    )
    .expect("harness settings should write");
    let mut command = Command::new(env!("CARGO_BIN_EXE_neuron"));
    command
        .current_dir(cwd)
        .env_clear()
        .env("ANTHROPIC_API_KEY", "test-stream-json-key")
        .env("ANTHROPIC_BASE_URL", base_url)
        .env("CLAW_CONFIG_HOME", config_home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("NO_COLOR", "1")
        .env("PATH", "/usr/bin:/bin")
        .args(args);
    // Windows networking needs SystemRoot even in a cleared environment.
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", system_root);
    }
    command.output().expect("claw should launch")
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_millis();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "neuron-stream-json-{label}-{}-{millis}-{counter}",
        std::process::id()
    ))
}
