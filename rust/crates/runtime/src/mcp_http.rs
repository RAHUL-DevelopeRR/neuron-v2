//! Remote MCP servers: the Streamable HTTP transport (MCP 2025-06-18) for
//! `"type": "http"` servers, and the older HTTP+SSE transport (2024-11-05) for
//! `"type": "sse"` servers and for `http` servers that turn out to speak only it.
//!
//! Streamable HTTP: every JSON-RPC message is a POST to the server URL accepting
//! `application/json` and `text/event-stream`; a request is answered by one JSON
//! object or by an SSE stream that carries the response; notifications get `202`.
//! An `Mcp-Session-Id` from the `initialize` answer goes on every later request,
//! `404` on it means the session ended (the manager starts a new one), and a `DELETE`
//! ends it on shutdown. HTTP+SSE: a GET opens an event stream whose first `endpoint`
//! event names the URL to POST to; responses come back on the stream.
//!
//! Static `headers` may reference environment variables as `${NAME}` and secrets kept
//! in the keychain as `${secret:NAME}` (see `mcp_install`). Header values
//! and tokens never appear in errors or debug output.

use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::process::ExitStatus;
use std::time::Duration;

use reqwest::header::CONTENT_TYPE;
use reqwest::{RequestBuilder, Response, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

use crate::mcp_client::{
    McpClientBootstrap, McpClientTransport, McpManagedProxyTransport, McpRemoteTransport,
};
use crate::mcp_oauth::{auth_required, bearer_token, canonical_resource_uri};
use crate::mcp_stdio::{
    JsonRpcId, JsonRpcRequest, JsonRpcResponse, McpInitializeParams, McpInitializeResult,
    McpListResourcesParams, McpListResourcesResult, McpListToolsParams, McpListToolsResult,
    McpReadResourceParams, McpReadResourceResult, McpStdioProcess, McpToolCallParams,
    McpToolCallResult,
};
use crate::sse::{IncrementalSseParser, SseEvent};

const SESSION_HEADER: &str = "Mcp-Session-Id";
const PROTOCOL_HEADER: &str = "MCP-Protocol-Version";
const DELETE_WAIT: Duration = Duration::from_secs(3);

/// The `initialize` parameters as JSON (for the OAuth discovery probe).
pub(crate) fn initialize_params_json() -> Value {
    json!({
        "protocolVersion": "2025-03-26",
        "capabilities": {},
        "clientInfo": {"name": "runtime", "version": env!("CARGO_PKG_VERSION")}
    })
}

/// A started MCP server: a child process (stdio) or a remote endpoint.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // one per server; the process is the common case
pub(crate) enum McpConnection {
    Stdio(McpStdioProcess),
    Remote(Box<McpRemoteConnection>),
}

impl McpConnection {
    pub(crate) fn spawn(bootstrap: &McpClientBootstrap) -> io::Result<Self> {
        match &bootstrap.transport {
            McpClientTransport::Stdio(transport) => Ok(Self::Stdio(McpStdioProcess::spawn(
                &crate::mcp_install::prepare_stdio_transport(&bootstrap.server_name, transport)?,
            )?)),
            McpClientTransport::Http(transport) => Ok(Self::Remote(Box::new(
                McpRemoteConnection::new(&bootstrap.server_name, transport, Flavor::Streamable)?,
            ))),
            McpClientTransport::Sse(transport) => Ok(Self::Remote(Box::new(
                McpRemoteConnection::new(&bootstrap.server_name, transport, Flavor::LegacySse)?,
            ))),
            McpClientTransport::ManagedProxy(proxy)
                if crate::mcp_zerox::hosted_connectors_enabled() =>
            {
                Ok(Self::Remote(Box::new(McpRemoteConnection::zerox(
                    &bootstrap.server_name,
                    proxy,
                )?)))
            }
            other => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "MCP transport for {} is not supported: {}",
                    bootstrap.server_name,
                    transport_label(other)
                ),
            )),
        }
    }

    pub(crate) fn is_stdio(&self) -> bool {
        matches!(self, Self::Stdio(_))
    }

    pub(crate) fn has_exited(&mut self) -> io::Result<bool> {
        match self {
            Self::Stdio(process) => process.has_exited(),
            Self::Remote(_) => Ok(false),
        }
    }

    /// Waits for a stdio server to exit; a remote one never "exits".
    pub(crate) async fn wait(&mut self) -> io::Result<ExitStatus> {
        match self {
            Self::Stdio(process) => process.wait().await,
            Self::Remote(_) => std::future::pending().await,
        }
    }

    pub(crate) fn start_kill(&mut self) {
        if let Self::Stdio(process) = self {
            process.start_kill();
        }
    }

    pub(crate) async fn shutdown(&mut self) -> io::Result<()> {
        match self {
            Self::Stdio(process) => process.shutdown().await,
            Self::Remote(remote) => {
                remote.shutdown().await;
                Ok(())
            }
        }
    }

    pub(crate) async fn notify_initialized(&mut self) -> io::Result<()> {
        match self {
            Self::Stdio(process) => process.notify_initialized().await,
            Self::Remote(remote) => remote.notify_initialized().await,
        }
    }

    async fn request<P: Serialize, R: DeserializeOwned>(
        &mut self,
        id: JsonRpcId,
        method: &str,
        params: Option<P>,
    ) -> io::Result<JsonRpcResponse<R>> {
        match self {
            Self::Stdio(process) => process.request(id, method, params).await,
            Self::Remote(remote) => remote.request(id, method, params).await,
        }
    }

    pub(crate) async fn initialize(
        &mut self,
        id: JsonRpcId,
        params: McpInitializeParams,
    ) -> io::Result<JsonRpcResponse<McpInitializeResult>> {
        self.request(id, "initialize", Some(params)).await
    }

    pub(crate) async fn list_tools(
        &mut self,
        id: JsonRpcId,
        params: Option<McpListToolsParams>,
    ) -> io::Result<JsonRpcResponse<McpListToolsResult>> {
        self.request(id, "tools/list", params).await
    }

    /// Calls a tool. `confirmed`: the user approved this call in the permission prompt,
    /// which a hosted connector is told (`x-neuron-confirmed`); other servers never are.
    pub(crate) async fn call_tool(
        &mut self,
        id: JsonRpcId,
        params: McpToolCallParams,
        confirmed: bool,
    ) -> io::Result<JsonRpcResponse<McpToolCallResult>> {
        match self {
            Self::Remote(remote) => {
                remote.confirmed = confirmed && remote.is_hosted();
                let response = remote.request(id, "tools/call", Some(params)).await;
                remote.confirmed = false;
                response
            }
            Self::Stdio(_) => self.request(id, "tools/call", Some(params)).await,
        }
    }

    pub(crate) async fn list_resources(
        &mut self,
        id: JsonRpcId,
        params: Option<McpListResourcesParams>,
    ) -> io::Result<JsonRpcResponse<McpListResourcesResult>> {
        self.request(id, "resources/list", params).await
    }

    pub(crate) async fn read_resource(
        &mut self,
        id: JsonRpcId,
        params: McpReadResourceParams,
    ) -> io::Result<JsonRpcResponse<McpReadResourceResult>> {
        self.request(id, "resources/read", Some(params)).await
    }
}

