//! `neuron mcp serve` checks every call against the user's permission settings
//! (read-only unless `--permission-mode` says otherwise), annotates its tools, and
//! refuses to serve an untrusted folder.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "neuron-mcp-serve-{label}-{}-{nanos}",
        std::process::id()
    ));
    for dir in ["workspace", "config", "home"] {
        fs::create_dir_all(root.join(dir)).expect("temp dir");
    }
    root
}

fn frame(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

/// The JSON-RPC messages in `stdout`, in order.
fn frames(stdout: &[u8]) -> Vec<Value> {
    let text = String::from_utf8_lossy(stdout);
    let mut rest = text.as_ref();
    let mut messages = Vec::new();
    while let Some(start) = rest.find("Content-Length:") {
        rest = &rest[start + "Content-Length:".len()..];
        let (length, after) = rest.split_once("\r\n\r\n").expect("header end");
        let length = length.trim().parse::<usize>().expect("length");
        messages.push(serde_json::from_str(&after[..length]).expect("json"));
        rest = &after[length..];
    }
    messages
}

/// Runs `neuron mcp serve` in `root`'s workspace with `requests` on stdin.
fn serve(root: &Path, extra_args: &[&str], trusted: bool, requests: &[Value]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_neuron"));
    command
        .current_dir(root.join("workspace"))
        .args(extra_args)
        .args(["mcp", "serve"])
        .env("NEURON_CONFIG_HOME", root.join("config"))
        .env("NEURON_HOME", root.join("home"))
        .env_remove("CLAW_CONFIG_HOME")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if trusted {
        command.env("NEURON_TRUST_WORKSPACE", "1");
    } else {
        command.env_remove("NEURON_TRUST_WORKSPACE");
    }
    let mut child = command.spawn().expect("neuron starts");
    {
        let mut stdin = child.stdin.take().expect("stdin");
        for request in requests {
            stdin.write_all(&frame(request)).expect("request");
        }
    }
    child.wait_with_output().expect("neuron exits")
}

fn call(id: u64, name: &str, arguments: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
}

#[test]
fn serve_refuses_a_destructive_command_by_default_and_annotates_its_tools() {
    let root = temp_root("default");
    let workspace = root.join("workspace");
    fs::create_dir_all(workspace.join("victim")).expect("victim");
    fs::write(workspace.join("hello.txt"), "hello from the workspace").expect("file");

    let output = serve(
        &root,
        &[],
        true,
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
            call(3, "bash", &json!({"command": "rm -rf victim"})),
            call(4, "read_file", &json!({"path": "hello.txt"})),
            call(5, "write_file", &json!({"path": "new.txt", "content": "x"})),
        ],
    );
    let messages = frames(&output.stdout);
    assert_eq!(
        messages.len(),
        5,
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let tools = messages[1]["result"]["tools"].as_array().expect("tools");
    let annotations = |name: &str| {
        tools.iter().find(|tool| tool["name"] == name).map_or_else(
            || panic!("{name} is listed"),
            |tool| tool["annotations"].clone(),
        )
    };
    assert_eq!(annotations("read_file")["readOnlyHint"], true);
    assert_eq!(annotations("bash")["readOnlyHint"], false);
    assert_eq!(annotations("bash")["destructiveHint"], true);
    assert_eq!(annotations("WebFetch")["openWorldHint"], true);
    assert_eq!(annotations("write_file")["readOnlyHint"], false);

    let refused = &messages[2]["result"];
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(workspace.join("victim").is_dir(), "rm ran");
    let read = &messages[3]["result"];
    assert_eq!(read["isError"], false, "{read}");
    assert!(
        read.to_string().contains("hello from the workspace"),
        "{read}"
    );
    assert_eq!(messages[4]["result"]["isError"], true);
    assert!(!workspace.join("new.txt").exists());

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn serve_follows_permission_mode_and_deny_rules() {
    let root = temp_root("mode");
    fs::write(
        root.join("config").join("settings.json"),
        r#"{"permissions":{"deny":["read_file"]}}"#,
    )
    .expect("settings");
    fs::write(root.join("workspace").join("hello.txt"), "hello").expect("file");
    let output = serve(
        &root,
        &["--permission-mode", "workspace-write"],
        true,
        &[
            call(1, "write_file", &json!({"path": "new.txt", "content": "x"})),
            call(2, "read_file", &json!({"path": "hello.txt"})),
        ],
    );
    let messages = frames(&output.stdout);
    assert_eq!(
        messages.len(),
        2,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(messages[0]["result"]["isError"], false, "{}", messages[0]);
    assert!(root.join("workspace").join("new.txt").is_file());
    assert_eq!(messages[1]["result"]["isError"], true);
    assert!(
        messages[1].to_string().contains("denied by rule"),
        "{}",
        messages[1]
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn serve_refuses_an_untrusted_folder() {
    let root = temp_root("untrusted");
    let output = serve(&root, &[], false, &[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not trusted"), "{stderr}");
    assert_eq!(frames(&output.stdout), [] as [serde_json::Value; 0]);
    let _ = fs::remove_dir_all(&root);
}
