//! Optional OpenTelemetry export over OTLP/HTTP with JSON bodies.
//!
//! [`OtlpExporter`] is a [`TelemetrySink`]: install it behind a [`SessionTracer`] and the
//! runtime's session trace records turn into spans and metrics:
//!
//! | span                  | from                                                        |
//! |-----------------------|-------------------------------------------------------------|
//! | `neuron.turn`         | `turn_started` .. `turn_completed` / `turn_failed`          |
//! | `neuron.model_request`| `model_request_started` .. `assistant_iteration_completed`  |
//! | `neuron.tool_call`    | `tool_execution_started` .. `tool_execution_finished`       |
//!
//! Metrics (delta temporality): `neuron.tokens` (sum, by `token.type` and model),
//! `neuron.model_request.duration`, `neuron.tool_call.duration` and
//! `neuron.turn.duration` (histograms in milliseconds).
//!
//! The encoder is hand-rolled (`serde_json`) instead of the `opentelemetry` crate tree,
//! which would add dozens of crates for three span kinds. Prompt and response text are
//! never exported unless [`OtelConfig::log_content`] is on: attributes that carry text
//! (`user_input`, `error`, ...) are dropped.
//!
//! Configuration ([`OtelConfig::from_sources`]): `OTEL_EXPORTER_OTLP_ENDPOINT` (and the
//! per-signal `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` / `..._METRICS_ENDPOINT`),
//! `OTEL_EXPORTER_OTLP_HEADERS`, `OTEL_SERVICE_NAME`, `OTEL_SDK_DISABLED`, and the
//! `telemetry.otel` setting (`true`, an endpoint string, or
//! `{"enabled", "endpoint", "serviceName", "logContent"}`).
//!
//! The built-in [`PlainHttpTransport`] speaks plain `http://` only; callers that have a
//! TLS client pass their own [`OtlpTransport`] for `https://` collectors.
//!
//! [`SessionTracer`]: crate::SessionTracer

use std::collections::hash_map::RandomState;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::hash::{BuildHasher, Hasher};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::{SessionTraceRecord, TelemetryEvent, TelemetrySink};

/// Where a collector listens for OTLP/HTTP when only "on" was configured.
pub const DEFAULT_OTLP_ENDPOINT: &str = "http://localhost:4318";
/// The `service.name` resource attribute unless `OTEL_SERVICE_NAME` or the setting
/// names another.
pub const DEFAULT_SERVICE_NAME: &str = "neuron";
/// How often finished spans and metrics are sent.
pub const DEFAULT_EXPORT_INTERVAL: Duration = Duration::from_secs(2);
/// Spans that trigger an export before the interval ends.
pub const MAX_BATCH_SPANS: usize = 256;
/// Spans kept while the collector is unreachable; older ones are dropped.
pub const MAX_PENDING_SPANS: usize = 4096;

/// Histogram bucket bounds, in milliseconds.
const DURATION_BOUNDS_MS: [f64; 12] = [
    10.0, 50.0, 100.0, 250.0, 500.0, 1_000.0, 2_500.0, 5_000.0, 10_000.0, 30_000.0, 60_000.0,
    120_000.0,
];

/// Trace attributes that can carry prompt, response or tool text. Dropped unless
/// [`OtelConfig::log_content`] is on.
const CONTENT_ATTRIBUTES: [&str; 8] = [
    "user_input",
    "prompt",
    "error",
    "input",
    "output",
    "text",
    "content",
    "message",
];

/// What the exporter sends and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtelConfig {
    /// Full URL spans are sent to (`.../v1/traces`).
    pub traces_url: String,
    /// Full URL metrics are sent to (`.../v1/metrics`).
    pub metrics_url: String,
    /// Extra request headers (`OTEL_EXPORTER_OTLP_HEADERS`), typically auth. Never logged.
    pub headers: Vec<(String, String)>,
    pub service_name: String,
    pub service_version: String,
    /// Export prompt/response/tool text attributes too. Off by default.
    pub log_content: bool,
    pub export_interval: Duration,
}

impl OtelConfig {
    /// A config sending to `endpoint` (a base URL such as `http://localhost:4318`).
    #[must_use]
    pub fn for_endpoint(endpoint: &str) -> Self {
        Self {
            traces_url: signal_url(endpoint, "traces"),
            metrics_url: signal_url(endpoint, "metrics"),
            headers: Vec::new(),
            service_name: DEFAULT_SERVICE_NAME.to_string(),
            service_version: env!("CARGO_PKG_VERSION").to_string(),
            log_content: false,
            export_interval: DEFAULT_EXPORT_INTERVAL,
        }
    }

    /// The export configuration from the environment (`env`) and the `telemetry.otel`
    /// setting (`setting`), or `None` when export is off. Export is on when
    /// `OTEL_EXPORTER_OTLP_ENDPOINT` is set or the setting turns it on; the setting set
    /// to `false` (or `OTEL_SDK_DISABLED=true`) turns it off whatever the environment
    /// says.
    pub fn from_sources(
        env: &dyn Fn(&str) -> Option<String>,
        setting: Option<&Value>,
    ) -> Result<Option<Self>, String> {
        let env = |name: &str| env(name).filter(|value| !value.trim().is_empty());
        if env("OTEL_SDK_DISABLED").is_some_and(|value| value.trim().eq_ignore_ascii_case("true")) {
            return Ok(None);
        }
        let setting = OtelSetting::parse(setting)?;
        if setting.enabled == Some(false) {
            return Ok(None);
        }
        let env_endpoint = env("OTEL_EXPORTER_OTLP_ENDPOINT");
        let traces_override = env("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT");
        let metrics_override = env("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT");
        let enabled = env_endpoint.is_some()
            || traces_override.is_some()
            || metrics_override.is_some()
            || setting.enabled == Some(true);
        if !enabled {
            return Ok(None);
        }
        if let Some(protocol) = env("OTEL_EXPORTER_OTLP_PROTOCOL") {
            if protocol.trim() != "http/json" && protocol.trim() != "http/protobuf" {
                return Err(format!(
                    "OTEL_EXPORTER_OTLP_PROTOCOL={} is not supported; Neuron exports OTLP/HTTP with JSON bodies (http/json)",
                    protocol.trim()
                ));
            }
        }
        let endpoint = env_endpoint
            .or(setting.endpoint)
            .unwrap_or_else(|| DEFAULT_OTLP_ENDPOINT.to_string());
        let mut config = Self::for_endpoint(endpoint.trim());
        if let Some(url) = traces_override {
            config.traces_url = url.trim().to_string();
        }
        if let Some(url) = metrics_override {
            config.metrics_url = url.trim().to_string();
        }
        for url in [&config.traces_url, &config.metrics_url] {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(format!(
                    "OTLP endpoint must start with http:// or https:// (got {})",
                    redact_url(url)
                ));
            }
        }
        if let Some(headers) = env("OTEL_EXPORTER_OTLP_HEADERS") {
            config.headers = parse_headers(&headers)?;
        }
        if let Some(name) = env("OTEL_SERVICE_NAME").or(setting.service_name) {
            config.service_name = name.trim().to_string();
        }
        config.log_content = setting.log_content
            || env("NEURON_OTEL_LOG_CONTENT")
                .is_some_and(|value| matches!(value.trim(), "1" | "true" | "yes"));
        Ok(Some(config))
    }

    /// [`Self::from_sources`] with the process environment.
    pub fn from_env(setting: Option<&Value>) -> Result<Option<Self>, String> {
        Self::from_sources(&|name| std::env::var(name).ok(), setting)
    }
}