fn transport_label(transport: &McpClientTransport) -> &'static str {
    match transport {
        McpClientTransport::Stdio(_) => "stdio",
        McpClientTransport::Sse(_) => "sse",
        McpClientTransport::Http(_) => "http",
        McpClientTransport::WebSocket(_) => "ws",
        McpClientTransport::Sdk(_) => "sdk",
        McpClientTransport::ManagedProxy(_) => "zerox",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flavor {
    Streamable,
    LegacySse,
}

/// Replaces `${NAME}` with the environment variable `NAME` (empty when unset).
#[must_use]
pub fn expand_env_refs(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        if let Some(end) = after.find('}') {
            let name = &after[..end];
            output.push_str(&std::env::var(name).unwrap_or_default());
            rest = &after[end + 1..];
        } else {
            output.push_str(&rest[start..]);
            rest = "";
        }
    }
    output.push_str(rest);
    output
}

fn transport_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        // Redirects stay on the server's origin, so its headers never go elsewhere.
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let same_origin = attempt
                .previous()
                .first()
                .is_some_and(|first| first.origin() == attempt.url().origin());
            if same_origin && attempt.previous().len() <= 5 {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .unwrap_or_default()
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn http_error(error: &reqwest::Error) -> io::Error {
    let kind = if error.is_connect() {
        io::ErrorKind::ConnectionRefused
    } else if error.is_timeout() {
        io::ErrorKind::TimedOut
    } else {
        io::ErrorKind::Other
    };
    // `without_url`: the URL is in the server's config already; keep errors short.
    let mut text = error.to_string();
    if let Some(url) = error.url() {
        text = text.replace(url.as_str(), "<server>");
    }
    io::Error::new(kind, format!("HTTP request failed: {text}"))
}

/// Reads SSE events off a response body as it arrives.
struct SseReader {
    response: Response,
    parser: IncrementalSseParser,
    pending: Vec<u8>,
    queue: VecDeque<SseEvent>,
    done: bool,
}

impl SseReader {
    fn new(response: Response) -> Self {
        Self {
            response,
            parser: IncrementalSseParser::new(),
            pending: Vec::new(),
            queue: VecDeque::new(),
            done: false,
        }
    }

    async fn next_event(&mut self) -> io::Result<Option<SseEvent>> {
        loop {
            if let Some(event) = self.queue.pop_front() {
                return Ok(Some(event));
            }
            if self.done {
                return Ok(None);
            }
            match self.response.chunk().await {
                Ok(Some(bytes)) => {
                    self.pending.extend_from_slice(&bytes);
                    let valid = match std::str::from_utf8(&self.pending) {
                        Ok(text) => text.len(),
                        Err(error) => error.valid_up_to(),
                    };
                    let text = String::from_utf8_lossy(&self.pending[..valid]).into_owned();
                    self.pending.drain(..valid);
                    self.queue.extend(self.parser.push_chunk(&text));
                }
                Ok(None) => {
                    self.done = true;
                    self.queue.extend(self.parser.finish());
                }
                Err(error) => return Err(http_error(&error)),
            }
        }
    }
}

/// What a message off an SSE stream is to the request waiting on it.
enum Incoming {
    /// The response to the request.
    Reply(Value),
    /// A request from the server, answered with this message.
    ServerRequest(Value),
    Other,
}

fn classify_message(data: &str, wait_id: &JsonRpcId) -> io::Result<Vec<Incoming>> {
    let value: Value = serde_json::from_str(data).map_err(|error| {
        invalid_data(format!("invalid JSON-RPC message on SSE stream: {error}"))
    })?;
    let messages = match value {
        Value::Array(items) => items,
        other => vec![other],
    };
    Ok(messages
        .into_iter()
        .map(|message| classify_one(message, wait_id))
        .collect())
}

fn classify_one(message: Value, wait_id: &JsonRpcId) -> Incoming {
    let is_response = message.get("result").is_some() || message.get("error").is_some();
    let id = message
        .get("id")
        .and_then(|id| serde_json::from_value::<JsonRpcId>(id.clone()).ok());
    match (message.get("method").and_then(Value::as_str), id) {
        (None, Some(id)) if is_response && id == *wait_id => Incoming::Reply(message),
        (Some(method), Some(_)) => {
            let id = message.get("id").cloned().unwrap_or(Value::Null);
            Incoming::ServerRequest(if method == "ping" {
                json!({"jsonrpc": "2.0", "id": id, "result": {}})
            } else {
                json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": {"code": -32601, "message": format!("method not supported by this client: {method}")}
                })
            })
        }
        _ => Incoming::Other,
    }
}

