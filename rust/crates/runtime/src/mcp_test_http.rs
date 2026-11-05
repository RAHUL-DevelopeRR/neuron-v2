//! A tiny in-process HTTP/1.1 server for the remote MCP transport and OAuth tests.
//! Each connection is answered once and closed; a handler may instead take the
//! connection over (an SSE stream that stays open).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub(crate) struct FakeRequest {
    pub method: String,
    /// Path with its query.
    pub target: String,
    /// Header names lowercased.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl FakeRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or_default()
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }

    /// `application/x-www-form-urlencoded` body fields.
    pub fn form(&self) -> Vec<(String, String)> {
        reqwest::Url::parse(&format!("http://x/?{}", self.body))
            .map(|url| url.query_pairs().into_owned().collect())
            .unwrap_or_default()
    }
}

type StreamWriter = Box<dyn FnOnce(TcpStream) + Send>;

pub(crate) enum FakeResponse {
    Full {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    },
    Stream(StreamWriter),
}

impl FakeResponse {
    pub fn status(status: u16) -> Self {
        Self::Full {
            status,
            headers: Vec::new(),
            body: String::new(),
        }
    }

    pub fn json(status: u16, value: &Value) -> Self {
        Self::Full {
            status,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: value.to_string(),
        }
    }

    /// One SSE response holding `messages` as `message` events.
    pub fn sse(messages: &[Value]) -> Self {
        let mut body = String::from(": opening comment\n\n");
        for message in messages {
            body.push_str("event: message\ndata: ");
            body.push_str(&message.to_string());
            body.push_str("\n\n");
        }
        Self::Full {
            status: 200,
            headers: vec![("Content-Type".into(), "text/event-stream".into())],
            body,
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        if let Self::Full { headers, .. } = &mut self {
            headers.push((name.to_string(), value.to_string()));
        }
        self
    }
}

pub(crate) type Handler = Arc<dyn Fn(&FakeRequest) -> FakeResponse + Send + Sync>;

pub(crate) struct FakeServer {
    pub url: String,
    pub requests: Arc<Mutex<Vec<FakeRequest>>>,
}

impl FakeServer {
    pub fn spawn(handler: Handler) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let handler = Arc::clone(&handler);
                let seen = Arc::clone(&seen);
                thread::spawn(move || serve(stream, &handler, &seen));
            }
        });
        Self { url, requests }
    }

    pub fn requests(&self) -> Vec<FakeRequest> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

fn serve(stream: TcpStream, handler: &Handler, seen: &Mutex<Vec<FakeRequest>>) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let mut headers = Vec::new();
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
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    let length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0_u8; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let request = FakeRequest {
        method,
        target,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    };
    seen.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(request.clone());
    match handler(&request) {
        FakeResponse::Full {
            status,
            headers,
            body,
        } => {
            let mut stream = stream;
            let mut head = format!("HTTP/1.1 {status} X\r\n");
            for (name, value) in headers {
                let _ = std::fmt::Write::write_fmt(&mut head, format_args!("{name}: {value}\r\n"));
            }
            let _ = std::fmt::Write::write_fmt(
                &mut head,
                format_args!(
                    "Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                ),
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body.as_bytes());
            let _ = stream.flush();
        }
        FakeResponse::Stream(writer) => writer(stream),
    }
}

/// What a small MCP server answers to `message` (a request), or `None` for a
/// notification.
pub(crate) fn mcp_reply(message: &Value) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message.get("method")?.as_str()?;
    let result = match method {
        "initialize" => json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {"tools": {}, "resources": {}},
            "serverInfo": {"name": "fake-remote", "version": "1.0.0"}
        }),
        "tools/list" => json!({
            "tools": [{
                "name": "echo",
                "description": "Echoes text",
                "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}
            }]
        }),
        "tools/call" => {
            let text = message
                .pointer("/params/arguments/text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            json!({"content": [{"type": "text", "text": format!("echo:{text}")}], "isError": false})
        }
        "resources/list" => json!({
            "resources": [{"uri": "file://remote/readme", "name": "readme", "mimeType": "text/plain"}]
        }),
        "resources/read" => json!({
            "contents": [{"uri": "file://remote/readme", "mimeType": "text/plain", "text": "remote hello"}]
        }),
        _ => {
            return Some(json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": "method not found"}
            }))
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}