#[derive(Debug, Default)]
struct OtelSetting {
    enabled: Option<bool>,
    endpoint: Option<String>,
    service_name: Option<String>,
    log_content: bool,
}

impl OtelSetting {
    fn parse(value: Option<&Value>) -> Result<Self, String> {
        let Some(value) = value else {
            return Ok(Self::default());
        };
        match value {
            Value::Null => Ok(Self::default()),
            Value::Bool(enabled) => Ok(Self {
                enabled: Some(*enabled),
                ..Self::default()
            }),
            Value::String(endpoint) => Ok(Self {
                enabled: Some(true),
                endpoint: Some(endpoint.clone()),
                ..Self::default()
            }),
            Value::Object(object) => Ok(Self {
                enabled: Some(
                    object
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                ),
                endpoint: object
                    .get("endpoint")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                service_name: object
                    .get("serviceName")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                log_content: object
                    .get("logContent")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            }),
            _ => {
                Err("telemetry.otel must be true, false, an endpoint URL or an object".to_string())
            }
        }
    }
}

fn signal_url(endpoint: &str, signal: &str) -> String {
    format!("{}/v1/{signal}", endpoint.trim().trim_end_matches('/'))
}

/// `scheme://host[:port]` of `url`, without a path, query or user info, for messages.
fn redact_url(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let host = rest.split('/').next().unwrap_or_default();
    let host = host.rsplit('@').next().unwrap_or_default();
    if scheme.is_empty() {
        host.to_string()
    } else {
        format!("{scheme}://{host}")
    }
}

/// `key=value,key2=value2` with percent-encoded values (the OTLP exporter format).
fn parse_headers(raw: &str) -> Result<Vec<(String, String)>, String> {
    let mut headers = Vec::new();
    for pair in raw.split(',').filter(|pair| !pair.trim().is_empty()) {
        let Some((key, value)) = pair.split_once('=') else {
            return Err("OTEL_EXPORTER_OTLP_HEADERS must be key=value pairs".to_string());
        };
        let key = key.trim().to_string();
        let value = percent_decode(value.trim());
        let invalid = |text: &str| text.chars().any(|ch| ch == '\r' || ch == '\n');
        if key.is_empty() || invalid(&key) || invalid(&value) {
            return Err("OTEL_EXPORTER_OTLP_HEADERS has an invalid header".to_string());
        }
        headers.push((key, value));
    }
    Ok(headers)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let digits = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(digits, 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Sends one OTLP/HTTP JSON request.
pub trait OtlpTransport: Send + Sync {
    /// POSTs `body` (JSON) to `url` with `headers`; the HTTP status on a response.
    fn post_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<u16, String>;
}

/// HTTP/1.1 over a plain TCP connection: `http://` URLs only.
#[derive(Debug, Clone, Copy)]
pub struct PlainHttpTransport {
    pub connect_timeout: Duration,
    pub io_timeout: Duration,
}

impl Default for PlainHttpTransport {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(2),
            io_timeout: Duration::from_secs(5),
        }
    }
}

impl OtlpTransport for PlainHttpTransport {
    fn post_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<u16, String> {
        let rest = url.strip_prefix("http://").ok_or_else(|| {
            format!(
                "{} needs a TLS transport; only http:// is built in",
                redact_url(url)
            )
        })?;
        let (authority, path) = match rest.find('/') {
            Some(slash) => (&rest[..slash], &rest[slash..]),
            None => (rest, "/"),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !host.ends_with(']') || authority.starts_with('[') => (
                host,
                port.parse::<u16>()
                    .map_err(|_| format!("bad port in {}", redact_url(url)))?,
            ),
            _ => (authority, 80),
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let addresses = (host, port)
            .to_socket_addrs()
            .map_err(|error| format!("cannot resolve {}: {error}", redact_url(url)))?;
        let mut stream = None;
        let mut last_error = String::from("no address");
        for address in addresses {
            match TcpStream::connect_timeout(&address, self.connect_timeout) {
                Ok(connected) => {
                    stream = Some(connected);
                    break;
                }
                Err(error) => last_error = error.to_string(),
            }
        }
        let mut stream =
            stream.ok_or_else(|| format!("cannot connect to {}: {last_error}", redact_url(url)))?;
        let _ = stream.set_read_timeout(Some(self.io_timeout));
        let _ = stream.set_write_timeout(Some(self.io_timeout));
        let mut request = format!(
            "POST {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        for (key, value) in headers {
            let _ = write!(request, "{key}: {value}\r\n");
        }
        request.push_str("\r\n");
        stream
            .write_all(request.as_bytes())
            .and_then(|()| stream.write_all(body))
            .and_then(|()| stream.flush())
            .map_err(|error| format!("send to {} failed: {error}", redact_url(url)))?;
        let mut response = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    response.extend_from_slice(&buffer[..read]);
                    if response.len() > 64 * 1024 {
                        break;
                    }
                }
                Err(error) if response.is_empty() => {
                    return Err(format!("no response from {}: {error}", redact_url(url)));
                }
                Err(_) => break,
            }
        }
        let status_line = String::from_utf8_lossy(&response);
        status_line
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse::<u16>().ok())
            .ok_or_else(|| format!("bad response from {}", redact_url(url)))
    }
}