struct LegacyStream {
    reader: SseReader,
    post_url: String,
}

/// How a remote server is signed in to.
enum RemoteAuth {
    /// Its own OAuth tokens, when `neuron mcp login` stored some.
    OAuth,
    /// A hosted connector: the Zero-X sign-in, read when the connection first sends
    /// and again after a `401`. `origin` is the gateway's, the only place it goes.
    Zerox {
        token: Option<String>,
        origin: String,
    },
}

/// One remote MCP server connection.
pub(crate) struct McpRemoteConnection {
    server_name: String,
    url: String,
    resource: String,
    flavor: Flavor,
    /// An `http` server that may still turn out to speak only HTTP+SSE.
    may_fall_back: bool,
    client: reqwest::Client,
    headers: Vec<(String, String)>,
    session_id: Option<String>,
    protocol_version: Option<String>,
    legacy: Option<LegacyStream>,
    auth: RemoteAuth,
    /// The `tools/call` being sent was approved by the user (hosted connectors only).
    confirmed: bool,
}

impl fmt::Debug for McpRemoteConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpRemoteConnection")
            .field("server_name", &self.server_name)
            .field("url", &self.url)
            .field("flavor", &self.flavor)
            .field("headers", &self.headers.len())
            .field("has_session", &self.session_id.is_some())
            .field("hosted", &self.is_hosted())
            .finish_non_exhaustive()
    }
}

/// How a POST to a Streamable HTTP endpoint went.
enum PostOutcome {
    Reply(Option<Value>),
    /// `initialize` got 404/405: the server may speak the older HTTP+SSE transport.
    TryLegacy,
}

impl McpRemoteConnection {
    pub(crate) fn new(
        server_name: &str,
        transport: &McpRemoteTransport,
        flavor: Flavor,
    ) -> io::Result<Self> {
        let resource = canonical_resource_uri(&transport.url)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        Ok(Self {
            server_name: server_name.to_string(),
            url: transport.url.clone(),
            resource,
            flavor,
            may_fall_back: flavor == Flavor::Streamable,
            client: transport_client(),
            headers: transport
                .headers
                .iter()
                .map(|(name, value)| {
                    crate::mcp_install::expand_mcp_header(server_name, &transport.url, value)
                        .map(|value| (name.clone(), value))
                        .map_err(|error| io::Error::new(io::ErrorKind::NotFound, error))
                })
                .collect::<io::Result<_>>()?,
            session_id: None,
            protocol_version: None,
            legacy: None,
            auth: RemoteAuth::OAuth,
            confirmed: false,
        })
    }

    /// A hosted connector: Streamable HTTP to the gateway's `/mcp/<id>`, signed in with
    /// the Zero-X sign-in. Fails when its settings name another endpoint.
    pub(crate) fn zerox(server_name: &str, proxy: &McpManagedProxyTransport) -> io::Result<Self> {
        let refused = |error: String| io::Error::new(io::ErrorKind::InvalidInput, error);
        let url = crate::mcp_zerox::resolve_zerox_endpoint(server_name, &proxy.url, &proxy.id)
            .map_err(refused)?;
        let origin = crate::mcp_zerox::zerox_origin().map_err(refused)?;
        Ok(Self {
            server_name: server_name.to_string(),
            resource: url.clone(),
            url,
            flavor: Flavor::Streamable,
            // The gateway speaks Streamable HTTP only.
            may_fall_back: false,
            client: transport_client(),
            headers: Vec::new(),
            session_id: None,
            protocol_version: None,
            legacy: None,
            auth: RemoteAuth::Zerox {
                token: None,
                origin,
            },
            confirmed: false,
        })
    }

    fn is_hosted(&self) -> bool {
        matches!(self.auth, RemoteAuth::Zerox { .. })
    }

    fn with_headers(&self, mut builder: RequestBuilder, bearer: Option<&str>) -> RequestBuilder {
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        if let Some(token) = bearer {
            builder = builder.bearer_auth(token);
        }
        if self.flavor == Flavor::Streamable {
            if let Some(session) = &self.session_id {
                builder = builder.header(SESSION_HEADER, session);
            }
            if let Some(version) = &self.protocol_version {
                builder = builder.header(PROTOCOL_HEADER, version);
            }
        }
        builder
    }

    /// Sends a request built by `build` with the stored token, refreshing the token
    /// and trying once more on a `401`.
    async fn send_authorized(
        &mut self,
        build: impl Fn(&Self, Option<&str>) -> RequestBuilder,
    ) -> io::Result<Response> {
        if self.is_hosted() {
            return self.send_zerox(build).await;
        }
        let mut forced = false;
        loop {
            let bearer = bearer_token(&self.server_name, &self.resource, forced).await?;
            let response = build(self, bearer.as_deref())
                .send()
                .await
                .map_err(|error| http_error(&error))?;
            if response.status() != StatusCode::UNAUTHORIZED {
                return Ok(response);
            }
            if bearer.is_some() && !forced {
                forced = true;
                continue;
            }
            return Err(auth_required(&self.server_name, None));
        }
    }

