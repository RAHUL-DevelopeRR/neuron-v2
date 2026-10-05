use std::fmt::Write as _;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn serve_gateway_roundtrip(listener: &TcpListener) {
    for turn in 0..2 {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(20));
                }
                result => panic!("Expected gateway request: {result:?}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line.trim_end(), "POST /v1/chat/completions HTTP/1.1");
        let mut length = 0;
        let mut authorized = false;
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if let Some(value) = lower.strip_prefix("content-length:") {
                length = value.trim().parse().unwrap();
            }
            if lower.trim_end() == "authorization: bearer ses_012345678901234567890123456789" {
                authorized = true;
            }
        }
        assert!(authorized, "Gateway token must authorize requests");
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body).unwrap();
        let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(request["model"], "auto");
        assert!(request["max_tokens"].as_u64().unwrap() <= 8192);
        if turn == 0 {
            assert!(request["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["function"]["name"] == "read_file"));
        } else {
            let messages = request["messages"].as_array().unwrap();
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
        }
        let events = if turn == 0 {
            vec![
                serde_json::json!({"id":"test","model":"auto","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_read","function":{"name":"read_file","arguments":"{\"path\":\""}},{"index":1,"id":"call_second","function":{"name":"read_file","arguments":"{\"path\":\""}}]}}]}),
                serde_json::json!({"id":"test","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"fixture.txt\"}"}},{"index":1,"function":{"arguments":"second.txt\"}"}}]}}]}),
                serde_json::json!({"id":"test","choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
            ]
        } else {
            vec![
                serde_json::json!({"id":"test","model":"auto","choices":[{"delta":{"content":"Gateway tool roundtrip complete."}}]}),
                serde_json::json!({"id":"test","choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5}}),
            ]
        };
        let mut sse = String::new();
        for event in events {
            write!(sse, "data: {event}\r\n\r\n").unwrap();
        }
        sse.push_str("data: [DONE]\n\n");
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse}", sse.len()).unwrap();
    }
}

#[test]
fn gateway_default_model_preserves_parallel_tool_calls_and_file_results() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = thread::spawn(move || serve_gateway_roundtrip(&listener));
    let workspace =
        std::env::temp_dir().join(format!("neuron-gateway-test-{}", std::process::id()));
    fs::create_dir_all(workspace.join("home")).unwrap();
    fs::write(workspace.join("fixture.txt"), "gateway fixture\n").unwrap();
    fs::write(workspace.join("second.txt"), "second fixture\n").unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_neuron"));
    command
        .env_clear()
        .current_dir(&workspace)
        .env("NEURON_TOKEN", "ses_012345678901234567890123456789")
        .env("NEURON_API_BASE", base)
        .env("HOME", workspace.join("home"))
        .env("USERPROFILE", workspace.join("home"))
        .env("CLAW_CONFIG_HOME", workspace.join("config"))
        .env("NO_COLOR", "1")
        .args([
            "--compact",
            "--permission-mode",
            "read-only",
            "--allowedTools",
            "read_file",
            "prompt",
            "Read fixture.txt",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", system_root);
    }
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "Gateway roundtrip timed out: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output().unwrap();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "Gateway tool roundtrip complete."
    );
    fs::remove_dir_all(&workspace).unwrap();
}