/// A typed span attribute value.
#[derive(Debug, Clone, PartialEq)]
pub enum AttrValue {
    Str(String),
    Int(i64),
    Double(f64),
    Bool(bool),
}

impl AttrValue {
    fn from_json(value: &Value) -> Option<Self> {
        match value {
            Value::String(text) => Some(Self::Str(text.clone())),
            Value::Bool(flag) => Some(Self::Bool(*flag)),
            Value::Number(number) => number
                .as_i64()
                .map(Self::Int)
                .or_else(|| number.as_f64().map(Self::Double)),
            _ => None,
        }
    }

    fn to_otlp(&self) -> Value {
        match self {
            Self::Str(text) => json!({ "stringValue": text }),
            // OTLP/JSON encodes 64-bit integers as decimal strings.
            Self::Int(number) => json!({ "intValue": number.to_string() }),
            Self::Double(number) => json!({ "doubleValue": number }),
            Self::Bool(flag) => json!({ "boolValue": flag }),
        }
    }
}

fn otlp_attributes(attributes: &[(String, AttrValue)]) -> Value {
    Value::Array(
        attributes
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": value.to_otlp() }))
            .collect(),
    )
}

/// A finished span, ready to encode.
#[derive(Debug, Clone, PartialEq)]
pub struct FinishedSpan {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub name: String,
    /// OTLP span kind: 1 internal, 3 client.
    pub kind: u8,
    pub start_ns: u64,
    pub end_ns: u64,
    pub attributes: Vec<(String, AttrValue)>,
    /// `Some` when the span failed; the message only with content logging on.
    pub error: Option<String>,
}

impl FinishedSpan {
    fn to_otlp(&self) -> Value {
        let mut span = json!({
            "traceId": self.trace_id,
            "spanId": self.span_id,
            "name": self.name,
            "kind": self.kind,
            "startTimeUnixNano": self.start_ns.to_string(),
            "endTimeUnixNano": self.end_ns.to_string(),
            "attributes": otlp_attributes(&self.attributes),
            "status": match &self.error {
                None => json!({ "code": 1 }),
                Some(message) if message.is_empty() => json!({ "code": 2 }),
                Some(message) => json!({ "code": 2, "message": message }),
            },
        });
        if let Some(parent) = &self.parent_span_id {
            span["parentSpanId"] = json!(parent);
        }
        span
    }
}

fn resource(config: &OtelConfig) -> Value {
    json!({
        "attributes": otlp_attributes(&[
            ("service.name".to_string(), AttrValue::Str(config.service_name.clone())),
            ("service.version".to_string(), AttrValue::Str(config.service_version.clone())),
            ("telemetry.sdk.name".to_string(), AttrValue::Str("neuron".to_string())),
            ("telemetry.sdk.language".to_string(), AttrValue::Str("rust".to_string())),
        ])
    })
}

fn scope(config: &OtelConfig) -> Value {
    json!({ "name": "neuron", "version": config.service_version })
}

/// The `ExportTraceServiceRequest` body for `spans`.
#[must_use]
pub fn encode_traces(config: &OtelConfig, spans: &[FinishedSpan]) -> Value {
    json!({
        "resourceSpans": [{
            "resource": resource(config),
            "scopeSpans": [{
                "scope": scope(config),
                "spans": spans.iter().map(FinishedSpan::to_otlp).collect::<Vec<_>>(),
            }],
        }],
    })
}

/// A delta histogram of durations in milliseconds.
#[derive(Debug, Clone, PartialEq)]
struct Histogram {
    count: u64,
    sum: f64,
    min: f64,
    max: f64,
    buckets: Vec<u64>,
}

impl Histogram {
    fn new() -> Self {
        Self {
            count: 0,
            sum: 0.0,
            min: f64::MAX,
            max: 0.0,
            buckets: vec![0; DURATION_BOUNDS_MS.len() + 1],
        }
    }

    fn observe(&mut self, value: f64) {
        self.count += 1;
        self.sum += value;
        self.min = self.min.min(value);
        self.max = self.max.max(value);
        let bucket = DURATION_BOUNDS_MS
            .iter()
            .position(|bound| value <= *bound)
            .unwrap_or(DURATION_BOUNDS_MS.len());
        self.buckets[bucket] += 1;
    }
}

/// Metric points gathered since the last export.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MetricsSnapshot {
    /// `(token.type, model)` -> tokens.
    tokens: BTreeMap<(String, String), u64>,
    /// `(metric name, attribute key, attribute value)` -> histogram.
    durations: BTreeMap<(String, String, String), Histogram>,
}

impl MetricsSnapshot {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty() && self.durations.is_empty()
    }

    fn add_tokens(&mut self, kind: &str, model: &str, tokens: u64) {
        if tokens > 0 {
            *self
                .tokens
                .entry((kind.to_string(), model.to_string()))
                .or_default() += tokens;
        }
    }

    fn observe(&mut self, metric: &str, key: &str, value: &str, millis: f64) {
        self.durations
            .entry((metric.to_string(), key.to_string(), value.to_string()))
            .or_insert_with(Histogram::new)
            .observe(millis);
    }

    /// Tokens recorded for `kind` (`input`, `output`, ...), all models.
    #[must_use]
    pub fn tokens(&self, kind: &str) -> u64 {
        self.tokens
            .iter()
            .filter(|((token_kind, _), _)| token_kind == kind)
            .map(|(_, tokens)| tokens)
            .sum()
    }
}