    /// [`Self::send_authorized`] for a hosted connector: the Zero-X sign-in as the
    /// bearer, read again once after a `401` (the user may have signed in since); a
    /// second `401` asks the user to sign in again. A `403` that says `needs_connect`
    /// marks the server as waiting for its connector to be linked; any other answer
    /// past the sign-in clears that mark.
    async fn send_zerox(
        &mut self,
        build: impl Fn(&Self, Option<&str>) -> RequestBuilder,
    ) -> io::Result<Response> {
        let mut reread = false;
        loop {
            let (token, origin) = match &mut self.auth {
                RemoteAuth::Zerox { token, origin } => {
                    if token.is_none() || reread {
                        *token = Some(crate::mcp_zerox::zerox_token().map_err(|error| {
                            io::Error::new(io::ErrorKind::PermissionDenied, error)
                        })?);
                    }
                    (token.clone(), origin.clone())
                }
                RemoteAuth::OAuth => (None, String::new()),
            };
            let response = build(self, token.as_deref())
                .send()
                .await
                .map_err(|error| http_error(&error))?;
            match response.status() {
                StatusCode::UNAUTHORIZED if !reread => reread = true,
                StatusCode::UNAUTHORIZED => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        crate::mcp_zerox::sign_in_again(&self.server_name),
                    ))
                }
                StatusCode::FORBIDDEN => {
                    let body = response.text().await.unwrap_or_default();
                    let Some(connect_url) = crate::mcp_zerox::needs_connect_url(&body, &origin)
                    else {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            format!(
                                "Zero-X refused the hosted connector `{}` (HTTP 403)",
                                self.server_name
                            ),
                        ));
                    };
                    crate::mcp_zerox::note_needs_connect(&self.server_name, &connect_url);
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        crate::mcp_zerox::needs_connect_message(&self.server_name, &connect_url),
                    ));
                }
                _ => {
                    crate::mcp_zerox::note_connected(&self.server_name);
                    return Ok(response);
                }
            }
        }
    }

    async fn request<P: Serialize, R: DeserializeOwned>(
        &mut self,
        id: JsonRpcId,
        method: &str,
        params: Option<P>,
    ) -> io::Result<JsonRpcResponse<R>> {
        let message = serde_json::to_value(JsonRpcRequest::new(id.clone(), method, params))
            .map_err(|error| invalid_data(error.to_string()))?;
        let reply = self
            .send(&message, Some(&id))
            .await?
            .ok_or_else(|| invalid_data(format!("no response to {method}")))?;
        if method == "initialize" {
            if let Some(version) = reply
                .pointer("/result/protocolVersion")
                .and_then(Value::as_str)
            {
                self.protocol_version = Some(version.to_string());
            }
        }
        let response: JsonRpcResponse<R> = serde_json::from_value(reply)
            .map_err(|error| invalid_data(format!("invalid response to {method}: {error}")))?;
        if response.jsonrpc != "2.0" {
            return Err(invalid_data(format!(
                "MCP response for {method} used unsupported jsonrpc version `{}`",
                response.jsonrpc
            )));
        }
        Ok(response)
    }

    async fn notify_initialized(&mut self) -> io::Result<()> {
        self.send(
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            None,
        )
        .await
        .map(|_| ())
    }

    async fn send(
        &mut self,
        message: &Value,
        wait_id: Option<&JsonRpcId>,
    ) -> io::Result<Option<Value>> {
        if self.flavor == Flavor::Streamable {
            match self.post_streamable(message, wait_id).await? {
                PostOutcome::Reply(reply) => return Ok(reply),
                PostOutcome::TryLegacy => {
                    self.flavor = Flavor::LegacySse;
                    self.session_id = None;
                }
            }
        }
        self.send_legacy(message, wait_id).await
    }

    async fn post_streamable(
        &mut self,
        message: &Value,
        wait_id: Option<&JsonRpcId>,
    ) -> io::Result<PostOutcome> {
        let body = message.to_string();
        // Only the POST that carries the approved `tools/call` says it was confirmed,
        // never an answer to a request the server sends while it runs.
        let confirmed =
            self.confirmed && message.get("method").and_then(Value::as_str) == Some("tools/call");
        let response = self
            .send_authorized(|this, bearer| {
                let request = this
                    .with_headers(this.client.post(&this.url), bearer)
                    .header(CONTENT_TYPE, "application/json")
                    .header("Accept", "application/json, text/event-stream")
                    .body(body.clone());
                if confirmed {
                    request.header(crate::mcp_zerox::ZEROX_CONFIRMED_HEADER, "1")
                } else {
                    request
                }
            })
            .await?;
        let status = response.status();
        let is_initialize = message.get("method").and_then(Value::as_str) == Some("initialize");
        if status == StatusCode::NOT_FOUND && self.session_id.is_some() {
            self.session_id = None;
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "the MCP session expired",
            ));
        }
        if is_initialize
            && self.may_fall_back
            && matches!(
                status,
                StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
            )
        {
            self.may_fall_back = false;
            return Ok(PostOutcome::TryLegacy);
        }
        if !status.is_success() {
            return Err(io::Error::other(format!(
                "HTTP {} from the MCP server",
                status.as_u16()
            )));
        }
        if is_initialize {
            self.may_fall_back = false;
            if let Some(session) = response
                .headers()
                .get(SESSION_HEADER)
                .and_then(|value| value.to_str().ok())
            {
                self.session_id = Some(session.to_string());
            }
        }
        let Some(wait_id) = wait_id else {
            return Ok(PostOutcome::Reply(None));
        };
        if status == StatusCode::ACCEPTED {
            return Err(invalid_data(
                "the server accepted a request without answering it",
            ));
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if content_type.starts_with("text/event-stream") {
            let mut reader = SseReader::new(response);
            while let Some(event) = reader.next_event().await? {
                if !matches!(event.event.as_deref(), None | Some("message"))
                    || event.data.is_empty()
                {
                    continue;
                }
                for incoming in classify_message(&event.data, wait_id)? {
                    match incoming {
                        Incoming::Reply(reply) => return Ok(PostOutcome::Reply(Some(reply))),
                        Incoming::ServerRequest(answer) => {
                            let _ = Box::pin(self.post_streamable(&answer, None)).await;
                        }
                        Incoming::Other => {}
                    }
                }
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the SSE stream ended before the response",
            ));
        }
        let text = response.text().await.map_err(|error| http_error(&error))?;
        for incoming in classify_message(&text, wait_id)? {
            if let Incoming::Reply(reply) = incoming {
                return Ok(PostOutcome::Reply(Some(reply)));
            }
        }
        Err(invalid_data(
            "the server's answer holds no response to the request",
        ))
    }

    /// Opens the HTTP+SSE event stream and waits for its `endpoint` event.
    async fn ensure_legacy_stream(&mut self) -> io::Result<()> {
        if self.legacy.is_some() {
            return Ok(());
        }
        let response = self
            .send_authorized(|this, bearer| {
                this.with_headers(this.client.get(&this.url), bearer)
                    .header("Accept", "text/event-stream")
            })
            .await?;
        if !response.status().is_success() {
            return Err(io::Error::other(format!(
                "HTTP {} opening the MCP event stream",
                response.status().as_u16()
            )));
        }
        let mut reader = SseReader::new(response);
        let endpoint = loop {
            match reader.next_event().await? {
                Some(event) if event.event.as_deref() == Some("endpoint") => break event.data,
                Some(_) => {}
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the event stream closed before naming its endpoint",
                    ))
                }
            }
        };
        let base = Url::parse(&self.url).map_err(|error| invalid_data(error.to_string()))?;
        let post_url = base
            .join(endpoint.trim())
            .map_err(|error| invalid_data(format!("invalid endpoint event: {error}")))?;
        if post_url.origin() != base.origin() {
            return Err(invalid_data(
                "the event stream named an endpoint on another origin",
            ));
        }
        self.legacy = Some(LegacyStream {
            reader,
            post_url: post_url.to_string(),
        });
        Ok(())
    }

    async fn post_legacy(&mut self, post_url: &str, message: &Value) -> io::Result<()> {
        let body = message.to_string();
        let response = self
            .send_authorized(|this, bearer| {
                this.with_headers(this.client.post(post_url), bearer)
                    .header(CONTENT_TYPE, "application/json")
                    .body(body.clone())
            })
            .await?;
        if response.status().is_success() {
            return Ok(());
        }
        let kind = if response.status() == StatusCode::NOT_FOUND {
            io::ErrorKind::ConnectionReset
        } else {
            io::ErrorKind::Other
        };
        Err(io::Error::new(
            kind,
            format!("HTTP {} from the MCP server", response.status().as_u16()),
        ))
    }

    async fn send_legacy(
        &mut self,
        message: &Value,
        wait_id: Option<&JsonRpcId>,
    ) -> io::Result<Option<Value>> {
        self.ensure_legacy_stream().await?;
        let post_url = self
            .legacy
            .as_ref()
            .map(|stream| stream.post_url.clone())
            .unwrap_or_default();
        if let Err(error) = self.post_legacy(&post_url, message).await {
            self.legacy = None;
            return Err(error);
        }
        let Some(wait_id) = wait_id else {
            return Ok(None);
        };
        // Taken while reading, so answering the server can borrow `self`; put back
        // only when the stream is still good.
        let Some(mut stream) = self.legacy.take() else {
            return Err(invalid_data("the event stream is gone"));
        };
        loop {
            let Some(event) = stream.reader.next_event().await? else {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the event stream closed before the response",
                ));
            };
            if !matches!(event.event.as_deref(), None | Some("message")) || event.data.is_empty() {
                continue;
            }
            for incoming in classify_message(&event.data, wait_id)? {
                match incoming {
                    Incoming::Reply(reply) => {
                        self.legacy = Some(stream);
                        return Ok(Some(reply));
                    }
                    Incoming::ServerRequest(answer) => {
                        let _ = self.post_legacy(&stream.post_url, &answer).await;
                    }
                    Incoming::Other => {}
                }
            }
        }
    }

    /// Ends the session: `DELETE` with its id (Streamable HTTP), or closes the stream.
    async fn shutdown(&mut self) {
        self.legacy = None;
        if self.flavor != Flavor::Streamable {
            return;
        }
        if self.session_id.is_none() {
            return;
        }
        let request = self.send_authorized(|this, bearer| {
            this.with_headers(this.client.delete(&this.url), bearer)
        });
        let _ = tokio::time::timeout(DELETE_WAIT, request).await;
        self.session_id = None;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Mutex};

    use serde_json::json;

    use super::expand_env_refs;
    use crate::config::{
        ConfigSource, McpRemoteServerConfig, McpServerConfig, ScopedMcpServerConfig,
    };
    use crate::mcp_oauth::tests::{install_memory_store, sample_tokens};
    use crate::mcp_oauth::{save_mcp_tokens, unix_now};
    use crate::mcp_stdio::McpServerManager;
    use crate::mcp_test_http::{mcp_reply, FakeRequest, FakeResponse, FakeServer};
    use crate::mcp_zerox::tests::ZeroxTestGateway;
    use crate::mcp_zerox::zerox_needs_connect;

    fn remote(kind: &str, url: &str, headers: &[(&str, &str)]) -> ScopedMcpServerConfig {
        let config = McpRemoteServerConfig {
            url: url.to_string(),
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
            headers_helper: None,
            oauth: None,
        };
        ScopedMcpServerConfig {
            scope: ConfigSource::User,
            config: if kind == "sse" {
                McpServerConfig::Sse(config)
            } else {
                McpServerConfig::Http(config)
            },
        }
    }

    fn manager(name: &str, server: ScopedMcpServerConfig) -> McpServerManager {
        McpServerManager::from_servers(&BTreeMap::from([(name.to_string(), server)]))
    }

    /// A Streamable HTTP server: JSON answers (or SSE when `sse`), a session id.
    fn streamable_server(sse: bool) -> FakeServer {
        FakeServer::spawn(Arc::new(move |request: &FakeRequest| {
            if request.method == "DELETE" {
                return FakeResponse::status(200);
            }
            if request.method != "POST" || request.path() != "/mcp" {
                return FakeResponse::status(405);
            }
            let message = request.json();
            let is_initialize = message["method"] == "initialize";
            if !is_initialize && request.header("mcp-session-id") != Some("session-1") {
                return FakeResponse::status(400);
            }
            match mcp_reply(&message) {
                None => FakeResponse::status(202),
                Some(reply) => {
                    let response = if sse {
                        // A server notification first, then the response.
                        FakeResponse::sse(&[
                            json!({"jsonrpc": "2.0", "method": "notifications/message", "params": {}}),
                            reply,
                        ])
                    } else {
                        FakeResponse::json(200, &reply)
                    };
                    if is_initialize {
                        response.with_header("Mcp-Session-Id", "session-1")
                    } else {
                        response
                    }
                }
            }
        }))
    }

    fn exercise(manager: &mut McpServerManager, name: &str) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let report = runtime.block_on(manager.discover_tools_best_effort());
        assert!(
            report.failed_servers.is_empty(),
            "{:?}",
            report.failed_servers
        );
        assert!(report.unsupported_servers.is_empty());
        let names: Vec<_> = report
            .tools
            .iter()
            .map(|tool| tool.qualified_name.clone())
            .collect();
        assert_eq!(names, vec![format!("mcp__{name}__echo")]);
        let response = runtime
            .block_on(manager.call_tool(&format!("mcp__{name}__echo"), Some(json!({"text": "hi"}))))
            .unwrap();
        let result = response.result.unwrap();
        assert_eq!(result.content[0].data["text"], "echo:hi");
        let resources = runtime.block_on(manager.list_resources(name)).unwrap();
        assert_eq!(resources.resources[0].uri, "file://remote/readme");
        let read = runtime
            .block_on(manager.read_resource(name, "file://remote/readme"))
            .unwrap();
        assert_eq!(read.contents[0].text.as_deref(), Some("remote hello"));
        runtime.block_on(manager.shutdown()).unwrap();
    }

    #[test]
    fn streamable_http_with_json_answers_discovers_calls_and_ends_the_session() {
        std::env::set_var("NEURON_TEST_MCP_HEADER", "header-secret");
        let server = streamable_server(false);
        let mut manager = manager(
            "remote-json",
            remote(
                "http",
                &format!("{}/mcp", server.url),
                &[("X-Api-Key", "${NEURON_TEST_MCP_HEADER}")],
            ),
        );
        exercise(&mut manager, "remote-json");
        let requests = server.requests();
        let posts: Vec<_> = requests.iter().filter(|r| r.method == "POST").collect();
        assert!(posts
            .iter()
            .all(|r| r.header("x-api-key") == Some("header-secret")));
        assert!(posts.iter().all(|r| r
            .header("accept")
            .is_some_and(|accept| accept.contains("application/json")
                && accept.contains("text/event-stream"))));
        // Notification answered 202; protocol version sent after initialize.
        assert!(posts[1..]
            .iter()
            .all(|r| r.header("mcp-protocol-version") == Some("2025-06-18")));
        let delete = requests
            .iter()
            .find(|r| r.method == "DELETE")
            .expect("DELETE sent");
        assert_eq!(delete.header("mcp-session-id"), Some("session-1"));
    }

    #[test]
    fn streamable_http_with_sse_answers_works() {
        let server = streamable_server(true);
        let mut manager = manager(
            "remote-sse-answers",
            remote("http", &format!("{}/mcp", server.url), &[]),
        );
        exercise(&mut manager, "remote-sse-answers");
    }

    #[test]
    fn an_expired_session_is_started_again() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let server = FakeServer::spawn(Arc::new(move |request: &FakeRequest| {
            let message = request.json();
            if message["method"] == "initialize" {
                let n = counter.fetch_add(1, Ordering::SeqCst);
                return FakeResponse::json(200, &mcp_reply(&message).unwrap())
                    .with_header("Mcp-Session-Id", &format!("s{n}"));
            }
            // The first session expires once tools were listed.
            if request.header("mcp-session-id") == Some("s0") && message["method"] == "tools/call" {
                return FakeResponse::status(404);
            }
            match mcp_reply(&message) {
                None => FakeResponse::status(202),
                Some(reply) => FakeResponse::json(200, &reply),
            }
        }));
        let mut manager = manager(
            "remote-expiry",
            remote("http", &format!("{}/mcp", server.url), &[]),
        );
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let report = runtime.block_on(manager.discover_tools_best_effort());
        assert_eq!(report.tools.len(), 1);
        let first = runtime
            .block_on(manager.call_tool("mcp__remote-expiry__echo", Some(json!({"text": "a"}))));
        assert!(first.is_err(), "the expired session fails the call");
        let second = runtime
            .block_on(manager.call_tool("mcp__remote-expiry__echo", Some(json!({"text": "b"}))))
            .unwrap();
        assert_eq!(second.result.unwrap().content[0].data["text"], "echo:b");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    /// An HTTP+SSE server: GET /sse streams, POST /messages answers on the stream.
    fn legacy_server(stream_path: &'static str) -> FakeServer {
        let (sender, receiver) = mpsc::channel::<serde_json::Value>();
        let receiver = Arc::new(Mutex::new(receiver));
        let sender = Arc::new(Mutex::new(sender));
        FakeServer::spawn(Arc::new(move |request: &FakeRequest| {
            match (request.method.as_str(), request.path()) {
                ("GET", path) if path == stream_path => {
                    let receiver = Arc::clone(&receiver);
                    FakeResponse::Stream(Box::new(move |mut stream| {
                        use std::io::Write;
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\nevent: endpoint\ndata: /messages?session=abc\n\n"
                        );
                        let _ = stream.flush();
                        let receiver = receiver.lock().unwrap();
                        while let Ok(message) = receiver.recv() {
                            if write!(stream, "event: message\ndata: {message}\n\n").is_err() {
                                break;
                            }
                            let _ = stream.flush();
                        }
                    }))
                }
                ("POST", "/messages") if request.target.contains("session=abc") => {
                    if let Some(reply) = mcp_reply(&request.json()) {
                        sender.lock().unwrap().send(reply).unwrap();
                    }
                    FakeResponse::status(202)
                }
                _ => FakeResponse::status(405),
            }
        }))
    }

    #[test]
    fn legacy_sse_servers_work() {
        let server = legacy_server("/sse");
        let mut manager = manager(
            "remote-legacy",
            remote("sse", &format!("{}/sse", server.url), &[]),
        );
        exercise(&mut manager, "remote-legacy");
    }

    #[test]
    fn http_servers_that_only_speak_sse_fall_back() {
        // POST to the URL gets 405, then a GET there opens the legacy stream.
        let server = legacy_server("/mcp");
        let mut manager = manager(
            "remote-fallback",
            remote("http", &format!("{}/mcp", server.url), &[]),
        );
        exercise(&mut manager, "remote-fallback");
    }

    #[test]
    fn oauth_tokens_are_sent_and_401_without_tokens_asks_for_login() {
        install_memory_store();
        let server = FakeServer::spawn(Arc::new(|request: &FakeRequest| {
            if request.header("authorization") != Some("Bearer access-1") {
                return FakeResponse::status(401).with_header(
                    "WWW-Authenticate",
                    r#"Bearer resource_metadata="http://x/.well-known/oauth-protected-resource""#,
                );
            }
            match mcp_reply(&request.json()) {
                None => FakeResponse::status(202),
                Some(reply) => FakeResponse::json(200, &reply),
            }
        }));
        let url = format!("{}/mcp", server.url);

        let mut locked = manager("remote-locked", remote("http", &url, &[]));
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let report = runtime.block_on(locked.discover_tools_best_effort());
        assert_eq!(report.failed_servers.len(), 1);
        assert!(
            report.failed_servers[0]
                .error
                .contains("neuron mcp login remote-locked"),
            "{}",
            report.failed_servers[0].error
        );

        let mut tokens = sample_tokens(&url, "http://127.0.0.1:9/token");
        tokens.expires_at = Some(unix_now() + 3600);
        save_mcp_tokens("remote-authed", &tokens).unwrap();
        let mut authed = manager("remote-authed", remote("http", &url, &[]));
        let report = runtime.block_on(authed.discover_tools_best_effort());
        assert!(
            report.failed_servers.is_empty(),
            "{:?}",
            report.failed_servers
        );
        assert_eq!(report.tools.len(), 1);
    }

    #[test]
    fn expands_env_references_in_header_values() {
        std::env::set_var("NEURON_TEST_EXPAND", "v1");
        assert_eq!(expand_env_refs("Bearer ${NEURON_TEST_EXPAND}"), "Bearer v1");
        assert_eq!(expand_env_refs("${NEURON_TEST_UNSET_XYZ}x"), "x");
        assert_eq!(expand_env_refs("plain $HOME ${open"), "plain $HOME ${open");
    }

    #[test]
    fn debug_output_hides_header_values() {
        let connection = super::McpRemoteConnection::new(
            "debug",
            &crate::mcp_client::McpRemoteTransport {
                url: "https://mcp.example.com/mcp".into(),
                headers: BTreeMap::from([("Authorization".into(), "Bearer hidden".into())]),
                headers_helper: None,
                auth: crate::mcp_client::McpClientAuth::None,
            },
            super::Flavor::Streamable,
        )
        .unwrap();
        assert!(!format!("{connection:?}").contains("hidden"));
    }

    fn hosted(id: &str) -> ScopedMcpServerConfig {
        ScopedMcpServerConfig {
            scope: ConfigSource::Managed,
            config: McpServerConfig::ManagedProxy(crate::config::McpManagedProxyServerConfig {
                url: String::new(),
                id: id.to_string(),
            }),
        }
    }

    /// The gateway's `/mcp/<id>`: `Bearer ses_linked` reaches a server with one read and
    /// one destructive GitHub tool; `ses_unlinked` has not linked the connector; any
    /// other sign-in is refused. A tool call is answered on an SSE stream that pings the
    /// client before the result.
    fn gateway() -> FakeServer {
        let origin = Arc::new(Mutex::new(String::new()));
        let server_origin = Arc::clone(&origin);
        let server = FakeServer::spawn(Arc::new(move |request: &FakeRequest| {
            if !request.path().starts_with("/mcp/") {
                return FakeResponse::status(404);
            }
            match request.header("authorization") {
                Some("Bearer ses_linked") => {}
                Some("Bearer ses_unlinked") => {
                    let connect_url =
                        format!("{}/dashboard#connectors", server_origin.lock().unwrap());
                    return FakeResponse::json(
                        403,
                        &json!({"error": "needs_connect", "connect_url": connect_url}),
                    );
                }
                _ => return FakeResponse::status(401),
            }
            if request.method == "DELETE" {
                return FakeResponse::status(200);
            }
            let message = request.json();
            if message["method"] == "tools/list" {
                let tool = |name: &str| json!({"name": name, "inputSchema": {"type": "object"}});
                return FakeResponse::json(
                    200,
                    &json!({"jsonrpc": "2.0", "id": message["id"], "result": {
                        "tools": [tool("get_me"), tool("delete_repository")]
                    }}),
                );
            }
            match mcp_reply(&message) {
                None => FakeResponse::status(202),
                Some(reply) if message["method"] == "tools/call" => FakeResponse::sse(&[
                    json!({"jsonrpc": "2.0", "id": "gateway-ping", "method": "ping"}),
                    reply,
                ]),
                Some(reply) => FakeResponse::json(200, &reply),
            }
        }));
        *origin.lock().unwrap() = server.url.clone();
        server
    }

    #[test]
    fn a_linked_hosted_connector_is_ready_with_the_zerox_sign_in() {
        let server = gateway();
        let _gateway = ZeroxTestGateway::new(&server.url, Some("ses_linked"));
        let mut manager = manager("github", hosted("github"));
        let runtime = tokio::runtime::Runtime::new().unwrap();

        let report = runtime.block_on(manager.discover_tools_best_effort());

        assert!(
            report.failed_servers.is_empty(),
            "{:?}",
            report.failed_servers
        );
        assert!(report.unsupported_servers.is_empty());
        assert_eq!(report.tools.len(), 2);
        assert_eq!(zerox_needs_connect("github"), None);
        runtime.block_on(manager.shutdown()).unwrap();
        let requests = server.requests();
        assert!(requests
            .iter()
            .all(|request| request.path() == "/mcp/github"
                && request.header("authorization") == Some("Bearer ses_linked")));
    }

    #[test]
    fn an_unlinked_hosted_connector_needs_connect_until_it_answers() {
        let server = gateway();
        let gateway_guard = ZeroxTestGateway::new(&server.url, Some("ses_unlinked"));
        let mut manager = manager("hosted-unlinked", hosted("github"));
        let runtime = tokio::runtime::Runtime::new().unwrap();

        let report = runtime.block_on(manager.discover_tools_best_effort());

        let connect_url = format!("{}/dashboard#connectors", server.url);
        assert_eq!(report.failed_servers.len(), 1);
        assert!(
            report.failed_servers[0]
                .error
                .contains(&format!("connect it at {connect_url}")),
            "{}",
            report.failed_servers[0].error
        );
        assert_eq!(
            zerox_needs_connect("hosted-unlinked").as_deref(),
            Some(connect_url.as_str())
        );

        // Linked on the dashboard: the next start clears the mark.
        drop(gateway_guard);
        let _linked = ZeroxTestGateway::new(&server.url, Some("ses_linked"));
        let mut linked = self::manager("hosted-unlinked", hosted("github"));
        let report = runtime.block_on(linked.discover_tools_best_effort());
        assert!(report.failed_servers.is_empty());
        assert_eq!(zerox_needs_connect("hosted-unlinked"), None);
    }

    #[test]
    fn a_refused_zerox_sign_in_asks_to_sign_in_again() {
        let server = gateway();
        let expired = ZeroxTestGateway::new(&server.url, Some("ses_expired"));
        let mut manager = manager("hosted-expired", hosted("github"));
        let runtime = tokio::runtime::Runtime::new().unwrap();

        let report = runtime.block_on(manager.discover_tools_best_effort());

        assert_eq!(report.failed_servers.len(), 1);
        let error = &report.failed_servers[0].error;
        assert!(
            error.contains("run `neuron login` to sign in again"),
            "{error}"
        );
        assert!(!error.contains("ses_expired"), "{error}");
        // The sign-in is read again after the first 401 before giving up.
        assert!(server.requests().len() >= 2);

        drop(expired);
        let _signed_out = ZeroxTestGateway::new(&server.url, None);
        let mut signed_out = self::manager("hosted-signed-out", hosted("github"));
        let report = runtime.block_on(signed_out.discover_tools_best_effort());
        assert!(report.failed_servers[0]
            .error
            .contains("not signed in to Zero-X"));
    }

    #[test]
    fn only_user_approved_destructive_calls_are_confirmed_to_the_gateway() {
        let server = gateway();
        let _gateway = ZeroxTestGateway::new(&server.url, Some("ses_linked"));
        let mut manager = manager("github", hosted("github"));
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(manager.discover_tools_best_effort());

        let mut confirmed = |tool: &str, approved: bool| {
            let before = server.requests().len();
            runtime
                .block_on(manager.call_tool_approved(
                    &format!("mcp__github__{tool}"),
                    Some(json!({})),
                    approved,
                ))
                .unwrap();
            let requests = server.requests();
            let call = requests[before..]
                .iter()
                .find(|request| request.json()["method"] == "tools/call")
                .expect("tools/call sent")
                .clone();
            call.header("x-neuron-confirmed").map(str::to_string)
        };

        assert_eq!(confirmed("delete_repository", true).as_deref(), Some("1"));
        // Allowed by a rule or the mode, not by the user: not confirmed.
        assert_eq!(confirmed("delete_repository", false), None);
        // A read-tier tool is never confirmed.
        assert_eq!(confirmed("get_me", true), None);
        // Nothing else carries it, the answers to the gateway's pings during a
        // confirmed call included.
        let requests = server.requests();
        let ping_answers = requests
            .iter()
            .filter(|request| request.json()["id"] == "gateway-ping")
            .collect::<Vec<_>>();
        assert_eq!(ping_answers.len(), 3, "one per tool call");
        assert!(requests
            .iter()
            .filter(|request| request.json()["method"] != "tools/call")
            .all(|request| request.header("x-neuron-confirmed").is_none()));
    }

    #[test]
    fn other_servers_are_never_told_a_call_was_confirmed() {
        let server = streamable_server(false);
        let mut manager = manager(
            "remote-plain",
            remote("http", &format!("{}/mcp", server.url), &[]),
        );
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(manager.discover_tools_best_effort());
        runtime
            .block_on(manager.call_tool_approved(
                "mcp__remote-plain__echo",
                Some(json!({"text": "x"})),
                true,
            ))
            .unwrap();
        assert!(server
            .requests()
            .iter()
            .all(|request| request.header("x-neuron-confirmed").is_none()));
    }

    #[test]
    fn hosted_connectors_stay_unsupported_while_switched_off() {
        let server = gateway();
        let _gateway = ZeroxTestGateway::off(&server.url);
        let mut manager = manager("github", hosted("github"));
        let runtime = tokio::runtime::Runtime::new().unwrap();

        let report = runtime.block_on(manager.discover_tools_best_effort());

        assert_eq!(report.unsupported_servers.len(), 1);
        assert!(report.unsupported_servers[0]
            .reason
            .contains("NEURON_HOSTED_CONNECTORS=1"));
        assert!(server.requests().is_empty(), "nothing reaches the gateway");
    }
}