/// The `ExportMetricsServiceRequest` body for `snapshot`, covering `start_ns..end_ns`.
#[must_use]
pub fn encode_metrics(
    config: &OtelConfig,
    snapshot: &MetricsSnapshot,
    start_ns: u64,
    end_ns: u64,
) -> Value {
    let mut metrics = Vec::new();
    if !snapshot.tokens.is_empty() {
        let points = snapshot
            .tokens
            .iter()
            .map(|((kind, model), tokens)| {
                json!({
                    "attributes": otlp_attributes(&[
                        ("token.type".to_string(), AttrValue::Str(kind.clone())),
                        ("model".to_string(), AttrValue::Str(model.clone())),
                    ]),
                    "startTimeUnixNano": start_ns.to_string(),
                    "timeUnixNano": end_ns.to_string(),
                    "asInt": tokens.to_string(),
                })
            })
            .collect::<Vec<_>>();
        metrics.push(json!({
            "name": "neuron.tokens",
            "description": "Tokens used by model requests",
            "unit": "{token}",
            // 1 = AGGREGATION_TEMPORALITY_DELTA
            "sum": { "aggregationTemporality": 1, "isMonotonic": true, "dataPoints": points },
        }));
    }
    let mut by_metric: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for ((metric, key, value), histogram) in &snapshot.durations {
        by_metric.entry(metric.as_str()).or_default().push(json!({
            "attributes": otlp_attributes(&[(key.clone(), AttrValue::Str(value.clone()))]),
            "startTimeUnixNano": start_ns.to_string(),
            "timeUnixNano": end_ns.to_string(),
            "count": histogram.count.to_string(),
            "sum": histogram.sum,
            "min": histogram.min,
            "max": histogram.max,
            "bucketCounts": histogram.buckets.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "explicitBounds": DURATION_BOUNDS_MS,
        }));
    }
    for (metric, points) in by_metric {
        metrics.push(json!({
            "name": metric,
            "unit": "ms",
            "histogram": { "aggregationTemporality": 1, "dataPoints": points },
        }));
    }
    json!({
        "resourceMetrics": [{
            "resource": resource(config),
            "scopeMetrics": [{ "scope": scope(config), "metrics": metrics }],
        }],
    })
}

#[derive(Debug, Clone)]
struct OpenSpan {
    span_id: String,
    start_ns: u64,
    attributes: Vec<(String, AttrValue)>,
}

#[derive(Debug, Default)]
struct SessionState {
    trace_id: Option<String>,
    turn: Option<OpenSpan>,
    model_request: Option<OpenSpan>,
    tools: Vec<(String, OpenSpan)>,
    /// When the last model request or tool call ended: where a model request starts
    /// when no `model_request_started` record came.
    last_boundary_ns: u64,
}

/// Turns session trace records into finished spans and metric points.
#[derive(Debug, Default)]
pub struct SpanAssembler {
    log_content: bool,
    sessions: HashMap<String, SessionState>,
}

impl SpanAssembler {
    #[must_use]
    pub fn new(log_content: bool) -> Self {
        Self {
            log_content,
            sessions: HashMap::new(),
        }
    }

    /// Feeds one record; finished spans go to `spans`, metric points to `metrics`.
    #[allow(clippy::too_many_lines)]
    pub fn observe(
        &mut self,
        record: &SessionTraceRecord,
        spans: &mut Vec<FinishedSpan>,
        metrics: &mut MetricsSnapshot,
    ) {
        let now_ns = record.timestamp_ms.saturating_mul(1_000_000);
        let log_content = self.log_content;
        let attributes = |extra: &Map<String, Value>| -> Vec<(String, AttrValue)> {
            let mut out = vec![(
                "session.id".to_string(),
                AttrValue::Str(record.session_id.clone()),
            )];
            for (key, value) in extra {
                if !log_content && CONTENT_ATTRIBUTES.contains(&key.as_str()) {
                    continue;
                }
                if let Some(value) = AttrValue::from_json(value) {
                    out.push((format!("neuron.{key}"), value));
                }
            }
            out
        };
        let state = self.sessions.entry(record.session_id.clone()).or_default();
        match record.name.as_str() {
            "turn_started" => {
                // A turn that never closed (a crash, a lost record) is closed now.
                close_dangling(state, now_ns, spans);
                state.trace_id = Some(hex(&random_bytes::<16>()));
                state.turn = Some(OpenSpan {
                    span_id: hex(&random_bytes::<8>()),
                    start_ns: now_ns,
                    attributes: attributes(&record.attributes),
                });
                state.last_boundary_ns = now_ns;
            }
            "model_request_started" => {
                ensure_turn(state, now_ns, &record.session_id);
                state.model_request = Some(OpenSpan {
                    span_id: hex(&random_bytes::<8>()),
                    start_ns: now_ns,
                    attributes: attributes(&record.attributes),
                });
            }
            "assistant_iteration_completed" => {
                ensure_turn(state, now_ns, &record.session_id);
                let open = state.model_request.take().unwrap_or_else(|| OpenSpan {
                    span_id: hex(&random_bytes::<8>()),
                    start_ns: state.last_boundary_ns,
                    attributes: Vec::new(),
                });
                let model = record
                    .attributes
                    .get("served_model")
                    .or_else(|| record.attributes.get("model"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                for (kind, key) in [
                    ("input", "input_tokens"),
                    ("output", "output_tokens"),
                    ("cache_read", "cache_read_input_tokens"),
                    ("cache_creation", "cache_creation_input_tokens"),
                ] {
                    if let Some(tokens) = record.attributes.get(key).and_then(Value::as_u64) {
                        metrics.add_tokens(kind, &model, tokens);
                    }
                }
                let mut span_attributes = open.attributes;
                span_attributes.extend(
                    attributes(&record.attributes)
                        .into_iter()
                        .filter(|(key, _)| key != "session.id"),
                );
                metrics.observe(
                    "neuron.model_request.duration",
                    "model",
                    &model,
                    millis_between(open.start_ns, now_ns),
                );
                spans.push(child_span(
                    state,
                    "neuron.model_request",
                    3,
                    open.span_id,
                    open.start_ns,
                    now_ns,
                    span_attributes,
                    None,
                ));
                state.last_boundary_ns = now_ns;
            }
            "tool_execution_started" => {
                ensure_turn(state, now_ns, &record.session_id);
                let name = tool_name(&record.attributes);
                state.tools.push((
                    name,
                    OpenSpan {
                        span_id: hex(&random_bytes::<8>()),
                        start_ns: now_ns,
                        attributes: attributes(&record.attributes),
                    },
                ));
            }
            "tool_execution_finished" => {
                ensure_turn(state, now_ns, &record.session_id);
                let name = tool_name(&record.attributes);
                let open = state
                    .tools
                    .iter()
                    .rposition(|(open_name, _)| *open_name == name)
                    .map(|index| state.tools.remove(index).1);
                // A call that never ran (denied) has no start: a zero-length span.
                let open = open.unwrap_or_else(|| OpenSpan {
                    span_id: hex(&random_bytes::<8>()),
                    start_ns: now_ns,
                    attributes: attributes(&Map::new()),
                });
                let failed = record
                    .attributes
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let mut span_attributes = open.attributes;
                span_attributes.retain(|(key, _)| key != "neuron.tool_name");
                span_attributes.push(("tool.name".to_string(), AttrValue::Str(name.clone())));
                span_attributes.push(("neuron.is_error".to_string(), AttrValue::Bool(failed)));
                metrics.observe(
                    "neuron.tool_call.duration",
                    "tool.name",
                    &name,
                    millis_between(open.start_ns, now_ns),
                );
                spans.push(child_span(
                    state,
                    "neuron.tool_call",
                    1,
                    open.span_id,
                    open.start_ns,
                    now_ns,
                    span_attributes,
                    failed.then(String::new),
                ));
                state.last_boundary_ns = now_ns;
            }
            "turn_completed" | "turn_failed" => {
                let failed = record.name == "turn_failed";
                let error = failed.then(|| {
                    if log_content {
                        record
                            .attributes
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    } else {
                        String::new()
                    }
                });
                // A failed model request ends with the turn.
                if let Some(open) = state.model_request.take() {
                    spans.push(child_span(
                        state,
                        "neuron.model_request",
                        3,
                        open.span_id,
                        open.start_ns,
                        now_ns,
                        open.attributes,
                        error.clone(),
                    ));
                }
                close_tools(state, now_ns, spans);
                if let Some(turn) = state.turn.take() {
                    let mut turn_attributes = turn.attributes;
                    turn_attributes.extend(
                        attributes(&record.attributes)
                            .into_iter()
                            .filter(|(key, _)| key != "session.id"),
                    );
                    metrics.observe(
                        "neuron.turn.duration",
                        "outcome",
                        if failed { "failed" } else { "completed" },
                        millis_between(turn.start_ns, now_ns),
                    );
                    spans.push(FinishedSpan {
                        trace_id: state.trace_id.clone().unwrap_or_default(),
                        span_id: turn.span_id,
                        parent_span_id: None,
                        name: "neuron.turn".to_string(),
                        kind: 1,
                        start_ns: turn.start_ns,
                        end_ns: now_ns,
                        attributes: turn_attributes,
                        error,
                    });
                }
                state.trace_id = None;
            }
            _ => {}
        }
    }
}

fn tool_name(attributes: &Map<String, Value>) -> String {
    attributes
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string()
}

fn millis_between(start_ns: u64, end_ns: u64) -> f64 {
    // Durations of a turn fit an f64 exactly in practice.
    #[allow(clippy::cast_precision_loss)]
    let millis = end_ns.saturating_sub(start_ns) as f64 / 1_000_000.0;
    millis
}

/// Records before any `turn_started` (a sub-agent's runtime, a resumed turn) still get
/// a turn to hang from.
fn ensure_turn(state: &mut SessionState, now_ns: u64, session_id: &str) {
    if state.turn.is_none() {
        state.trace_id = Some(hex(&random_bytes::<16>()));
        state.turn = Some(OpenSpan {
            span_id: hex(&random_bytes::<8>()),
            start_ns: now_ns,
            attributes: vec![(
                "session.id".to_string(),
                AttrValue::Str(session_id.to_string()),
            )],
        });
        state.last_boundary_ns = now_ns;
    }
}

fn close_tools(state: &mut SessionState, now_ns: u64, spans: &mut Vec<FinishedSpan>) {
    for (name, open) in std::mem::take(&mut state.tools) {
        let mut attributes = open.attributes;
        attributes.push(("tool.name".to_string(), AttrValue::Str(name)));
        spans.push(child_span(
            state,
            "neuron.tool_call",
            1,
            open.span_id,
            open.start_ns,
            now_ns,
            attributes,
            Some(String::new()),
        ));
    }
}

fn close_dangling(state: &mut SessionState, now_ns: u64, spans: &mut Vec<FinishedSpan>) {
    if let Some(open) = state.model_request.take() {
        spans.push(child_span(
            state,
            "neuron.model_request",
            3,
            open.span_id,
            open.start_ns,
            now_ns,
            open.attributes,
            Some(String::new()),
        ));
    }
    close_tools(state, now_ns, spans);
    if let Some(turn) = state.turn.take() {
        spans.push(FinishedSpan {
            trace_id: state.trace_id.clone().unwrap_or_default(),
            span_id: turn.span_id,
            parent_span_id: None,
            name: "neuron.turn".to_string(),
            kind: 1,
            start_ns: turn.start_ns,
            end_ns: now_ns,
            attributes: turn.attributes,
            error: Some(String::new()),
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn child_span(
    state: &SessionState,
    name: &str,
    kind: u8,
    span_id: String,
    start_ns: u64,
    end_ns: u64,
    attributes: Vec<(String, AttrValue)>,
    error: Option<String>,
) -> FinishedSpan {
    FinishedSpan {
        trace_id: state.trace_id.clone().unwrap_or_default(),
        span_id,
        parent_span_id: state.turn.as_ref().map(|turn| turn.span_id.clone()),
        name: name.to_string(),
        kind,
        start_ns,
        end_ns: end_ns.max(start_ns),
        attributes,
        error,
    }
}

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0_u8; N];
    for chunk in out.chunks_mut(8) {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(ID_COUNTER.fetch_add(1, Ordering::Relaxed));
        hasher.write_u128(now_ns().into());
        hasher.write_u32(std::process::id());
        let bytes = hasher.finish().to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
    // An all-zero id is invalid in OTLP.
    if out.iter().all(|byte| *byte == 0) {
        out[0] = 1;
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
        })
}

/// What an exporter has done, for tests and `/status`-style reports.
#[derive(Debug, Default)]
pub struct ExportStats {
    pub exported_spans: AtomicU64,
    pub exported_metric_requests: AtomicU64,
    pub failed_requests: AtomicU64,
    pub dropped_spans: AtomicU64,
}

#[derive(Debug, Default)]
struct Pending {
    spans: Vec<FinishedSpan>,
    metrics: MetricsSnapshot,
    /// Start of the metrics window.
    window_start_ns: u64,
}

struct Inner {
    config: OtelConfig,
    transport: Box<dyn OtlpTransport>,
    assembler: Mutex<SpanAssembler>,
    pending: Mutex<Pending>,
    stats: ExportStats,
}

impl Inner {
    fn export_pending(&self) {
        let (spans, metrics, start_ns) = {
            let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            let start_ns = pending.window_start_ns;
            pending.window_start_ns = now_ns();
            (
                std::mem::take(&mut pending.spans),
                std::mem::take(&mut pending.metrics),
                start_ns,
            )
        };
        if !spans.is_empty() {
            let body = encode_traces(&self.config, &spans).to_string();
            match self.transport.post_json(
                &self.config.traces_url,
                &self.config.headers,
                body.as_bytes(),
            ) {
                Ok(status) if (200..300).contains(&status) => {
                    self.stats
                        .exported_spans
                        .fetch_add(spans.len() as u64, Ordering::Relaxed);
                }
                _ => {
                    self.stats.failed_requests.fetch_add(1, Ordering::Relaxed);
                    self.stats
                        .dropped_spans
                        .fetch_add(spans.len() as u64, Ordering::Relaxed);
                }
            }
        }
        if !metrics.is_empty() {
            let body = encode_metrics(&self.config, &metrics, start_ns, now_ns()).to_string();
            match self.transport.post_json(
                &self.config.metrics_url,
                &self.config.headers,
                body.as_bytes(),
            ) {
                Ok(status) if (200..300).contains(&status) => {
                    self.stats
                        .exported_metric_requests
                        .fetch_add(1, Ordering::Relaxed);
                }
                _ => {
                    self.stats.failed_requests.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

enum Control {
    Wake,
    Flush(Sender<()>),
    Shutdown(Sender<()>),
}

/// A [`TelemetrySink`] that exports spans and metrics to an OTLP/HTTP collector from a
/// background thread. Recording never blocks on the network; export failures are
/// counted in [`Self::stats`], never printed (the full-screen UI owns the terminal).
pub struct OtlpExporter {
    inner: Arc<Inner>,
    control: Mutex<Option<Sender<Control>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for OtlpExporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // No headers: they carry credentials.
        f.debug_struct("OtlpExporter")
            .field("traces_url", &redact_url(&self.inner.config.traces_url))
            .finish_non_exhaustive()
    }
}

impl OtlpExporter {
    /// Starts an exporter with the built-in plain-HTTP transport.
    #[must_use]
    pub fn new(config: OtelConfig) -> Self {
        Self::with_transport(config, Box::new(PlainHttpTransport::default()))
    }

    /// Starts an exporter that sends through `transport`.
    #[must_use]
    pub fn with_transport(config: OtelConfig, transport: Box<dyn OtlpTransport>) -> Self {
        let interval = config.export_interval;
        let inner = Arc::new(Inner {
            assembler: Mutex::new(SpanAssembler::new(config.log_content)),
            config,
            transport,
            pending: Mutex::new(Pending {
                window_start_ns: now_ns(),
                ..Pending::default()
            }),
            stats: ExportStats::default(),
        });
        let (sender, receiver) = mpsc::channel::<Control>();
        let worker_inner = Arc::clone(&inner);
        let worker = thread::Builder::new()
            .name("neuron-otel".to_string())
            .spawn(move || loop {
                match receiver.recv_timeout(interval) {
                    Ok(Control::Wake) | Err(RecvTimeoutError::Timeout) => {
                        worker_inner.export_pending();
                    }
                    Ok(Control::Flush(done)) => {
                        worker_inner.export_pending();
                        let _ = done.send(());
                    }
                    Ok(Control::Shutdown(done)) => {
                        worker_inner.export_pending();
                        let _ = done.send(());
                        return;
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        worker_inner.export_pending();
                        return;
                    }
                }
            })
            .ok();
        Self {
            inner,
            control: Mutex::new(Some(sender)),
            worker: Mutex::new(worker),
        }
    }

    #[must_use]
    pub fn config(&self) -> &OtelConfig {
        &self.inner.config
    }

    #[must_use]
    pub fn stats(&self) -> &ExportStats {
        &self.inner.stats
    }

    fn send(&self, control: Control) -> bool {
        self.control
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|sender| sender.send(control).is_ok())
    }

    /// Sends everything finished so far and waits (up to `timeout`) until it went.
    pub fn flush(&self, timeout: Duration) -> bool {
        let (done, wait) = mpsc::channel();
        self.send(Control::Flush(done)) && wait.recv_timeout(timeout).is_ok()
    }

    /// Sends what is left and stops the export thread (waiting up to `timeout`).
    /// Later records are dropped.
    pub fn shutdown(&self, timeout: Duration) {
        let (done, wait) = mpsc::channel();
        if self.send(Control::Shutdown(done)) {
            let _ = wait.recv_timeout(timeout);
        }
        self.control
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}

impl Drop for OtlpExporter {
    fn drop(&mut self) {
        self.shutdown(Duration::from_secs(2));
    }
}

impl TelemetrySink for OtlpExporter {
    fn record(&self, event: TelemetryEvent) {
        let TelemetryEvent::SessionTrace(record) = event else {
            return;
        };
        let mut spans = Vec::new();
        let mut metrics = MetricsSnapshot::default();
        self.inner
            .assembler
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .observe(&record, &mut spans, &mut metrics);
        if spans.is_empty() && metrics.is_empty() {
            return;
        }
        let wake = {
            let mut pending = self
                .inner
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            pending.spans.extend(spans);
            if pending.spans.len() > MAX_PENDING_SPANS {
                let excess = pending.spans.len() - MAX_PENDING_SPANS;
                pending.spans.drain(..excess);
                self.inner
                    .stats
                    .dropped_spans
                    .fetch_add(excess as u64, Ordering::Relaxed);
            }
            for (key, tokens) in metrics.tokens {
                *pending.metrics.tokens.entry(key).or_default() += tokens;
            }
            for (key, histogram) in metrics.durations {
                let merged = pending
                    .metrics
                    .durations
                    .entry(key)
                    .or_insert_with(Histogram::new);
                merged.count += histogram.count;
                merged.sum += histogram.sum;
                merged.min = merged.min.min(histogram.min);
                merged.max = merged.max.max(histogram.max);
                for (total, count) in merged.buckets.iter_mut().zip(histogram.buckets) {
                    *total += count;
                }
            }
            pending.spans.len() >= MAX_BATCH_SPANS
        };
        if wake {
            self.send(Control::Wake);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionTracer;
    use std::net::TcpListener;

    fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<Vec<_>>();
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn export_is_off_without_endpoint_or_setting() {
        let config = OtelConfig::from_sources(&env_from(&[]), None).unwrap();
        assert_eq!(config, None);
    }

    #[test]
    fn env_endpoint_turns_export_on_with_signal_paths() {
        let env = env_from(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318/"),
            ("OTEL_SERVICE_NAME", "ci-agent"),
            (
                "OTEL_EXPORTER_OTLP_HEADERS",
                "authorization=Bearer%20abc,x-team=core",
            ),
        ]);
        let config = OtelConfig::from_sources(&env, None).unwrap().unwrap();
        assert_eq!(config.traces_url, "http://collector:4318/v1/traces");
        assert_eq!(config.metrics_url, "http://collector:4318/v1/metrics");
        assert_eq!(config.service_name, "ci-agent");
        assert_eq!(
            config.headers,
            vec![
                ("authorization".to_string(), "Bearer abc".to_string()),
                ("x-team".to_string(), "core".to_string()),
            ]
        );
        assert!(!config.log_content);
        // Debug output never shows the headers.
        let exporter = OtlpExporter::new(config);
        assert!(!format!("{exporter:?}").contains("Bearer"));
    }

    #[test]
    fn setting_turns_export_on_off_and_configures_it() {
        let on = OtelConfig::from_sources(&env_from(&[]), Some(&json!(true)))
            .unwrap()
            .unwrap();
        assert_eq!(on.traces_url, "http://localhost:4318/v1/traces");

        let object =
            json!({ "endpoint": "http://otel:4318", "serviceName": "svc", "logContent": true });
        let configured = OtelConfig::from_sources(&env_from(&[]), Some(&object))
            .unwrap()
            .unwrap();
        assert_eq!(configured.metrics_url, "http://otel:4318/v1/metrics");
        assert_eq!(configured.service_name, "svc");
        assert!(configured.log_content);

        let env = env_from(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318")]);
        assert_eq!(
            OtelConfig::from_sources(&env, Some(&json!(false))).unwrap(),
            None
        );
        let disabled = env_from(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318"),
            ("OTEL_SDK_DISABLED", "true"),
        ]);
        assert_eq!(OtelConfig::from_sources(&disabled, None).unwrap(), None);
        assert!(OtelConfig::from_sources(&env_from(&[]), Some(&json!(3))).is_err());
        let grpc = env_from(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4317"),
            ("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc"),
        ]);
        assert!(OtelConfig::from_sources(&grpc, None).is_err());
    }

    fn record(name: &str, timestamp_ms: u64, attributes: &Value) -> SessionTraceRecord {
        SessionTraceRecord {
            session_id: "session-1".to_string(),
            sequence: 0,
            name: name.to_string(),
            timestamp_ms,
            attributes: attributes.as_object().cloned().unwrap_or_default(),
        }
    }

    #[test]
    fn assembler_builds_turn_model_and_tool_spans_and_drops_prompt_text() {
        let mut assembler = SpanAssembler::new(false);
        let mut spans = Vec::new();
        let mut metrics = MetricsSnapshot::default();
        for record in [
            record(
                "turn_started",
                1_000,
                &json!({ "user_input": "secret prompt" }),
            ),
            record("model_request_started", 1_010, &json!({ "iteration": 1 })),
            record(
                "assistant_iteration_completed",
                1_510,
                &json!({ "iteration": 1, "input_tokens": 120, "output_tokens": 30, "served_model": "m1" }),
            ),
            record(
                "tool_execution_started",
                1_520,
                &json!({ "tool_name": "bash" }),
            ),
            record(
                "tool_execution_finished",
                1_720,
                &json!({ "tool_name": "bash", "is_error": true }),
            ),
            record("turn_completed", 2_000, &json!({ "iterations": 2 })),
        ] {
            assembler.observe(&record, &mut spans, &mut metrics);
        }
        let names = spans
            .iter()
            .map(|span| span.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            ["neuron.model_request", "neuron.tool_call", "neuron.turn"]
        );
        let turn = &spans[2];
        assert_eq!(turn.parent_span_id, None);
        assert_eq!(turn.start_ns, 1_000_000_000);
        assert_eq!(turn.end_ns, 2_000_000_000);
        for child in &spans[..2] {
            assert_eq!(child.trace_id, turn.trace_id);
            assert_eq!(child.parent_span_id.as_deref(), Some(turn.span_id.as_str()));
        }
        assert_eq!(spans[0].start_ns, 1_010_000_000);
        assert_eq!(spans[1].error, Some(String::new()));
        assert_eq!(turn.trace_id.len(), 32);
        assert_eq!(turn.span_id.len(), 16);
        let encoded = encode_traces(&OtelConfig::for_endpoint("http://x"), &spans).to_string();
        assert!(!encoded.contains("secret prompt"));
        assert_eq!(metrics.tokens("input"), 120);
        assert_eq!(metrics.tokens("output"), 30);
    }

    #[test]
    fn content_logging_keeps_prompt_text() {
        let mut assembler = SpanAssembler::new(true);
        let mut spans = Vec::new();
        let mut metrics = MetricsSnapshot::default();
        assembler.observe(
            &record("turn_started", 1, &json!({ "user_input": "hello" })),
            &mut spans,
            &mut metrics,
        );
        assembler.observe(
            &record("turn_failed", 2, &json!({ "error": "boom" })),
            &mut spans,
            &mut metrics,
        );
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].error.as_deref(), Some("boom"));
        assert!(spans[0].attributes.contains(&(
            "neuron.user_input".to_string(),
            AttrValue::Str("hello".to_string())
        )));
    }

    #[test]
    fn metrics_encode_as_delta_sum_and_histograms() {
        let mut metrics = MetricsSnapshot::default();
        metrics.add_tokens("input", "m", 5);
        metrics.observe("neuron.tool_call.duration", "tool.name", "bash", 75.0);
        let body = encode_metrics(&OtelConfig::for_endpoint("http://x"), &metrics, 1, 2);
        let list = &body["resourceMetrics"][0]["scopeMetrics"][0]["metrics"];
        assert_eq!(list[0]["name"], "neuron.tokens");
        assert_eq!(list[0]["sum"]["dataPoints"][0]["asInt"], "5");
        assert_eq!(list[1]["name"], "neuron.tool_call.duration");
        let point = &list[1]["histogram"]["dataPoints"][0];
        assert_eq!(point["count"], "1");
        // 75 ms falls in the (50, 100] bucket.
        assert_eq!(point["bucketCounts"][2], "1");
    }

    /// A fake OTLP/HTTP collector: records each request's path and body.
    struct FakeCollector {
        url: String,
        requests: Arc<Mutex<Vec<(String, String, String)>>>,
    }

    impl FakeCollector {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let seen = Arc::clone(&requests);
            thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { break };
                    let mut data = Vec::new();
                    let mut buffer = [0_u8; 4096];
                    let (head, body) = loop {
                        let read = stream.read(&mut buffer).unwrap_or(0);
                        if read == 0 {
                            break (String::from_utf8_lossy(&data).into_owned(), String::new());
                        }
                        data.extend_from_slice(&buffer[..read]);
                        let text = String::from_utf8_lossy(&data).into_owned();
                        if let Some(end) = text.find("\r\n\r\n") {
                            let head = text[..end].to_string();
                            let length = head
                                .lines()
                                .find_map(|line| {
                                    line.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|value| value.trim().parse::<usize>().unwrap())
                                })
                                .unwrap_or(0);
                            while data.len() < end + 4 + length {
                                let read = stream.read(&mut buffer).unwrap_or(0);
                                if read == 0 {
                                    break;
                                }
                                data.extend_from_slice(&buffer[..read]);
                            }
                            let body = String::from_utf8_lossy(&data[end + 4..]).into_owned();
                            break (head, body);
                        }
                    };
                    let path = head
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_string();
                    seen.lock().unwrap().push((path, head, body));
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
                    );
                }
            });
            Self { url, requests }
        }
    }

    #[test]
    fn exporter_sends_spans_and_metrics_to_a_collector() {
        let collector = FakeCollector::start();
        let mut config = OtelConfig::for_endpoint(&collector.url);
        config.headers = vec![("x-api-key".to_string(), "k".to_string())];
        let exporter = Arc::new(OtlpExporter::new(config));
        let tracer = SessionTracer::new("session-9", exporter.clone());
        let mut attributes = Map::new();
        attributes.insert("user_input".to_string(), json!("do not export me"));
        tracer.record("turn_started", attributes);
        tracer.record("model_request_started", Map::new());
        let mut usage = Map::new();
        usage.insert("input_tokens".to_string(), json!(10));
        usage.insert("output_tokens".to_string(), json!(4));
        tracer.record("assistant_iteration_completed", usage);
        let mut tool = Map::new();
        tool.insert("tool_name".to_string(), json!("read_file"));
        tracer.record("tool_execution_started", tool.clone());
        tracer.record("tool_execution_finished", tool);
        tracer.record("turn_completed", Map::new());
        assert!(exporter.flush(Duration::from_secs(10)));

        let requests = collector.requests.lock().unwrap().clone();
        let trace_request = requests
            .iter()
            .find(|(path, _, _)| path == "/v1/traces")
            .expect("a trace export");
        assert!(trace_request
            .1
            .to_ascii_lowercase()
            .contains("x-api-key: k"));
        assert!(!trace_request.2.contains("do not export me"));
        let body: Value = serde_json::from_str(&trace_request.2).unwrap();
        let spans = body["resourceSpans"][0]["scopeSpans"][0]["spans"]
            .as_array()
            .unwrap();
        assert_eq!(spans.len(), 3);
        let metrics = requests
            .iter()
            .find(|(path, _, _)| path == "/v1/metrics")
            .expect("a metrics export");
        assert!(metrics.2.contains("neuron.tokens"));
        assert_eq!(exporter.stats().exported_spans.load(Ordering::Relaxed), 3);
        assert_eq!(exporter.stats().failed_requests.load(Ordering::Relaxed), 0);
        exporter.shutdown(Duration::from_secs(5));
    }

    #[test]
    fn unreachable_collector_counts_failures_without_panicking() {
        // Bind then drop: nothing listens on the port.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let exporter = Arc::new(OtlpExporter::new(OtelConfig::for_endpoint(&format!(
            "http://127.0.0.1:{port}"
        ))));
        let tracer = SessionTracer::new("s", exporter.clone());
        tracer.record("turn_started", Map::new());
        tracer.record("turn_completed", Map::new());
        assert!(exporter.flush(Duration::from_secs(10)));
        assert!(exporter.stats().failed_requests.load(Ordering::Relaxed) >= 1);
    }

    #[test]
    fn https_needs_a_tls_transport() {
        let error = PlainHttpTransport::default()
            .post_json("https://user:pw@collector:4318/v1/traces", &[], b"{}")
            .unwrap_err();
        assert!(error.contains("https://collector:4318"));
        assert!(!error.contains("pw"));
    }
}
