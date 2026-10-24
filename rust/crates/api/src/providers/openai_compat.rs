use std::collections::{BTreeMap, VecDeque};
use std::fmt::{Debug, Formatter};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{ApiError, GatewayError, GatewayErrorCode};
use crate::http_client::build_http_client_or_default;
use crate::retry::{
    retry_after_from_headers, RetryBudget, RetryNotice, RetryObserver, RetryProgress,
    CAPACITY_MAX_WAIT, CAPACITY_TOTAL_BACKOFF, MAX_TOTAL_BACKOFF,
};
use crate::types::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    InputContentBlock, InputMessage, MessageDelta, MessageDeltaEvent, MessageRequest,
    MessageResponse, MessageStartEvent, MessageStopEvent, OutputContentBlock, StreamEvent,
    ToolChoice, ToolDefinition, ToolResultContentBlock, Usage,
};

use super::{preflight_message_request, Provider, ProviderFuture};

pub const DEFAULT_XAI_BASE_URL: &str = "https://api.x.ai/v1";
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_DASHSCOPE_BASE_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";
pub const DEFAULT_ZERO_X_BASE_URL: &str = "https://zero-x.live/v1";
/// Provider name of the Zero-X gateway configuration.
pub const ZERO_X_PROVIDER_NAME: &str = "Zero-X";
const REQUEST_ID_HEADER: &str = "request-id";
const ALT_REQUEST_ID_HEADER: &str = "x-request-id";
const DEFAULT_INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(32);
const DEFAULT_MAX_RETRIES: u32 = 8;
/// The gateway already fails over between upstream pools, so the client only
/// retries transient gateway errors a couple of times (contract §5).
const ZERO_X_MAX_RETRIES: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenAiCompatConfig {
    pub provider_name: &'static str,
    pub api_key_env: &'static str,
    pub base_url_env: &'static str,
    pub default_base_url: &'static str,
    /// Maximum request body size in bytes. Provider-specific limits:
    /// - `DashScope`: 6MB (`6_291_456` bytes) - observed in dogfood testing
    /// - `OpenAI`: 100MB (`104_857_600` bytes)
    /// - `xAI`: 50MB (`52_428_800` bytes)
    pub max_request_body_bytes: usize,
}

const XAI_ENV_VARS: &[&str] = &["XAI_API_KEY"];
const OPENAI_ENV_VARS: &[&str] = &["OPENAI_API_KEY"];
const DASHSCOPE_ENV_VARS: &[&str] = &["DASHSCOPE_API_KEY"];
const ZERO_X_ENV_VARS: &[&str] = &["NEURON_TOKEN"];

// Provider-specific request body size limits in bytes
const XAI_MAX_REQUEST_BODY_BYTES: usize = 52_428_800; // 50MB
const OPENAI_MAX_REQUEST_BODY_BYTES: usize = 104_857_600; // 100MB
const DASHSCOPE_MAX_REQUEST_BODY_BYTES: usize = 6_291_456; // 6MB (observed limit in dogfood)

impl OpenAiCompatConfig {
    #[must_use]
    pub const fn xai() -> Self {
        Self {
            provider_name: "xAI",
            api_key_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: DEFAULT_XAI_BASE_URL,
            max_request_body_bytes: XAI_MAX_REQUEST_BODY_BYTES,
        }
    }

    #[must_use]
    pub const fn openai() -> Self {
        Self {
            provider_name: "OpenAI",
            api_key_env: "OPENAI_API_KEY",
            base_url_env: "OPENAI_BASE_URL",
            default_base_url: DEFAULT_OPENAI_BASE_URL,
            max_request_body_bytes: OPENAI_MAX_REQUEST_BODY_BYTES,
        }
    }

    /// Alibaba `DashScope` compatible-mode endpoint (Qwen family models).
    /// Uses the OpenAI-compatible REST shape at /compatible-mode/v1.
    /// Requested via Discord #clawcode-get-help: native Alibaba API for
    /// higher rate limits than going through `OpenRouter`.
    #[must_use]
    pub const fn dashscope() -> Self {
        Self {
            provider_name: "DashScope",
            api_key_env: "DASHSCOPE_API_KEY",
            base_url_env: "DASHSCOPE_BASE_URL",
            default_base_url: DEFAULT_DASHSCOPE_BASE_URL,
            max_request_body_bytes: DASHSCOPE_MAX_REQUEST_BODY_BYTES,
        }
    }

    /// The Zero-X gateway (`docs/GATEWAY_CONTRACT.md`): `OpenAI` wire format,
    /// actionable error envelopes and a short transient-only retry policy.
    #[must_use]
    pub const fn zero_x() -> Self {
        Self {
            provider_name: ZERO_X_PROVIDER_NAME,
            api_key_env: "NEURON_TOKEN",
            base_url_env: "NEURON_API_BASE",
            default_base_url: DEFAULT_ZERO_X_BASE_URL,
            max_request_body_bytes: OPENAI_MAX_REQUEST_BODY_BYTES,
        }
    }

    #[must_use]
    pub fn is_zero_x(self) -> bool {
        self.provider_name == ZERO_X_PROVIDER_NAME
    }

    #[must_use]
    pub fn credential_env_vars(self) -> &'static [&'static str] {
        match self.provider_name {
            "xAI" => XAI_ENV_VARS,
            "OpenAI" => OPENAI_ENV_VARS,
            "DashScope" => DASHSCOPE_ENV_VARS,
            ZERO_X_PROVIDER_NAME => ZERO_X_ENV_VARS,
            _ => &[],
        }
    }
}

/// Callback told the token usage of every completed request, streamed or
/// not (for example to track a provider's daily quota).
#[derive(Clone)]
pub struct UsageObserver(Arc<dyn Fn(&Usage) + Send + Sync>);

impl UsageObserver {
    pub fn new(observer: impl Fn(&Usage) + Send + Sync + 'static) -> Self {
        Self(Arc::new(observer))
    }

    pub fn notify(&self, usage: &Usage) {
        (self.0)(usage);
    }
}

impl Debug for UsageObserver {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("UsageObserver(..)")
    }
}

/// Two observers are equal when they are the same callback.
impl PartialEq for UsageObserver {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for UsageObserver {}

/// An OpenAI-compatible endpoint chosen by the caller, with its credential
/// held in memory instead of the process environment (where every child
/// process would inherit it).
#[derive(Clone, PartialEq, Eq)]
pub struct OpenAiCompatEndpoint {
    pub config: OpenAiCompatConfig,
    pub base_url: String,
    api_key: String,
    usage_observer: Option<UsageObserver>,
}

impl OpenAiCompatEndpoint {
    #[must_use]
    pub fn new(
        config: OpenAiCompatConfig,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            config,
            base_url: base_url.into(),
            api_key: api_key.into(),
            usage_observer: None,
        }
    }

    /// Reports the usage of every completed request made through clients
    /// built from this endpoint.
    #[must_use]
    pub fn with_usage_observer(mut self, observer: UsageObserver) -> Self {
        self.usage_observer = Some(observer);
        self
    }

    #[must_use]
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    #[must_use]
    pub fn usage_observer(&self) -> Option<&UsageObserver> {
        self.usage_observer.as_ref()
    }
}

impl Debug for OpenAiCompatEndpoint {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatEndpoint")
            .field("provider", &self.config.provider_name)
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field("usage_observer", &self.usage_observer.is_some())
            .finish()
    }
}

fn installed_endpoint_slot() -> &'static RwLock<Option<OpenAiCompatEndpoint>> {
    static SLOT: OnceLock<RwLock<Option<OpenAiCompatEndpoint>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// Installs (or clears) the process-wide OpenAI-compatible endpoint that
/// [`crate::ProviderClient`] uses for OpenAI-kind models instead of reading
/// `OPENAI_API_KEY` / `OPENAI_BASE_URL` from the environment.
pub fn install_openai_compat_endpoint(endpoint: Option<OpenAiCompatEndpoint>) {
    *installed_endpoint_slot()
        .write()
        .unwrap_or_else(PoisonError::into_inner) = endpoint;
}

/// The endpoint installed with [`install_openai_compat_endpoint`], if any.
#[must_use]
pub fn installed_openai_compat_endpoint() -> Option<OpenAiCompatEndpoint> {
    installed_endpoint_slot()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

#[derive(Debug, Clone)]
pub struct OpenAiCompatClient {
    http: reqwest::Client,
    api_key: String,
    config: OpenAiCompatConfig,
    base_url: String,
    max_retries: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
    max_total_backoff: Duration,
    /// Budget for Zero-X `capacity_exhausted` / `rate_limited` waits.
    capacity_budget: RetryBudget,
    retry_observer: Option<RetryObserver>,
    usage_observer: Option<UsageObserver>,
}

/// A failed HTTP exchange plus the `Retry-After` the server sent with it.
struct HttpFailure {
    error: Box<ApiError>,
    retry_after: Option<Duration>,
}

impl From<ApiError> for HttpFailure {
    fn from(error: ApiError) -> Self {
        Self {
            error: Box::new(error),
            retry_after: None,
        }
    }
}

impl OpenAiCompatClient {
    const fn config(&self) -> OpenAiCompatConfig {
        self.config
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
    #[must_use]
    pub fn new(api_key: impl Into<String>, config: OpenAiCompatConfig) -> Self {
        Self {
            http: build_http_client_or_default(),
            api_key: api_key.into(),
            config,
            base_url: read_base_url(config),
            max_retries: if config.is_zero_x() {
                ZERO_X_MAX_RETRIES
            } else {
                DEFAULT_MAX_RETRIES
            },
            initial_backoff: DEFAULT_INITIAL_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
            max_total_backoff: MAX_TOTAL_BACKOFF,
            capacity_budget: RetryBudget {
                total: CAPACITY_TOTAL_BACKOFF,
                max_wait: Some(CAPACITY_MAX_WAIT),
            },
            retry_observer: Some(RetryObserver::stderr()),
            usage_observer: None,
        }
    }

    /// Client for an explicitly chosen endpoint (no environment lookups).
    #[must_use]
    pub fn from_endpoint(endpoint: &OpenAiCompatEndpoint) -> Self {
        let mut client = Self::new(endpoint.api_key.clone(), endpoint.config)
            .with_base_url(endpoint.base_url.clone());
        client.usage_observer.clone_from(&endpoint.usage_observer);
        client
    }

    pub fn from_env(config: OpenAiCompatConfig) -> Result<Self, ApiError> {
        let Some(api_key) = read_env_non_empty(config.api_key_env)? else {
            return Err(ApiError::missing_credentials(
                config.provider_name,
                config.credential_env_vars(),
            ));
        };
        Ok(Self::new(api_key, config))
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    #[must_use]
    pub fn with_retry_policy(
        mut self,
        max_retries: u32,
        initial_backoff: Duration,
        max_backoff: Duration,
    ) -> Self {
        self.max_retries = max_retries;
        self.initial_backoff = initial_backoff;
        self.max_backoff = max_backoff;
        self
    }

    /// Caps the total time spent sleeping between retries of one request.
    #[must_use]
    pub fn with_max_total_backoff(mut self, max_total_backoff: Duration) -> Self {
        self.max_total_backoff = max_total_backoff;
        self
    }

    /// Replaces the budget for Zero-X `capacity_exhausted` and
    /// `rate_limited` waits (default: each `Retry-After` up to
    /// [`CAPACITY_MAX_WAIT`], [`CAPACITY_TOTAL_BACKOFF`] in total). Other
    /// failures and other providers keep [`Self::with_max_total_backoff`].
    #[must_use]
    pub fn with_capacity_retry_budget(mut self, max_wait: Duration, total: Duration) -> Self {
        self.capacity_budget = RetryBudget {
            total,
            max_wait: Some(max_wait),
        };
        self
    }

    /// Replaces the retry progress callback (`None` silences it). The
    /// default prints one line per retry to stderr.
    #[must_use]
    pub fn with_retry_observer(mut self, observer: Option<RetryObserver>) -> Self {
        self.retry_observer = observer;
        self
    }

    #[must_use]
    pub const fn provider_config(&self) -> OpenAiCompatConfig {
        self.config
    }

    pub async fn send_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse, ApiError> {
        let request = MessageRequest {
            stream: false,
            ..request.clone()
        };
        preflight_message_request(&request)?;
        let response = self
            .send_with_retry(&request, &RetryProgress::new())
            .await?;
        record_quota_headers(response.headers());
        let request_id = request_id_from_headers(response.headers());
        let body = response.text().await.map_err(ApiError::from)?;
        // Some backends return {"error":{"message":"...","type":"...","code":...}}
        // instead of a valid completion object. Check for this before attempting
        // full deserialization so the user sees the actual error, not a cryptic
        // "missing field 'id'" parse failure.
        if let Ok(raw) = serde_json::from_str::<serde_json::Value>(&body) {
            if let Some(error) = embedded_error(
                &raw,
                &body,
                request_id.clone(),
                self.config.provider_name,
                "provider returned an error",
            ) {
                return Err(error);
            }
        }
        let payload = serde_json::from_str::<ChatCompletionResponse>(&body).map_err(|error| {
            ApiError::json_deserialize(self.config.provider_name, &request.model, &body, error)
        })?;
        let mut normalized = normalize_response(&request.model, payload)?;
        if normalized.request_id.is_none() {
            normalized.request_id = request_id;
        }
        if let Some(observer) = &self.usage_observer {
            observer.notify(&normalized.usage);
        }
        Ok(normalized)
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageStream, ApiError> {
        self.stream_message_with_progress(request, &RetryProgress::new())
            .await
    }

    /// Like [`Self::stream_message`], continuing the retry count and budget
    /// recorded in `progress` (see [`RetryProgress`]).
    pub async fn stream_message_with_progress(
        &self,
        request: &MessageRequest,
        progress: &RetryProgress,
    ) -> Result<MessageStream, ApiError> {
        preflight_message_request(request)?;
        let response = self
            .send_with_retry(&request.clone().with_streaming(), progress)
            .await?;
        record_quota_headers(response.headers());
        Ok(MessageStream {
            request_id: request_id_from_headers(response.headers()),
            response,
            parser: OpenAiSseParser::with_context(self.config.provider_name, request.model.clone()),
            pending: VecDeque::new(),
            done: false,
            state: StreamState::new(request.model.clone()),
            usage_observer: self.usage_observer.clone(),
        })
    }

    /// Sends `request`, retrying transient failures. Non-retryable errors
    /// (quota, credits, plan, region, suspension, invalid session, bad
    /// request) return immediately. `Retry-After` is honoured and the total
    /// sleep across retries never exceeds `max_total_backoff`, except while
    /// the Zero-X gateway reports `capacity_exhausted` or `rate_limited`,
    /// which uses the larger capacity budget. Requests and waits already
    /// recorded in `progress` count against the same limits.
    async fn send_with_retry(
        &self,
        request: &MessageRequest,
        progress: &RetryProgress,
    ) -> Result<reqwest::Response, ApiError> {
        loop {
            let attempts = progress.start_attempt(self.max_retries);
            let failure = match self.send_raw_request(request).await {
                Ok(response) => match expect_success(response, self.config).await {
                    Ok(response) => return Ok(response),
                    Err(failure) => failure,
                },
                Err(error) => HttpFailure::from(error),
            };
            if !failure.error.is_retryable() {
                return Err(*failure.error);
            }
            let exhausted = |error: Box<ApiError>| {
                if attempts == 1 {
                    *error
                } else {
                    ApiError::RetriesExhausted {
                        attempts,
                        last_error: error,
                    }
                }
            };
            if attempts > self.max_retries {
                return Err(exhausted(failure.error));
            }
            let retry_after = failure.retry_after.or_else(|| failure.error.retry_after());
            let backoff = self.jittered_backoff_for_attempt(attempts)?;
            let Some(delay) =
                self.retry_budget_for(&failure.error)
                    .plan(retry_after, backoff, progress.slept())
            else {
                return Err(exhausted(failure.error));
            };
            let notice = RetryNotice {
                provider: self.config.provider_name.to_string(),
                retry: attempts,
                max_retries: self.max_retries,
                delay,
                reason: retry_reason(&failure.error),
            };
            let observer = self
                .retry_observer
                .as_ref()
                .map(|observer| (observer, &notice));
            progress.wait(delay, observer).await;
        }
    }

    async fn send_raw_request(
        &self,
        request: &MessageRequest,
    ) -> Result<reqwest::Response, ApiError> {
        // Pre-flight check: verify request body size against provider limits
        check_request_body_size(request, self.config())?;

        let request_url = chat_completions_endpoint(&self.base_url);
        self.http
            .post(&request_url)
            .header("content-type", "application/json")
            .bearer_auth(&self.api_key)
            .json(&build_chat_completion_request(request, self.config()))
            .send()
            .await
            .map_err(ApiError::from)
    }

    /// The capacity budget for the Zero-X gateway's shared-pool waits
    /// (`capacity_exhausted`, `rate_limited`); the standard one otherwise.
    fn retry_budget_for(&self, error: &ApiError) -> RetryBudget {
        let capacity_wait = self.config.is_zero_x()
            && error
                .gateway_error()
                .and_then(GatewayError::code_kind)
                .is_some_and(|code| {
                    matches!(
                        code,
                        GatewayErrorCode::CapacityExhausted | GatewayErrorCode::RateLimited
                    )
                });
        if capacity_wait {
            self.capacity_budget
        } else {
            RetryBudget::standard(self.max_total_backoff)
        }
    }

    fn backoff_for_attempt(&self, attempt: u32) -> Result<Duration, ApiError> {
        let Some(multiplier) = 1_u32.checked_shl(attempt.saturating_sub(1)) else {
            return Err(ApiError::BackoffOverflow {
                attempt,
                base_delay: self.initial_backoff,
            });
        };
        Ok(self
            .initial_backoff
            .checked_mul(multiplier)
            .map_or(self.max_backoff, |delay| delay.min(self.max_backoff)))
    }

    fn jittered_backoff_for_attempt(&self, attempt: u32) -> Result<Duration, ApiError> {
        let base = self.backoff_for_attempt(attempt)?;
        Ok(base + jitter_for_base(base))
    }
}

/// Process-wide counter that guarantees distinct jitter samples even when
/// the system clock resolution is coarser than consecutive retry sleeps.
static JITTER_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Returns a random additive jitter in `[0, base]` to decorrelate retries
/// Deserialize a JSON field as a `Vec<T>`, treating an explicit `null` value
/// the same as a missing field (i.e. as an empty vector).
/// Some OpenAI-compatible providers emit `"tool_calls": null` instead of
/// omitting the field or using `[]`, which serde's `#[serde(default)]` alone
/// does not tolerate — `default` only handles absent keys, not null values.
fn deserialize_null_as_empty_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

/// from multiple concurrent clients. Entropy is drawn from the nanosecond
/// wall clock mixed with a monotonic counter and run through a splitmix64
/// finalizer; adequate for retry jitter (no cryptographic requirement).
fn jitter_for_base(base: Duration) -> Duration {
    let base_nanos = u64::try_from(base.as_nanos()).unwrap_or(u64::MAX);
    if base_nanos == 0 {
        return Duration::ZERO;
    }
    let raw_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
        });
    let tick = JITTER_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut mixed = raw_nanos
        .wrapping_add(tick)
        .wrapping_add(0x9E37_79B9_7F4A_7C15);
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    mixed ^= mixed >> 31;
    let jitter_nanos = mixed % base_nanos.saturating_add(1);
    Duration::from_nanos(jitter_nanos)
}

impl Provider for OpenAiCompatClient {
    type Stream = MessageStream;

    fn send_message<'a>(
        &'a self,
        request: &'a MessageRequest,
    ) -> ProviderFuture<'a, MessageResponse> {
        Box::pin(async move { self.send_message(request).await })
    }

    fn stream_message<'a>(
        &'a self,
        request: &'a MessageRequest,
    ) -> ProviderFuture<'a, Self::Stream> {
        Box::pin(async move { self.stream_message(request).await })
    }
}

#[derive(Debug)]
pub struct MessageStream {
    request_id: Option<String>,
    response: reqwest::Response,
    parser: OpenAiSseParser,
    pending: VecDeque<StreamEvent>,
    done: bool,
    state: StreamState,
    usage_observer: Option<UsageObserver>,
}

impl MessageStream {
    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }

            if self.done {
                if self.state.is_truncated(self.parser.saw_done) {
                    return Err(ApiError::StreamTruncated {
                        provider: self.parser.provider.clone(),
                        model: self.state.model.clone(),
                    });
                }
                let finished = self.state.finish()?;
                if let Some(observer) = &self.usage_observer {
                    // `finish` yields the final usage exactly once.
                    for event in &finished {
                        if let StreamEvent::MessageDelta(delta) = event {
                            observer.notify(&delta.usage);
                        }
                    }
                }
                self.pending.extend(finished);
                if let Some(event) = self.pending.pop_front() {
                    return Ok(Some(event));
                }
                return Ok(None);
            }

            if let Some(chunk) = self.response.chunk().await? {
                for parsed in self.parser.push(&chunk)? {
                    self.pending.extend(self.state.ingest_chunk(parsed)?);
                }
            } else {
                for parsed in self.parser.finish()? {
                    self.pending.extend(self.state.ingest_chunk(parsed)?);
                }
                self.done = true;
            }
        }
    }
}

#[derive(Debug, Default)]
struct OpenAiSseParser {
    buffer: Vec<u8>,
    provider: String,
    model: String,
    /// Whether the `data: [DONE]` sentinel has been seen.
    saw_done: bool,
}

impl OpenAiSseParser {
    fn with_context(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            buffer: Vec::new(),
            provider: provider.into(),
            model: model.into(),
            saw_done: false,
        }
    }

    fn push(&mut self, chunk: &[u8]) -> Result<Vec<ChatCompletionChunk>, ApiError> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();

        while let Some(frame) = next_sse_frame(&mut self.buffer) {
            self.accept(
                parse_sse_frame(&frame, &self.provider, &self.model)?,
                &mut events,
            );
        }

        Ok(events)
    }

    fn finish(&mut self) -> Result<Vec<ChatCompletionChunk>, ApiError> {
        let trailing = std::mem::take(&mut self.buffer);
        let mut events = Vec::new();
        let frame = parse_sse_frame(
            &String::from_utf8_lossy(&trailing),
            &self.provider,
            &self.model,
        )?;
        self.accept(frame, &mut events);
        Ok(events)
    }

    fn accept(&mut self, frame: SseFrame, events: &mut Vec<ChatCompletionChunk>) {
        match frame {
            SseFrame::Chunk(chunk) => events.push(chunk),
            SseFrame::Done => self.saw_done = true,
            SseFrame::Skip => {}
        }
    }
}

/// One decoded SSE frame.
#[derive(Debug)]
enum SseFrame {
    /// Comment, keep-alive or empty frame.
    Skip,
    /// The `data: [DONE]` sentinel.
    Done,
    Chunk(ChatCompletionChunk),
}

/// Kind of the streamed prose block (answer text or reasoning) that is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProseKind {
    Text,
    Thinking,
}

#[derive(Debug)]
struct StreamState {
    model: String,
    message_started: bool,
    finished: bool,
    stop_reason: Option<String>,
    usage: Option<Usage>,
    tool_calls: BTreeMap<u32, ToolCallState>,
    /// Index the next content block gets. Blocks are numbered in the order
    /// they start, so text, thinking and tool blocks never share an index.
    next_block_index: u32,
    /// The text or thinking block currently open, with its index. At most
    /// one is open: switching between reasoning and answer text closes the
    /// open block and starts a new one.
    open_prose: Option<(ProseKind, u32)>,
}

impl StreamState {
    fn new(model: String) -> Self {
        Self {
            model,
            message_started: false,
            finished: false,
            stop_reason: None,
            usage: None,
            tool_calls: BTreeMap::new(),
            next_block_index: 0,
            open_prose: None,
        }
    }

    fn allocate_block_index(&mut self) -> u32 {
        let index = self.next_block_index;
        self.next_block_index += 1;
        index
    }

    /// Appends `delta` to the open block of `kind`, first closing an open
    /// block of the other kind and starting a new one when needed.
    fn push_prose_delta(&mut self, kind: ProseKind, delta: String, events: &mut Vec<StreamEvent>) {
        let index = match self.open_prose {
            Some((open_kind, index)) if open_kind == kind => index,
            _ => {
                self.close_prose(events);
                let index = self.allocate_block_index();
                self.open_prose = Some((kind, index));
                let content_block = match kind {
                    ProseKind::Text => OutputContentBlock::Text {
                        text: String::new(),
                    },
                    ProseKind::Thinking => OutputContentBlock::Thinking {
                        thinking: String::new(),
                        signature: None,
                    },
                };
                events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                    index,
                    content_block,
                }));
                index
            }
        };
        let delta = match kind {
            ProseKind::Text => ContentBlockDelta::TextDelta { text: delta },
            ProseKind::Thinking => ContentBlockDelta::ThinkingDelta { thinking: delta },
        };
        events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index,
            delta,
        }));
    }

    fn close_prose(&mut self, events: &mut Vec<StreamEvent>) {
        if let Some((_, index)) = self.open_prose.take() {
            events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                index,
            }));
        }
    }

    fn ingest_chunk(&mut self, chunk: ChatCompletionChunk) -> Result<Vec<StreamEvent>, ApiError> {
        let mut events = Vec::new();
        if !self.message_started {
            self.message_started = true;
            events.push(StreamEvent::MessageStart(MessageStartEvent {
                message: MessageResponse {
                    id: chunk.id.clone(),
                    kind: "message".to_string(),
                    role: "assistant".to_string(),
                    content: Vec::new(),
                    model: chunk.model.clone().unwrap_or_else(|| self.model.clone()),
                    stop_reason: None,
                    stop_sequence: None,
                    usage: Usage {
                        input_tokens: 0,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                        output_tokens: 0,
                    },
                    request_id: None,
                },
            }));
        }

        if let Some(usage) = chunk.usage {
            self.usage = Some(usage.to_usage());
        }

        for choice in chunk.choices {
            let ChunkDelta {
                content,
                reasoning_content,
                reasoning,
                tool_calls,
            } = choice.delta;
            // Reasoning (`reasoning_content`, or `reasoning` as sent by Groq
            // and OpenRouter) streams as its own thinking block, never as
            // answer text. It precedes the answer when a chunk carries both.
            if let Some(thinking) = reasoning_text(reasoning_content, reasoning.as_ref()) {
                self.push_prose_delta(ProseKind::Thinking, thinking, &mut events);
            }
            if let Some(text) = content.filter(|value| !value.is_empty()) {
                self.push_prose_delta(ProseKind::Text, text, &mut events);
            }

            for (position, tool_call) in tool_calls.into_iter().enumerate() {
                let index = self.resolve_tool_call_index(&tool_call, position);
                let state = self.tool_calls.entry(index).or_default();
                state.apply(index, tool_call);
                if !state.started {
                    if state.name.is_none() {
                        continue;
                    }
                    // Reasoning ends where the tool call begins; the answer
                    // text block stays open until the end of the message.
                    if let Some((ProseKind::Thinking, thinking_index)) = self.open_prose {
                        self.open_prose = None;
                        events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                            index: thinking_index,
                        }));
                    }
                    state.block_index = self.next_block_index;
                    self.next_block_index += 1;
                    if let Some(start_event) = state.start_event()? {
                        state.started = true;
                        events.push(StreamEvent::ContentBlockStart(start_event));
                    } else {
                        continue;
                    }
                }
                if let Some(delta_event) = state.delta_event() {
                    events.push(StreamEvent::ContentBlockDelta(delta_event));
                }
                if choice.finish_reason.as_deref() == Some("tool_calls") && !state.stopped {
                    state.stopped = true;
                    events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                        index: state.block_index,
                    }));
                }
            }

            if let Some(finish_reason) = choice.finish_reason {
                self.stop_reason = Some(normalize_finish_reason(&finish_reason));
                if finish_reason == "tool_calls" {
                    for state in self.tool_calls.values_mut() {
                        if state.started && !state.stopped {
                            state.stopped = true;
                            events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                                index: state.block_index,
                            }));
                        }
                    }
                }
            }
        }

        Ok(events)
    }

    /// `OpenAI` marks every streamed tool call with `index`; Gemini's
    /// compatibility endpoint omits it and sends parallel calls as separate
    /// items of one chunk. Without an index, reuse the slot of a call with
    /// the same id, otherwise use the item's position in the chunk (moving
    /// past slots already owned by a different call id).
    fn resolve_tool_call_index(&self, tool_call: &DeltaToolCall, position: usize) -> u32 {
        if let Some(index) = tool_call.index {
            return index;
        }
        let position = u32::try_from(position).unwrap_or(u32::MAX);
        let Some(id) = tool_call.id.as_deref() else {
            return position;
        };
        if let Some((index, _)) = self
            .tool_calls
            .iter()
            .find(|(_, state)| state.id.as_deref() == Some(id))
        {
            return *index;
        }
        let slot_taken = self
            .tool_calls
            .get(&position)
            .is_some_and(|state| state.id.as_deref().is_some_and(|existing| existing != id));
        if slot_taken {
            self.tool_calls
                .keys()
                .next_back()
                .map_or(position, |last| last.saturating_add(1))
        } else {
            position
        }
    }

    /// A stream that delivered content but neither a `finish_reason` nor
    /// `[DONE]` was cut off mid-reply.
    fn is_truncated(&self, saw_done: bool) -> bool {
        self.message_started && !self.finished && !saw_done && self.stop_reason.is_none()
    }

    fn finish(&mut self) -> Result<Vec<StreamEvent>, ApiError> {
        if self.finished {
            return Ok(Vec::new());
        }
        if !self.tool_calls.is_empty() {
            if self.stop_reason.as_deref() != Some("tool_use") {
                return Err(ApiError::InvalidSseFrame(
                    "Tool stream ended without a tool_calls finish reason",
                ));
            }
            if self.tool_calls.values().any(|call| {
                call.name.as_ref().is_none_or(String::is_empty)
                    || !serde_json::from_str::<Value>(&call.arguments)
                        .is_ok_and(|value| value.is_object())
            }) {
                return Err(ApiError::InvalidSseFrame(
                    "Tool stream ended with an incomplete function name or arguments",
                ));
            }
        }
        self.finished = true;

        let mut events = Vec::new();
        self.close_prose(&mut events);

        for state in self.tool_calls.values_mut() {
            if !state.started && state.name.is_some() {
                state.block_index = self.next_block_index;
                self.next_block_index += 1;
                if let Some(start_event) = state.start_event()? {
                    state.started = true;
                    events.push(StreamEvent::ContentBlockStart(start_event));
                    if let Some(delta_event) = state.delta_event() {
                        events.push(StreamEvent::ContentBlockDelta(delta_event));
                    }
                }
            }
            if state.started && !state.stopped {
                state.stopped = true;
                events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                    index: state.block_index,
                }));
            }
        }

        if self.message_started {
            events.push(StreamEvent::MessageDelta(MessageDeltaEvent {
                delta: MessageDelta {
                    stop_reason: Some(
                        self.stop_reason
                            .clone()
                            .unwrap_or_else(|| "end_turn".to_string()),
                    ),
                    stop_sequence: None,
                },
                usage: self.usage.clone().unwrap_or(Usage {
                    input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                    output_tokens: 0,
                }),
            }));
            events.push(StreamEvent::MessageStop(MessageStopEvent {}));
        }
        Ok(events)
    }
}

#[derive(Debug, Default)]
struct ToolCallState {
    openai_index: u32,
    /// Content block index, assigned when the call's block starts.
    block_index: u32,
    id: Option<String>,
    name: Option<String>,
    arguments: String,
    emitted_len: usize,
    started: bool,
    stopped: bool,
}

impl ToolCallState {
    fn apply(&mut self, index: u32, tool_call: DeltaToolCall) {
        self.openai_index = index;
        if let Some(id) = tool_call.id {
            self.id = Some(id);
        }
        if let Some(name) = tool_call.function.name {
            self.name = Some(name);
        }
        if let Some(arguments) = tool_call.function.arguments {
            self.arguments.push_str(&arguments);
        }
    }

    #[allow(clippy::unnecessary_wraps)]
    fn start_event(&self) -> Result<Option<ContentBlockStartEvent>, ApiError> {
        let Some(name) = self.name.clone() else {
            return Ok(None);
        };
        let id = self
            .id
            .clone()
            .unwrap_or_else(|| format!("tool_call_{}", self.openai_index));
        Ok(Some(ContentBlockStartEvent {
            index: self.block_index,
            content_block: OutputContentBlock::ToolUse {
                id,
                name,
                input: json!({}),
            },
        }))
    }

    fn delta_event(&mut self) -> Option<ContentBlockDeltaEvent> {
        if self.emitted_len >= self.arguments.len() {
            return None;
        }
        let delta = self.arguments[self.emitted_len..].to_string();
        self.emitted_len = self.arguments.len();
        Some(ContentBlockDeltaEvent {
            index: self.block_index,
            delta: ContentBlockDelta::InputJsonDelta {
                partial_json: delta,
            },
        })
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    id: String,
    model: String,
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    role: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    /// Groq / `OpenRouter` name for `reasoning_content`.
    #[serde(default)]
    reasoning: Option<Value>,
    #[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
    tool_calls: Vec<ResponseToolCall>,
}

#[derive(Debug, Deserialize)]
struct ResponseToolCall {
    id: String,
    function: ResponseToolFunction,
}

#[derive(Debug, Deserialize)]
struct ResponseToolFunction {
    name: String,
    arguments: String,
}

// The field names are the wire names.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens: Option<u32>,
    #[serde(default)]
    total_tokens: Option<u32>,
}

impl OpenAiUsage {
    /// `completion_tokens` already includes reasoning tokens on OpenAI-style
    /// APIs (`completion_tokens_details.reasoning_tokens` is a breakdown of
    /// it, so it is not added again). Providers that hide reasoning tokens
    /// (Gemini) count them only in `total_tokens`; as in the gateway's
    /// billing, any surplus of the total over prompt + completion is output.
    fn to_usage(&self) -> Usage {
        let input_tokens = self.prompt_tokens.unwrap_or_else(|| {
            match (self.total_tokens, self.completion_tokens) {
                (Some(total), Some(completion)) => total.saturating_sub(completion),
                _ => 0,
            }
        });
        let completion_tokens = self.completion_tokens.unwrap_or(0);
        let output_tokens = self.total_tokens.map_or(completion_tokens, |total| {
            completion_tokens.max(total.saturating_sub(input_tokens))
        });
        Usage {
            input_tokens,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            output_tokens,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChunk {
    id: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    choices: Vec<ChunkChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct ChunkChoice {
    delta: ChunkDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChunkDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    /// Groq / `OpenRouter` name for `reasoning_content`.
    #[serde(default)]
    reasoning: Option<Value>,
    #[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
    tool_calls: Vec<DeltaToolCall>,
}

/// The non-empty reasoning text of a message or delta: `reasoning_content`,
/// else `reasoning` when it is a string.
fn reasoning_text(reasoning_content: Option<String>, reasoning: Option<&Value>) -> Option<String> {
    reasoning_content
        .filter(|value| !value.is_empty())
        .or_else(|| {
            reasoning
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
}

#[derive(Debug, Deserialize)]
struct DeltaToolCall {
    /// Absent on Gemini's OpenAI-compatible stream; see
    /// [`StreamState::resolve_tool_call_index`].
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: DeltaFunction,
}

#[derive(Debug, Default, Deserialize)]
struct DeltaFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// Returns true for models known to reject tuning parameters like temperature,
/// `top_p`, `frequency_penalty`, and `presence_penalty`. These are typically
/// reasoning/chain-of-thought models with fixed sampling.
/// Returns true for models known to reject tuning parameters like temperature,
/// `top_p`, `frequency_penalty`, and `presence_penalty`. These are typically
/// reasoning/chain-of-thought models with fixed sampling.
/// Public for benchmarking and testing purposes.
#[must_use]
pub fn is_reasoning_model(model: &str) -> bool {
    let lowered = model.to_ascii_lowercase();
    // Strip any provider/ prefix for the check (e.g. qwen/qwen-qwq -> qwen-qwq)
    let canonical = lowered.rsplit('/').next().unwrap_or(lowered.as_str());
    // OpenAI reasoning models
    canonical.starts_with("o1")
        || canonical.starts_with("o3")
        || canonical.starts_with("o4")
        // xAI reasoning: grok-3-mini always uses reasoning mode
        || canonical == "grok-3-mini"
        // Alibaba DashScope reasoning variants (QwQ + Qwen3-Thinking family)
        || canonical.starts_with("qwen-qwq")
        || canonical.starts_with("qwq")
        || canonical.contains("thinking")
}

/// Strip routing prefix (e.g., "openai/gpt-4" → "gpt-4") for the wire.
/// The prefix is used only to select transport; the backend expects the
/// bare model id.
fn strip_routing_prefix(model: &str) -> &str {
    if let Some(pos) = model.find('/') {
        let prefix = &model[..pos];
        // Only strip if the prefix before "/" is a known routing prefix,
        // not if "/" appears in the middle of the model name for other reasons.
        if matches!(prefix, "openai" | "xai" | "grok" | "kimi") {
            &model[pos + 1..]
        } else {
            model
        }
    } else {
        model
    }
}

/// Estimate the serialized JSON size of a request payload in bytes.
/// This is a pre-flight check to avoid hitting provider-specific size limits.
pub fn estimate_request_body_size(request: &MessageRequest, config: OpenAiCompatConfig) -> usize {
    let payload = build_chat_completion_request(request, config);
    // serde_json::to_vec gives us the exact byte size of the serialized JSON
    serde_json::to_vec(&payload).map_or(0, |v| v.len())
}

/// Pre-flight check for request body size against provider limits.
/// Returns Ok(()) if the request is within limits, or an error with
/// a clear message about the size limit being exceeded.
pub fn check_request_body_size(
    request: &MessageRequest,
    config: OpenAiCompatConfig,
) -> Result<(), ApiError> {
    let estimated_bytes = estimate_request_body_size(request, config);
    let max_bytes = config.max_request_body_bytes;

    if estimated_bytes > max_bytes {
        Err(ApiError::RequestBodySizeExceeded {
            estimated_bytes,
            max_bytes,
            provider: config.provider_name,
        })
    } else {
        Ok(())
    }
}

/// Builds a chat completion request payload from a `MessageRequest`.
/// Public for benchmarking purposes.
pub fn build_chat_completion_request(
    request: &MessageRequest,
    config: OpenAiCompatConfig,
) -> Value {
    let mut messages = Vec::new();
    if let Some(system) = request.system.as_ref().filter(|value| !value.is_empty()) {
        messages.push(json!({
            "role": "system",
            "content": system,
        }));
    }
    // Strip routing prefix (e.g., "openai/gpt-4" → "gpt-4") for the wire.
    let wire_model = strip_routing_prefix(&request.model);
    for message in &request.messages {
        messages.extend(translate_message(message, wire_model));
    }
    // The gateway routes to upstreams that reject the non-standard
    // `is_error` field on tool messages (Groq answers 400 to every request
    // that carries a tool result), so it never goes to Zero-X.
    if config.is_zero_x() {
        for message in &mut messages {
            if let Some(object) = message.as_object_mut() {
                object.remove("is_error");
            }
        }
    }
    // Sanitize: drop any `role:"tool"` message that does not have a valid
    // paired `role:"assistant"` with a `tool_calls` entry carrying the same
    // `id` immediately before it (directly or as part of a run of tool
    // results). OpenAI-compatible backends return 400 for orphaned tool
    // messages regardless of how they were produced (compaction, session
    // editing, resume, etc.). We drop rather than error so the request can
    // still proceed with the remaining history intact.
    messages = sanitize_tool_message_pairing(messages);

    // gpt-5* requires `max_completion_tokens`; older OpenAI models accept both.
    // We send the correct field based on the wire model name so gpt-5.x requests
    // don't fail with "unknown field max_tokens".
    let max_tokens_key = if wire_model.starts_with("gpt-5") {
        "max_completion_tokens"
    } else {
        "max_tokens"
    };

    let mut payload = json!({
        "model": wire_model,
        max_tokens_key: request.max_tokens,
        "messages": messages,
        "stream": request.stream,
    });

    if request.stream && should_request_stream_usage(config) {
        payload["stream_options"] = json!({ "include_usage": true });
    }

    if let Some(tools) = &request.tools {
        payload["tools"] =
            Value::Array(tools.iter().map(openai_tool_definition).collect::<Vec<_>>());
    }
    if let Some(tool_choice) = &request.tool_choice {
        payload["tool_choice"] = openai_tool_choice(tool_choice);
    }

    // OpenAI-compatible tuning parameters — only included when explicitly set.
    // Reasoning models (o1/o3/o4/grok-3-mini) reject these params with 400;
    // silently strip them to avoid cryptic provider errors.
    if !is_reasoning_model(&request.model) {
        if let Some(temperature) = request.temperature {
            payload["temperature"] = json!(temperature);
        }
        if let Some(top_p) = request.top_p {
            payload["top_p"] = json!(top_p);
        }
        if let Some(frequency_penalty) = request.frequency_penalty {
            payload["frequency_penalty"] = json!(frequency_penalty);
        }
        if let Some(presence_penalty) = request.presence_penalty {
            payload["presence_penalty"] = json!(presence_penalty);
        }
    }
    // stop is generally safe for all providers
    if let Some(stop) = &request.stop {
        if !stop.is_empty() {
            payload["stop"] = json!(stop);
        }
    }
    // reasoning_effort for OpenAI-compatible reasoning models (o4-mini, o3, etc.)
    if let Some(effort) = &request.reasoning_effort {
        payload["reasoning_effort"] = json!(effort);
    }

    payload
}

/// Returns true for models that do NOT support the `is_error` field in tool results.
/// kimi models (via Moonshot AI/Dashscope) reject this field with 400 Bad Request.
/// Returns true for models that do NOT support the `is_error` field in tool results.
/// kimi models (via Moonshot AI/Dashscope) reject this field with 400 Bad Request.
/// Public for benchmarking and testing purposes.
#[must_use]
pub fn model_rejects_is_error_field(model: &str) -> bool {
    let lowered = model.to_ascii_lowercase();
    // Strip any provider/ prefix for the check
    let canonical = lowered.rsplit('/').next().unwrap_or(lowered.as_str());
    // kimi models (kimi-k2.5, kimi-k1.5, kimi-moonshot, etc.)
    canonical.starts_with("kimi")
}

/// Translates an `InputMessage` into OpenAI-compatible message format.
/// Public for benchmarking purposes.
#[must_use]
pub fn translate_message(message: &InputMessage, model: &str) -> Vec<Value> {
    let supports_is_error = !model_rejects_is_error_field(model);
    match message.role.as_str() {
        "assistant" => {
            let mut text = String::new();
            let mut tool_calls = Vec::new();
            for block in &message.content {
                match block {
                    InputContentBlock::Text { text: value } => text.push_str(value),
                    InputContentBlock::ToolUse { id, name, input } => tool_calls.push(json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": input.to_string(),
                        }
                    })),
                    InputContentBlock::ToolResult { .. } => {}
                }
            }
            if text.is_empty() && tool_calls.is_empty() {
                Vec::new()
            } else {
                let mut msg = serde_json::json!({
                    "role": "assistant",
                    "content": (!text.is_empty()).then_some(text),
                });
                // Only include tool_calls when non-empty: some providers reject
                // assistant messages with an explicit empty tool_calls array.
                if !tool_calls.is_empty() {
                    msg["tool_calls"] = json!(tool_calls);
                }
                vec![msg]
            }
        }
        _ => message
            .content
            .iter()
            .filter_map(|block| match block {
                InputContentBlock::Text { text } => Some(json!({
                    "role": "user",
                    "content": text,
                })),
                InputContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    let mut msg = json!({
                        "role": "tool",
                        "tool_call_id": tool_use_id,
                        "content": flatten_tool_result_content(content),
                    });
                    // Only include is_error for models that support it.
                    // kimi models reject this field with 400 Bad Request.
                    if supports_is_error {
                        msg["is_error"] = json!(is_error);
                    }
                    Some(msg)
                }
                InputContentBlock::ToolUse { .. } => None,
            })
            .collect(),
    }
}

/// Remove `role:"tool"` messages from `messages` that have no valid paired
/// `role:"assistant"` message with a matching `tool_calls[].id` immediately
/// preceding them. This is a last-resort safety net at the request-building
/// layer — the compaction boundary fix (6e301c8) prevents the most common
/// producer path, but resume, session editing, or future compaction variants
/// could still create orphaned tool messages.
///
/// Algorithm: scan left-to-right. For each `role:"tool"` message, check the
/// immediately preceding non-tool message. If it's `role:"assistant"` with a
/// `tool_calls` array containing an entry whose `id` matches the tool
/// message's `tool_call_id`, the pair is valid and both are kept. Otherwise
/// the tool message is dropped.
/// Remove `role:"tool"` messages from `messages` that have no valid paired
/// `role:"assistant"` message with a matching `tool_calls[].id` immediately
/// preceding them. Public for benchmarking purposes.
pub fn sanitize_tool_message_pairing(messages: Vec<Value>) -> Vec<Value> {
    // Collect indices of tool messages that are orphaned.
    let mut drop_indices = std::collections::HashSet::new();
    for (i, msg) in messages.iter().enumerate() {
        if msg.get("role").and_then(|v| v.as_str()) != Some("tool") {
            continue;
        }
        let tool_call_id = msg
            .get("tool_call_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // Find the nearest preceding non-tool message.
        let preceding = messages[..i]
            .iter()
            .rev()
            .find(|m| m.get("role").and_then(|v| v.as_str()) != Some("tool"));
        // A tool message is considered paired when:
        // (a) the nearest preceding non-tool message is an assistant message
        //     whose `tool_calls` array contains an entry with the matching id, OR
        // (b) there's no clear preceding context (e.g. the message comes right
        //     after a user turn — this can happen with translated mixed-content
        //     user messages). In case (b) we allow the message through rather
        //     than silently dropping potentially valid history.
        let preceding_role = preceding
            .and_then(|m| m.get("role"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // Only apply sanitization when the preceding message is an assistant
        // turn (the invariant is: assistant-with-tool_calls must precede tool).
        // If the preceding is something else (user, system) don't drop — it
        // may be a valid translation artifact or a path we don't understand.
        if preceding_role != "assistant" {
            continue;
        }
        let paired = preceding
            .and_then(|m| m.get("tool_calls").and_then(|tc| tc.as_array()))
            .is_some_and(|tool_calls| {
                tool_calls
                    .iter()
                    .any(|tc| tc.get("id").and_then(|v| v.as_str()) == Some(tool_call_id))
            });
        if !paired {
            drop_indices.insert(i);
        }
    }
    if drop_indices.is_empty() {
        return messages;
    }
    messages
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !drop_indices.contains(i))
        .map(|(_, m)| m)
        .collect()
}

/// Flattens tool result content blocks into a single string.
/// Optimized to pre-allocate capacity and avoid intermediate `Vec` construction.
#[must_use]
pub fn flatten_tool_result_content(content: &[ToolResultContentBlock]) -> String {
    // Pre-calculate total capacity needed to avoid reallocations
    let total_len: usize = content
        .iter()
        .map(|block| match block {
            ToolResultContentBlock::Text { text } => text.len(),
            ToolResultContentBlock::Json { value } => value.to_string().len(),
        })
        .sum();

    // Add capacity for newlines between blocks
    let capacity = total_len + content.len().saturating_sub(1);

    let mut result = String::with_capacity(capacity);
    for (i, block) in content.iter().enumerate() {
        if i > 0 {
            result.push('\n');
        }
        match block {
            ToolResultContentBlock::Text { text } => result.push_str(text),
            ToolResultContentBlock::Json { value } => {
                // Use write! to append without creating intermediate String
                result.push_str(&value.to_string());
            }
        }
    }
    result
}

/// Recursively ensure every object-type node in a JSON Schema has
/// `"properties"` (at least `{}`) and `"additionalProperties": false`.
/// The `OpenAI` `/responses` endpoint validates schemas strictly and rejects
/// objects that omit these fields; `/chat/completions` is lenient but also
/// accepts them, so we normalise unconditionally.
fn normalize_object_schema(schema: &mut Value) {
    if let Some(obj) = schema.as_object_mut() {
        if obj.get("type").and_then(Value::as_str) == Some("object") {
            obj.entry("properties").or_insert_with(|| json!({}));
            obj.entry("additionalProperties")
                .or_insert(Value::Bool(false));
        }
        // Recurse into properties values
        if let Some(props) = obj.get_mut("properties") {
            if let Some(props_obj) = props.as_object_mut() {
                let keys: Vec<String> = props_obj.keys().cloned().collect();
                for k in keys {
                    if let Some(v) = props_obj.get_mut(&k) {
                        normalize_object_schema(v);
                    }
                }
            }
        }
        // Recurse into items (arrays)
        if let Some(items) = obj.get_mut("items") {
            normalize_object_schema(items);
        }
    }
}

fn openai_tool_definition(tool: &ToolDefinition) -> Value {
    let mut parameters = tool.input_schema.clone();
    normalize_object_schema(&mut parameters);
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": parameters,
        }
    })
}

fn openai_tool_choice(tool_choice: &ToolChoice) -> Value {
    match tool_choice {
        ToolChoice::Auto => Value::String("auto".to_string()),
        ToolChoice::Any => Value::String("required".to_string()),
        ToolChoice::Tool { name } => json!({
            "type": "function",
            "function": { "name": name },
        }),
    }
}

fn should_request_stream_usage(config: OpenAiCompatConfig) -> bool {
    matches!(config.provider_name, "OpenAI" | ZERO_X_PROVIDER_NAME)
}

fn normalize_response(
    model: &str,
    response: ChatCompletionResponse,
) -> Result<MessageResponse, ApiError> {
    let choice = response
        .choices
        .into_iter()
        .next()
        .ok_or(ApiError::InvalidSseFrame(
            "chat completion response missing choices",
        ))?;
    let mut content = Vec::new();
    if let Some(thinking) = reasoning_text(
        choice.message.reasoning_content,
        choice.message.reasoning.as_ref(),
    ) {
        content.push(OutputContentBlock::Thinking {
            thinking,
            signature: None,
        });
    }
    if let Some(text) = choice.message.content.filter(|value| !value.is_empty()) {
        content.push(OutputContentBlock::Text { text });
    }
    for tool_call in choice.message.tool_calls {
        content.push(OutputContentBlock::ToolUse {
            id: tool_call.id,
            name: tool_call.function.name,
            input: parse_tool_arguments(&tool_call.function.arguments),
        });
    }

    Ok(MessageResponse {
        id: response.id,
        kind: "message".to_string(),
        role: choice.message.role,
        content,
        model: response.model.if_empty_then(model.to_string()),
        stop_reason: choice
            .finish_reason
            .map(|value| normalize_finish_reason(&value)),
        stop_sequence: None,
        usage: response
            .usage
            .as_ref()
            .map(OpenAiUsage::to_usage)
            .unwrap_or_default(),
        request_id: None,
    })
}

fn parse_tool_arguments(arguments: &str) -> Value {
    serde_json::from_str(arguments).unwrap_or_else(|_| json!({ "raw": arguments }))
}

fn next_sse_frame(buffer: &mut Vec<u8>) -> Option<String> {
    let separator = buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|position| (position, 2))
        .into_iter()
        .chain(
            buffer
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| (position, 4)),
        )
        .min_by_key(|(position, _)| *position)?;

    let (position, separator_len) = separator;
    let frame = buffer.drain(..position + separator_len).collect::<Vec<_>>();
    let frame_len = frame.len().saturating_sub(separator_len);
    Some(String::from_utf8_lossy(&frame[..frame_len]).into_owned())
}

fn parse_sse_frame(frame: &str, provider: &str, model: &str) -> Result<SseFrame, ApiError> {
    let trimmed = frame.trim();
    if trimmed.is_empty() {
        return Ok(SseFrame::Skip);
    }

    let mut data_lines = Vec::new();
    for line in trimmed.lines() {
        if line.starts_with(':') {
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start());
        }
    }
    if data_lines.is_empty() {
        return Ok(SseFrame::Skip);
    }
    let payload = data_lines.join("\n");
    if payload == "[DONE]" {
        return Ok(SseFrame::Done);
    }
    // Some backends embed an error object in a data: frame instead of using an
    // HTTP error status. Surface the error message directly rather than letting
    // ChatCompletionChunk deserialization fail with a cryptic 'missing field' error.
    if let Ok(raw) = serde_json::from_str::<serde_json::Value>(&payload) {
        if let Some(error) = embedded_error(
            &raw,
            &payload,
            None,
            provider,
            "provider returned an error in stream",
        ) {
            return Err(error);
        }
    }
    serde_json::from_str::<ChatCompletionChunk>(&payload)
        .map(SseFrame::Chunk)
        .map_err(|error| ApiError::json_deserialize(provider, model, &payload, error))
}

/// Converts an `{"error": ...}` object delivered with a 2xx status (or in a
/// streamed `data:` frame) into an error. Returns `None` when `raw` is not an
/// error envelope.
fn embedded_error(
    raw: &Value,
    payload: &str,
    request_id: Option<String>,
    provider: &str,
    fallback_message: &str,
) -> Option<ApiError> {
    let error = raw.get("error").filter(|value| !value.is_null())?;
    let code = error.get("code");
    let status = code
        .and_then(Value::as_u64)
        .and_then(|code| u16::try_from(code).ok())
        .or_else(|| {
            code.and_then(Value::as_str)
                .and_then(GatewayErrorCode::parse)
                .map(GatewayErrorCode::default_status)
        })
        .and_then(|code| reqwest::StatusCode::from_u16(code).ok())
        .unwrap_or(reqwest::StatusCode::BAD_REQUEST);
    if let Some(gateway) = GatewayError::from_value(status, raw) {
        if provider == ZERO_X_PROVIDER_NAME || gateway.has_actionable_hints() {
            return Some(ApiError::Gateway(Box::new(
                gateway.with_request_id_fallback(request_id),
            )));
        }
    }
    Some(ApiError::Api {
        status,
        error_type: error.get("type").and_then(Value::as_str).map(str::to_owned),
        message: Some(
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or(fallback_message)
                .to_string(),
        ),
        request_id,
        body: payload.to_string(),
        retryable: false,
        suggested_action: suggested_action_for_status(status),
    })
}

fn read_env_non_empty(key: &str) -> Result<Option<String>, ApiError> {
    match std::env::var(key) {
        Ok(value) if !value.is_empty() => Ok(Some(value)),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(super::dotenv_value(key)),
        Err(error) => Err(ApiError::from(error)),
    }
}

#[must_use]
pub fn has_api_key(key: &str) -> bool {
    read_env_non_empty(key)
        .ok()
        .and_then(std::convert::identity)
        .is_some()
}

#[must_use]
pub fn read_base_url(config: OpenAiCompatConfig) -> String {
    std::env::var(config.base_url_env).unwrap_or_else(|_| config.default_base_url.to_string())
}

fn chat_completions_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

/// What the Zero-X gateway said was left of today's quota on a reply
/// (`x-neuron-quota-remaining-requests` and `x-neuron-quota-remaining-tokens`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QuotaRemaining {
    pub requests: Option<u64>,
    pub tokens: Option<u64>,
}

static LAST_QUOTA_REMAINING: Mutex<Option<QuotaRemaining>> = Mutex::new(None);

/// The quota the last reply that reported one said was left, in this process.
#[must_use]
pub fn last_quota_remaining() -> Option<QuotaRemaining> {
    *LAST_QUOTA_REMAINING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Keeps the quota a reply reports; a reply without the headers changes nothing.
fn record_quota_headers(headers: &reqwest::header::HeaderMap) {
    let read = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
    };
    let quota = QuotaRemaining {
        requests: read("x-neuron-quota-remaining-requests"),
        tokens: read("x-neuron-quota-remaining-tokens"),
    };
    if quota != QuotaRemaining::default() {
        *LAST_QUOTA_REMAINING
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(quota);
    }
}

fn request_id_from_headers(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get(REQUEST_ID_HEADER)
        .or_else(|| headers.get(ALT_REQUEST_ID_HEADER))
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

async fn expect_success(
    response: reqwest::Response,
    config: OpenAiCompatConfig,
) -> Result<reqwest::Response, HttpFailure> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let request_id = request_id_from_headers(response.headers());
    let retry_after = retry_after_from_headers(response.headers());
    let body = response.text().await.unwrap_or_default();
    Err(HttpFailure {
        error: Box::new(error_from_response(
            status,
            request_id,
            retry_after,
            body,
            config,
        )),
        retry_after,
    })
}

/// Builds the error for a non-2xx response. Zero-X responses (and any
/// response carrying gateway hints such as `upgrade_url`) become
/// [`ApiError::Gateway`]; everything else keeps the generic shape.
fn error_from_response(
    status: reqwest::StatusCode,
    request_id: Option<String>,
    retry_after: Option<Duration>,
    body: String,
    config: OpenAiCompatConfig,
) -> ApiError {
    let parsed = GatewayError::from_body(status, &body);
    let is_gateway = config.is_zero_x()
        || parsed
            .as_ref()
            .is_some_and(GatewayError::has_actionable_hints);
    if is_gateway {
        let mut error = parsed
            .unwrap_or_else(|| GatewayError::from_status(status))
            .with_request_id_fallback(request_id)
            .with_retry_after_fallback(retry_after);
        // Older gateway builds answer `{"error": "Invalid or expired
        // session"}` without a code; any 401 there means the same thing.
        if config.is_zero_x() && status == reqwest::StatusCode::UNAUTHORIZED && error.code.is_none()
        {
            error.code = Some(GatewayErrorCode::InvalidSession.as_str().to_string());
        }
        return ApiError::Gateway(Box::new(error));
    }

    let (error_type, message, code) = match parsed {
        Some(error) => (
            error.error_type,
            Some(error.message).filter(|message| !message.is_empty()),
            error.code,
        ),
        None => (None, None, None),
    };
    ApiError::Api {
        status,
        error_type,
        message,
        request_id,
        body,
        retryable: is_retryable_response(status, code.as_deref()),
        suggested_action: suggested_action_for_status(status),
    }
}

/// Short reason for a retry progress line.
fn retry_reason(error: &ApiError) -> String {
    match error {
        ApiError::Gateway(error) => error
            .code
            .clone()
            .unwrap_or_else(|| error.status.to_string()),
        ApiError::Api { status, .. } => status.to_string(),
        ApiError::Http(error) if error.is_timeout() => "request timed out".to_string(),
        ApiError::Http(_) => "connection failed".to_string(),
        other => other.safe_failure_class().to_string(),
    }
}

const fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 409 | 429 | 500 | 502 | 503 | 504)
}

fn is_retryable_response(status: reqwest::StatusCode, error_code: Option<&str>) -> bool {
    is_retryable_status(status) && error_code != Some("insufficient_quota")
}

/// Generate a suggested user action based on the HTTP status code and error context.
/// This provides actionable guidance when API requests fail.
fn suggested_action_for_status(status: reqwest::StatusCode) -> Option<String> {
    match status.as_u16() {
        401 => Some("Check API key is set correctly and has not expired".to_string()),
        403 => Some("Verify API key has required permissions for this operation".to_string()),
        413 => Some("Reduce prompt size or context window before retrying".to_string()),
        429 => Some("Wait a moment before retrying; consider reducing request rate".to_string()),
        500 => Some("Provider server error - retry after a brief wait".to_string()),
        502..=504 => Some("Provider gateway error - retry after a brief wait".to_string()),
        _ => None,
    }
}

fn normalize_finish_reason(value: &str) -> String {
    match value {
        "stop" => "end_turn",
        "tool_calls" => "tool_use",
        other => other,
    }
    .to_string()
}

trait StringExt {
    fn if_empty_then(self, fallback: String) -> String;
}

impl StringExt for String {
    fn if_empty_then(self, fallback: String) -> String {
        if self.is_empty() {
            fallback
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn quota_exhaustion_is_not_retried_as_a_rate_limit() {
        assert!(!super::is_retryable_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            Some("insufficient_quota")
        ));
        assert!(super::is_retryable_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            None
        ));
    }

    #[test]
    fn stream_parser_flushes_eof_and_preserves_mixed_frame_order() {
        let mut parser = super::OpenAiSseParser::with_context("zero-x", "auto");
        let first = r#"{"id":"first","choices":[{"delta":{"content":"one"}}]}"#;
        let second = r#"{"id":"second","choices":[{"delta":{"content":"two"}}]}"#;
        let final_chunk = r#"{"id":"final","choices":[{"delta":{},"finish_reason":"stop"}]}"#;
        let chunks = parser
            .push(format!("data: {first}\r\n\r\ndata: {second}\n\ndata: {final_chunk}").as_bytes())
            .unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].id, "first");
        assert_eq!(chunks[1].id, "second");
        assert_eq!(parser.finish().unwrap()[0].id, "final");
        assert!(parser.finish().unwrap().is_empty());
        assert_eq!(
            super::strip_routing_prefix("qwen/qwen3-coder:free"),
            "qwen/qwen3-coder:free"
        );
        assert_eq!(
            super::strip_routing_prefix("openai/qwen/qwen3-coder:free"),
            "qwen/qwen3-coder:free"
        );
    }

    #[test]
    fn tool_stream_requires_explicit_completion_before_execution() {
        let mut state = super::StreamState::new("auto".into());
        let chunk = serde_json::from_value(serde_json::json!({
            "id": "tool", "choices": [{"delta": {"tool_calls": [{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{}"}}]}}]
        })).unwrap();
        state.ingest_chunk(chunk).unwrap();
        assert!(state.finish().is_err());
        let completed = serde_json::from_value(serde_json::json!({
            "id": "tool", "choices": [{"delta": {}, "finish_reason": "tool_calls"}]
        }))
        .unwrap();
        state.ingest_chunk(completed).unwrap();
        let events = state.finish().unwrap();
        assert!(events.iter().any(|event| matches!(event, crate::types::StreamEvent::MessageDelta(delta) if delta.delta.stop_reason.as_deref() == Some("tool_use"))));
        let mut malformed = super::StreamState::new("auto".into());
        let chunk = serde_json::from_value(serde_json::json!({
            "id": "tool", "choices": [{"delta": {"tool_calls": [{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{\"path\":"}}]}, "finish_reason":"tool_calls"}]
        })).unwrap();
        malformed.ingest_chunk(chunk).unwrap();
        assert!(malformed.finish().is_err());
    }
    /// Gemini's OpenAI-compatible stream sends two parallel calls as two
    /// items of one chunk and omits `index`. Before the fix both items
    /// landed in slot 0 and their arguments were concatenated into one
    /// invalid-JSON call.
    #[test]
    fn gemini_parallel_tool_calls_without_index_stay_separate() {
        let mut state = super::StreamState::new("gemini/gemini-2.5-flash".into());
        let chunk = serde_json::from_value(serde_json::json!({
            "id": "gemini",
            "choices": [{"delta": {"role": "assistant", "tool_calls": [
                {"id": "call_a", "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"a.txt\"}"}},
                {"id": "call_b", "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"b.txt\"}"}}
            ]}, "finish_reason": "tool_calls"}]
        }))
        .unwrap();
        let mut events = state.ingest_chunk(chunk).unwrap();
        events.extend(state.finish().unwrap());

        let starts = events
            .iter()
            .filter_map(|event| match event {
                crate::types::StreamEvent::ContentBlockStart(start) => match &start.content_block {
                    crate::types::OutputContentBlock::ToolUse { id, .. } => {
                        Some((start.index, id.clone()))
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            starts,
            vec![(0, "call_a".to_string()), (1, "call_b".to_string())]
        );
        let arguments = |index: u32| {
            events
                .iter()
                .filter_map(|event| match event {
                    crate::types::StreamEvent::ContentBlockDelta(delta) if delta.index == index => {
                        match &delta.delta {
                            crate::types::ContentBlockDelta::InputJsonDelta { partial_json } => {
                                Some(partial_json.clone())
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                })
                .collect::<String>()
        };
        assert_eq!(arguments(0), r#"{"path":"a.txt"}"#);
        assert_eq!(arguments(1), r#"{"path":"b.txt"}"#);
    }

    /// Collects `(kind, index)` for block starts, `("delta:<kind>", index)`
    /// for deltas and `("stop", index)` for stops, in stream order.
    fn block_trace(events: &[crate::types::StreamEvent]) -> Vec<(String, u32)> {
        use crate::types::{ContentBlockDelta, OutputContentBlock, StreamEvent};
        events
            .iter()
            .filter_map(|event| match event {
                StreamEvent::ContentBlockStart(start) => {
                    let kind = match &start.content_block {
                        OutputContentBlock::Text { .. } => "text",
                        OutputContentBlock::Thinking { .. } => "thinking",
                        OutputContentBlock::ToolUse { .. } => "tool",
                        OutputContentBlock::RedactedThinking { .. } => "redacted",
                    };
                    Some((kind.to_string(), start.index))
                }
                StreamEvent::ContentBlockDelta(delta) => {
                    let kind = match &delta.delta {
                        ContentBlockDelta::TextDelta { .. } => "delta:text",
                        ContentBlockDelta::ThinkingDelta { .. } => "delta:thinking",
                        ContentBlockDelta::InputJsonDelta { .. } => "delta:tool",
                        ContentBlockDelta::SignatureDelta { .. } => "delta:signature",
                    };
                    Some((kind.to_string(), delta.index))
                }
                StreamEvent::ContentBlockStop(stop) => Some(("stop".to_string(), stop.index)),
                _ => None,
            })
            .collect()
    }

    fn streamed_text(events: &[crate::types::StreamEvent]) -> (String, String) {
        use crate::types::{ContentBlockDelta, StreamEvent};
        let mut text = String::new();
        let mut thinking = String::new();
        for event in events {
            if let StreamEvent::ContentBlockDelta(delta) = event {
                match &delta.delta {
                    ContentBlockDelta::TextDelta { text: value } => text.push_str(value),
                    ContentBlockDelta::ThinkingDelta { thinking: value } => {
                        thinking.push_str(value);
                    }
                    _ => {}
                }
            }
        }
        (text, thinking)
    }

    fn ingest_all(
        state: &mut super::StreamState,
        chunks: Vec<serde_json::Value>,
    ) -> Vec<crate::types::StreamEvent> {
        let mut events = Vec::new();
        for chunk in chunks {
            events.extend(
                state
                    .ingest_chunk(serde_json::from_value(chunk).unwrap())
                    .unwrap(),
            );
        }
        events.extend(state.finish().unwrap());
        events
    }

    /// The gateway renames Groq / `OpenRouter` `reasoning` to
    /// `reasoning_content`; both stream as a thinking block, never as text.
    #[test]
    fn streamed_reasoning_becomes_a_thinking_block_before_the_text() {
        for field in ["reasoning_content", "reasoning"] {
            let mut state = super::StreamState::new("nvidia/nemotron".into());
            let events = ingest_all(
                &mut state,
                vec![
                    serde_json::json!({"id": "r", "choices": [{"delta": {field: "Let me "}}]}),
                    serde_json::json!({"id": "r", "choices": [{"delta": {field: "think."}}]}),
                    serde_json::json!({"id": "r", "choices": [{"delta": {"content": "Hello"}}]}),
                    serde_json::json!({"id": "r", "choices": [{"delta": {"content": "!"}, "finish_reason": "stop"}]}),
                ],
            );
            let (text, thinking) = streamed_text(&events);
            assert_eq!(text, "Hello!", "{field}");
            assert_eq!(thinking, "Let me think.", "{field}");
            let trace = block_trace(&events)
                .into_iter()
                .map(|(kind, index)| format!("{kind}@{index}"))
                .collect::<Vec<_>>();
            assert_eq!(
                trace,
                [
                    "thinking@0",
                    "delta:thinking@0",
                    "delta:thinking@0",
                    "stop@0",
                    "text@1",
                    "delta:text@1",
                    "delta:text@1",
                    "stop@1",
                ],
                "{field}"
            );
        }
    }

    #[test]
    fn chunk_with_reasoning_and_content_emits_both_blocks_in_order() {
        let mut state = super::StreamState::new("groq/openai/gpt-oss-120b".into());
        let events = ingest_all(
            &mut state,
            vec![
                serde_json::json!({"id": "r", "choices": [{"delta": {"reasoning": "plan", "content": "answer"}}]}),
                serde_json::json!({"id": "r", "choices": [{"delta": {"reasoning_content": "more"}}]}),
                serde_json::json!({"id": "r", "choices": [{"delta": {"content": "done"}, "finish_reason": "stop"}]}),
            ],
        );
        let trace = block_trace(&events)
            .into_iter()
            .map(|(kind, index)| format!("{kind}@{index}"))
            .collect::<Vec<_>>();
        assert_eq!(
            trace,
            [
                "thinking@0",
                "delta:thinking@0",
                "stop@0",
                "text@1",
                "delta:text@1",
                "stop@1",
                "thinking@2",
                "delta:thinking@2",
                "stop@2",
                "text@3",
                "delta:text@3",
                "stop@3",
            ]
        );
        assert_eq!(
            streamed_text(&events),
            ("answerdone".to_string(), "planmore".to_string())
        );
    }

    #[test]
    fn reasoning_then_tool_call_closes_thinking_and_gets_distinct_indexes() {
        let mut state = super::StreamState::new("auto".into());
        let events = ingest_all(
            &mut state,
            vec![
                serde_json::json!({"id": "t", "choices": [{"delta": {"reasoning_content": "need a file"}}]}),
                serde_json::json!({"id": "t", "choices": [{"delta": {"tool_calls": [
                    {"index": 0, "id": "call_1", "function": {"name": "read_file", "arguments": "{}"}}
                ]}, "finish_reason": "tool_calls"}]}),
            ],
        );
        let trace = block_trace(&events)
            .into_iter()
            .map(|(kind, index)| format!("{kind}@{index}"))
            .collect::<Vec<_>>();
        assert_eq!(
            trace,
            [
                "thinking@0",
                "delta:thinking@0",
                "stop@0",
                "tool@1",
                "delta:tool@1",
                "stop@1",
            ]
        );
    }

    #[test]
    fn non_streaming_reasoning_becomes_a_thinking_block() {
        for field in ["reasoning_content", "reasoning"] {
            let payload = serde_json::from_value(serde_json::json!({
                "id": "chatcmpl_r",
                "model": "nvidia/nemotron",
                "choices": [{"message": {"role": "assistant", "content": "Hi", field: "thinking..."}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 7}
            }))
            .unwrap();
            let response = super::normalize_response("nvidia/nemotron", payload).unwrap();
            assert_eq!(
                response.content,
                vec![
                    crate::types::OutputContentBlock::Thinking {
                        thinking: "thinking...".to_string(),
                        signature: None,
                    },
                    crate::types::OutputContentBlock::Text {
                        text: "Hi".to_string()
                    },
                ],
                "{field}"
            );
        }
    }

    #[test]
    fn output_tokens_match_the_gateways_completion_tokens() {
        let usage = |value: serde_json::Value| {
            serde_json::from_value::<super::OpenAiUsage>(value)
                .unwrap()
                .to_usage()
        };
        // OpenAI style: reasoning tokens are part of completion_tokens.
        let openai = usage(serde_json::json!({
            "prompt_tokens": 100, "completion_tokens": 50, "total_tokens": 150,
            "completion_tokens_details": {"reasoning_tokens": 30}
        }));
        assert_eq!((openai.input_tokens, openai.output_tokens), (100, 50));
        // Gemini style: hidden reasoning tokens only show in total_tokens.
        let gemini = usage(serde_json::json!({
            "prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 170
        }));
        assert_eq!((gemini.input_tokens, gemini.output_tokens), (100, 70));
        let no_total = usage(serde_json::json!({"prompt_tokens": 9, "completion_tokens": 4}));
        assert_eq!((no_total.input_tokens, no_total.output_tokens), (9, 4));
        let no_prompt = usage(serde_json::json!({"completion_tokens": 4, "total_tokens": 13}));
        assert_eq!((no_prompt.input_tokens, no_prompt.output_tokens), (9, 4));
    }

    #[test]
    fn index_less_calls_in_later_chunks_get_their_own_slot() {
        let mut state = super::StreamState::new("gemini".into());
        for (id, path) in [("call_a", "a.txt"), ("call_b", "b.txt")] {
            let chunk = serde_json::from_value(serde_json::json!({
                "id": "gemini",
                "choices": [{"delta": {"tool_calls": [
                    {"id": id, "function": {"name": "read_file", "arguments": format!("{{\"path\":\"{path}\"}}")}}
                ]}}]
            }))
            .unwrap();
            state.ingest_chunk(chunk).unwrap();
        }
        assert_eq!(state.tool_calls.len(), 2);
        assert_eq!(state.tool_calls[&0].id.as_deref(), Some("call_a"));
        assert_eq!(state.tool_calls[&1].id.as_deref(), Some("call_b"));
    }

    #[test]
    fn stream_without_finish_reason_or_done_is_truncated() {
        let mut state = super::StreamState::new("auto".into());
        let chunk = serde_json::from_value(serde_json::json!({
            "id": "partial", "choices": [{"delta": {"content": "half a rep"}}]
        }))
        .unwrap();
        state.ingest_chunk(chunk).unwrap();
        assert!(state.is_truncated(false));
        assert!(!state.is_truncated(true), "[DONE] marks a complete stream");

        let finished = serde_json::from_value(serde_json::json!({
            "id": "partial", "choices": [{"delta": {}, "finish_reason": "stop"}]
        }))
        .unwrap();
        state.ingest_chunk(finished).unwrap();
        assert!(
            !state.is_truncated(false),
            "finish_reason completes a stream"
        );

        let empty = super::StreamState::new("auto".into());
        assert!(
            !empty.is_truncated(false),
            "a stream with no data frames falls back to the non-streaming path"
        );
    }

    #[test]
    fn sse_parser_records_the_done_sentinel() {
        let mut parser = super::OpenAiSseParser::with_context("Zero-X", "auto");
        parser
            .push(b"data: {\"id\":\"x\",\"choices\":[]}\n\n")
            .unwrap();
        assert!(!parser.saw_done);
        parser.push(b"data: [DONE]\n\n").unwrap();
        assert!(parser.saw_done);
    }

    #[test]
    fn zero_x_responses_become_actionable_gateway_errors() {
        let quota = super::error_from_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            Some("req_header".to_string()),
            None,
            r#"{"error":{"message":"Daily quota reached","code":"insufficient_quota","resets_at":"2026-10-17T00:00:00Z","upgrade_url":"https://zero-x.live/pricing"}}"#.to_string(),
            OpenAiCompatConfig::zero_x(),
        );
        assert!(!quota.is_retryable());
        let gateway = quota.gateway_error().expect("gateway error");
        assert_eq!(gateway.request_id.as_deref(), Some("req_header"));

        let legacy_401 = super::error_from_response(
            reqwest::StatusCode::UNAUTHORIZED,
            None,
            None,
            r#"{"error":"Invalid or expired session"}"#.to_string(),
            OpenAiCompatConfig::zero_x(),
        );
        assert!(legacy_401.is_invalid_session());
        assert!(legacy_401
            .to_string()
            .starts_with(crate::error::INVALID_SESSION_MESSAGE));

        let html_502 = super::error_from_response(
            reqwest::StatusCode::BAD_GATEWAY,
            None,
            Some(std::time::Duration::from_secs(3)),
            "<html>bad gateway</html>".to_string(),
            OpenAiCompatConfig::zero_x(),
        );
        assert!(html_502.is_retryable());
        assert_eq!(
            html_502.retry_after(),
            Some(std::time::Duration::from_secs(3))
        );

        // Other providers keep the generic error unless the body carries
        // gateway hints.
        let openai = super::error_from_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            None,
            None,
            r#"{"error":{"message":"Rate limit","type":"requests","code":"rate_limit_exceeded"}}"#
                .to_string(),
            OpenAiCompatConfig::openai(),
        );
        assert!(matches!(openai, ApiError::Api { .. }));
        assert!(openai.is_retryable());
    }

    #[test]
    fn zero_x_config_uses_the_short_gateway_retry_policy() {
        let client = OpenAiCompatClient::new("ses_x", OpenAiCompatConfig::zero_x());
        assert_eq!(client.max_retries, super::ZERO_X_MAX_RETRIES);
        assert_eq!(client.max_total_backoff, crate::retry::MAX_TOTAL_BACKOFF);
        let openai = OpenAiCompatClient::new("sk", OpenAiCompatConfig::openai());
        assert_eq!(openai.max_retries, super::DEFAULT_MAX_RETRIES);
        assert_eq!(
            OpenAiCompatConfig::zero_x().credential_env_vars(),
            &["NEURON_TOKEN"]
        );
    }

    #[test]
    fn endpoint_debug_output_redacts_the_credential() {
        let endpoint = super::OpenAiCompatEndpoint::new(
            OpenAiCompatConfig::zero_x(),
            "https://zero-x.live/v1",
            "ses_supersecret0123456789",
        );
        let rendered = format!("{endpoint:?}");
        assert!(!rendered.contains("supersecret"));
        assert!(rendered.contains("<redacted>"));
        let client = OpenAiCompatClient::from_endpoint(&endpoint);
        assert_eq!(client.base_url(), "https://zero-x.live/v1");
        assert_eq!(client.provider_config(), OpenAiCompatConfig::zero_x());
    }

    use super::{
        build_chat_completion_request, chat_completions_endpoint, is_reasoning_model,
        normalize_finish_reason, openai_tool_choice, parse_tool_arguments, OpenAiCompatClient,
        OpenAiCompatConfig,
    };
    use crate::error::ApiError;
    use crate::types::{
        InputContentBlock, InputMessage, MessageRequest, ToolChoice, ToolDefinition,
        ToolResultContentBlock,
    };
    use serde_json::json;
    use std::sync::{Mutex, OnceLock};

    #[test]
    fn request_translation_uses_openai_compatible_shape() {
        let payload = build_chat_completion_request(
            &MessageRequest {
                model: "grok-3".to_string(),
                max_tokens: 64,
                messages: vec![InputMessage {
                    role: "user".to_string(),
                    content: vec![
                        InputContentBlock::Text {
                            text: "hello".to_string(),
                        },
                        InputContentBlock::ToolResult {
                            tool_use_id: "tool_1".to_string(),
                            content: vec![ToolResultContentBlock::Json {
                                value: json!({"ok": true}),
                            }],
                            is_error: false,
                        },
                    ],
                }],
                system: Some("be helpful".to_string()),
                tools: Some(vec![ToolDefinition {
                    name: "weather".to_string(),
                    description: Some("Get weather".to_string()),
                    input_schema: json!({"type": "object"}),
                }]),
                tool_choice: Some(ToolChoice::Auto),
                stream: false,
                ..Default::default()
            },
            OpenAiCompatConfig::xai(),
        );

        assert_eq!(payload["messages"][0]["role"], json!("system"));
        assert_eq!(payload["messages"][1]["role"], json!("user"));
        assert_eq!(payload["messages"][2]["role"], json!("tool"));
        assert_eq!(payload["tools"][0]["type"], json!("function"));
        assert_eq!(payload["tool_choice"], json!("auto"));
    }

    #[test]
    fn tool_schema_object_gets_strict_fields_for_responses_endpoint() {
        // OpenAI /responses endpoint rejects object schemas missing
        // "properties" and "additionalProperties". Verify normalize_object_schema
        // fills them in so the request shape is strict-validator-safe.
        use super::normalize_object_schema;

        // Bare object — no properties at all
        let mut schema = json!({"type": "object"});
        normalize_object_schema(&mut schema);
        assert_eq!(schema["properties"], json!({}));
        assert_eq!(schema["additionalProperties"], json!(false));

        // Nested object inside properties
        let mut schema2 = json!({
            "type": "object",
            "properties": {
                "location": {"type": "object", "properties": {"lat": {"type": "number"}}}
            }
        });
        normalize_object_schema(&mut schema2);
        assert_eq!(schema2["additionalProperties"], json!(false));
        assert_eq!(
            schema2["properties"]["location"]["additionalProperties"],
            json!(false)
        );

        // Existing properties/additionalProperties should not be overwritten
        let mut schema3 = json!({
            "type": "object",
            "properties": {"x": {"type": "string"}},
            "additionalProperties": true
        });
        normalize_object_schema(&mut schema3);
        assert_eq!(
            schema3["additionalProperties"],
            json!(true),
            "must not overwrite existing"
        );
    }

    #[test]
    fn reasoning_effort_is_included_when_set() {
        let payload = build_chat_completion_request(
            &MessageRequest {
                model: "o4-mini".to_string(),
                max_tokens: 1024,
                messages: vec![InputMessage::user_text("think hard")],
                reasoning_effort: Some("high".to_string()),
                ..Default::default()
            },
            OpenAiCompatConfig::openai(),
        );
        assert_eq!(payload["reasoning_effort"], json!("high"));
    }

    #[test]
    fn reasoning_effort_omitted_when_not_set() {
        let payload = build_chat_completion_request(
            &MessageRequest {
                model: "gpt-4o".to_string(),
                max_tokens: 64,
                messages: vec![InputMessage::user_text("hello")],
                ..Default::default()
            },
            OpenAiCompatConfig::openai(),
        );
        assert!(payload.get("reasoning_effort").is_none());
    }

    #[test]
    fn openai_streaming_requests_include_usage_opt_in() {
        let payload = build_chat_completion_request(
            &MessageRequest {
                model: "gpt-5".to_string(),
                max_tokens: 64,
                messages: vec![InputMessage::user_text("hello")],
                system: None,
                tools: None,
                tool_choice: None,
                stream: true,
                ..Default::default()
            },
            OpenAiCompatConfig::openai(),
        );

        assert_eq!(payload["stream_options"], json!({"include_usage": true}));
    }

    #[test]
    fn xai_streaming_requests_skip_openai_specific_usage_opt_in() {
        let payload = build_chat_completion_request(
            &MessageRequest {
                model: "grok-3".to_string(),
                max_tokens: 64,
                messages: vec![InputMessage::user_text("hello")],
                system: None,
                tools: None,
                tool_choice: None,
                stream: true,
                ..Default::default()
            },
            OpenAiCompatConfig::xai(),
        );

        assert!(payload.get("stream_options").is_none());
    }

    #[test]
    fn tool_choice_translation_supports_required_function() {
        assert_eq!(openai_tool_choice(&ToolChoice::Any), json!("required"));
        assert_eq!(
            openai_tool_choice(&ToolChoice::Tool {
                name: "weather".to_string(),
            }),
            json!({"type": "function", "function": {"name": "weather"}})
        );
    }

    #[test]
    fn parses_tool_arguments_fallback() {
        assert_eq!(
            parse_tool_arguments("{\"city\":\"Paris\"}"),
            json!({"city": "Paris"})
        );
        assert_eq!(parse_tool_arguments("not-json"), json!({"raw": "not-json"}));
    }

    #[test]
    fn missing_xai_api_key_is_provider_specific() {
        let _lock = env_lock();
        std::env::remove_var("XAI_API_KEY");
        let error = OpenAiCompatClient::from_env(OpenAiCompatConfig::xai())
            .expect_err("missing key should error");
        assert!(matches!(
            error,
            ApiError::MissingCredentials {
                provider: "xAI",
                ..
            }
        ));
    }

    #[test]
    fn endpoint_builder_accepts_base_urls_and_full_endpoints() {
        assert_eq!(
            chat_completions_endpoint("https://api.x.ai/v1"),
            "https://api.x.ai/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_endpoint("https://api.x.ai/v1/"),
            "https://api.x.ai/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_endpoint("https://api.x.ai/v1/chat/completions"),
            "https://api.x.ai/v1/chat/completions"
        );
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("env lock")
    }

    #[test]
    fn normalizes_stop_reasons() {
        assert_eq!(normalize_finish_reason("stop"), "end_turn");
        assert_eq!(normalize_finish_reason("tool_calls"), "tool_use");
    }

    #[test]
    fn tuning_params_included_in_payload_when_set() {
        let request = MessageRequest {
            model: "gpt-4o".to_string(),
            max_tokens: 1024,
            messages: vec![],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            temperature: Some(0.7),
            top_p: Some(0.9),
            frequency_penalty: Some(0.5),
            presence_penalty: Some(0.3),
            stop: Some(vec!["\n".to_string()]),
            reasoning_effort: None,
        };
        let payload = build_chat_completion_request(&request, OpenAiCompatConfig::openai());
        assert_eq!(payload["temperature"], 0.7);
        assert_eq!(payload["top_p"], 0.9);
        assert_eq!(payload["frequency_penalty"], 0.5);
        assert_eq!(payload["presence_penalty"], 0.3);
        assert_eq!(payload["stop"], json!(["\n"]));
    }

    #[test]
    fn reasoning_model_strips_tuning_params() {
        let request = MessageRequest {
            model: "o1-mini".to_string(),
            max_tokens: 1024,
            messages: vec![],
            stream: false,
            temperature: Some(0.7),
            top_p: Some(0.9),
            frequency_penalty: Some(0.5),
            presence_penalty: Some(0.3),
            stop: Some(vec!["\n".to_string()]),
            ..Default::default()
        };
        let payload = build_chat_completion_request(&request, OpenAiCompatConfig::openai());
        assert!(
            payload.get("temperature").is_none(),
            "reasoning model should strip temperature"
        );
        assert!(
            payload.get("top_p").is_none(),
            "reasoning model should strip top_p"
        );
        assert!(payload.get("frequency_penalty").is_none());
        assert!(payload.get("presence_penalty").is_none());
        // stop is safe for all providers
        assert_eq!(payload["stop"], json!(["\n"]));
    }

    #[test]
    fn grok_3_mini_is_reasoning_model() {
        assert!(is_reasoning_model("grok-3-mini"));
        assert!(is_reasoning_model("o1"));
        assert!(is_reasoning_model("o1-mini"));
        assert!(is_reasoning_model("o3-mini"));
        assert!(!is_reasoning_model("gpt-4o"));
        assert!(!is_reasoning_model("grok-3"));
        assert!(!is_reasoning_model("claude-sonnet-4-6"));
    }

    #[test]
    fn qwen_reasoning_variants_are_detected() {
        // QwQ reasoning model
        assert!(is_reasoning_model("qwen-qwq-32b"));
        assert!(is_reasoning_model("qwen/qwen-qwq-32b"));
        // Qwen3 thinking family
        assert!(is_reasoning_model("qwen3-30b-a3b-thinking"));
        assert!(is_reasoning_model("qwen/qwen3-30b-a3b-thinking"));
        // Bare qwq
        assert!(is_reasoning_model("qwq-plus"));
        // Regular Qwen models must NOT be classified as reasoning
        assert!(!is_reasoning_model("qwen-max"));
        assert!(!is_reasoning_model("qwen/qwen-plus"));
        assert!(!is_reasoning_model("qwen-turbo"));
    }

    #[test]
    fn tuning_params_omitted_from_payload_when_none() {
        let request = MessageRequest {
            model: "gpt-4o".to_string(),
            max_tokens: 1024,
            messages: vec![],
            stream: false,
            ..Default::default()
        };
        let payload = build_chat_completion_request(&request, OpenAiCompatConfig::openai());
        assert!(
            payload.get("temperature").is_none(),
            "temperature should be absent"
        );
        assert!(payload.get("top_p").is_none(), "top_p should be absent");
        assert!(payload.get("frequency_penalty").is_none());
        assert!(payload.get("presence_penalty").is_none());
        assert!(payload.get("stop").is_none());
    }

    #[test]
    fn gpt5_uses_max_completion_tokens_not_max_tokens() {
        // gpt-5* models require `max_completion_tokens`; legacy `max_tokens` causes
        // a request-validation failure. Verify the correct key is emitted.
        let request = MessageRequest {
            model: "gpt-5.2".to_string(),
            max_tokens: 512,
            messages: vec![],
            stream: false,
            ..Default::default()
        };
        let payload = build_chat_completion_request(&request, OpenAiCompatConfig::openai());
        assert_eq!(
            payload["max_completion_tokens"],
            json!(512),
            "gpt-5.2 should emit max_completion_tokens"
        );
        assert!(
            payload.get("max_tokens").is_none(),
            "gpt-5.2 must not emit max_tokens"
        );
    }

    /// Regression test: some OpenAI-compatible providers emit `"tool_calls": null`
    /// in stream delta chunks instead of omitting the field or using `[]`.
    /// Before the fix this produced: `invalid type: null, expected a sequence`.
    #[test]
    fn delta_with_null_tool_calls_deserializes_as_empty_vec() {
        use super::deserialize_null_as_empty_vec;

        #[allow(dead_code)]
        #[derive(serde::Deserialize, Debug)]
        struct Delta {
            content: Option<String>,
            #[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
            tool_calls: Vec<super::DeltaToolCall>,
        }

        // Simulate the exact shape observed in the wild (gaebal-gajae repro 2026-04-09)
        let json = r#"{
            "content": "",
            "function_call": null,
            "refusal": null,
            "role": "assistant",
            "tool_calls": null
        }"#;
        let delta: Delta = serde_json::from_str(json)
            .expect("delta with tool_calls:null must deserialize without error");
        assert!(
            delta.tool_calls.is_empty(),
            "tool_calls:null must produce an empty vec, not an error"
        );
    }

    /// Regression: when building a multi-turn request where a prior assistant
    /// turn has no tool calls, the serialized assistant message must NOT include
    /// `tool_calls: []`. Some providers reject requests that carry an empty
    /// `tool_calls` array on assistant turns (gaebal-gajae repro 2026-04-09).
    #[test]
    fn assistant_message_without_tool_calls_omits_tool_calls_field() {
        use crate::types::{InputContentBlock, InputMessage};

        let request = MessageRequest {
            model: "gpt-4o".to_string(),
            max_tokens: 100,
            messages: vec![InputMessage {
                role: "assistant".to_string(),
                content: vec![InputContentBlock::Text {
                    text: "Hello".to_string(),
                }],
            }],
            stream: false,
            ..Default::default()
        };
        let payload = build_chat_completion_request(&request, OpenAiCompatConfig::openai());
        let messages = payload["messages"].as_array().unwrap();
        let assistant_msg = messages
            .iter()
            .find(|m| m["role"] == "assistant")
            .expect("assistant message must be present");
        assert!(
            assistant_msg.get("tool_calls").is_none(),
            "assistant message without tool calls must omit tool_calls field: {assistant_msg:?}"
        );
    }

    /// Regression: assistant messages WITH tool calls must still include
    /// the `tool_calls` array (normal multi-turn tool-use flow).
    #[test]
    fn assistant_message_with_tool_calls_includes_tool_calls_field() {
        use crate::types::{InputContentBlock, InputMessage};

        let request = MessageRequest {
            model: "gpt-4o".to_string(),
            max_tokens: 100,
            messages: vec![InputMessage {
                role: "assistant".to_string(),
                content: vec![InputContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "/tmp/test"}),
                }],
            }],
            stream: false,
            ..Default::default()
        };
        let payload = build_chat_completion_request(&request, OpenAiCompatConfig::openai());
        let messages = payload["messages"].as_array().unwrap();
        let assistant_msg = messages
            .iter()
            .find(|m| m["role"] == "assistant")
            .expect("assistant message must be present");
        let tool_calls = assistant_msg
            .get("tool_calls")
            .expect("assistant message with tool calls must include tool_calls field");
        assert!(tool_calls.is_array());
        assert_eq!(tool_calls.as_array().unwrap().len(), 1);
    }

    /// Orphaned tool messages (no preceding assistant `tool_calls`) must be
    /// dropped by the request-builder sanitizer. Regression for the second
    /// layer of the tool-pairing invariant fix (gaebal-gajae 2026-04-10).
    #[test]
    fn sanitize_drops_orphaned_tool_messages() {
        use super::sanitize_tool_message_pairing;

        // Valid pair: assistant with tool_calls → tool result
        let valid = vec![
            json!({"role": "assistant", "content": null, "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "search", "arguments": "{}"}}]}),
            json!({"role": "tool", "tool_call_id": "call_1", "content": "result"}),
        ];
        let out = sanitize_tool_message_pairing(valid);
        assert_eq!(out.len(), 2, "valid pair must be preserved");

        // Orphaned tool message: no preceding assistant tool_calls
        let orphaned = vec![
            json!({"role": "assistant", "content": "hi"}),
            json!({"role": "tool", "tool_call_id": "call_2", "content": "orphaned"}),
        ];
        let out = sanitize_tool_message_pairing(orphaned);
        assert_eq!(out.len(), 1, "orphaned tool message must be dropped");
        assert_eq!(out[0]["role"], json!("assistant"));

        // Mismatched tool_call_id
        let mismatched = vec![
            json!({"role": "assistant", "content": null, "tool_calls": [{"id": "call_3", "type": "function", "function": {"name": "f", "arguments": "{}"}}]}),
            json!({"role": "tool", "tool_call_id": "call_WRONG", "content": "bad"}),
        ];
        let out = sanitize_tool_message_pairing(mismatched);
        assert_eq!(out.len(), 1, "tool message with wrong id must be dropped");

        // Two tool results both valid (same preceding assistant)
        let two_results = vec![
            json!({"role": "assistant", "content": null, "tool_calls": [
                {"id": "call_a", "type": "function", "function": {"name": "fa", "arguments": "{}"}},
                {"id": "call_b", "type": "function", "function": {"name": "fb", "arguments": "{}"}}
            ]}),
            json!({"role": "tool", "tool_call_id": "call_a", "content": "ra"}),
            json!({"role": "tool", "tool_call_id": "call_b", "content": "rb"}),
        ];
        let out = sanitize_tool_message_pairing(two_results);
        assert_eq!(out.len(), 3, "both valid tool results must be preserved");
    }

    #[test]
    fn non_gpt5_uses_max_tokens() {
        // Older OpenAI models expect `max_tokens`; verify gpt-4o is unaffected.
        let request = MessageRequest {
            model: "gpt-4o".to_string(),
            max_tokens: 512,
            messages: vec![],
            stream: false,
            ..Default::default()
        };
        let payload = build_chat_completion_request(&request, OpenAiCompatConfig::openai());
        assert_eq!(payload["max_tokens"], json!(512));
        assert!(
            payload.get("max_completion_tokens").is_none(),
            "gpt-4o must not emit max_completion_tokens"
        );
    }

    // ============================================================================
    // US-009: kimi model compatibility tests
    // ============================================================================

    #[test]
    fn model_rejects_is_error_field_detects_kimi_models() {
        // kimi models (various formats) should be detected
        assert!(super::model_rejects_is_error_field("kimi-k2.5"));
        assert!(super::model_rejects_is_error_field("kimi-k1.5"));
        assert!(super::model_rejects_is_error_field("kimi-moonshot"));
        assert!(super::model_rejects_is_error_field("KIMI-K2.5")); // case insensitive
        assert!(super::model_rejects_is_error_field("dashscope/kimi-k2.5")); // with prefix
        assert!(super::model_rejects_is_error_field("moonshot/kimi-k2.5")); // different prefix

        // Non-kimi models should NOT be detected
        assert!(!super::model_rejects_is_error_field("gpt-4o"));
        assert!(!super::model_rejects_is_error_field("gpt-4"));
        assert!(!super::model_rejects_is_error_field("claude-sonnet-4-6"));
        assert!(!super::model_rejects_is_error_field("grok-3"));
        assert!(!super::model_rejects_is_error_field("grok-3-mini"));
        assert!(!super::model_rejects_is_error_field("xai/grok-3"));
        assert!(!super::model_rejects_is_error_field("qwen/qwen-plus"));
        assert!(!super::model_rejects_is_error_field("o1-mini"));
    }

    #[test]
    fn translate_message_includes_is_error_for_non_kimi_models() {
        use crate::types::{InputContentBlock, InputMessage, ToolResultContentBlock};

        // Test with gpt-4o (should include is_error)
        let message = InputMessage {
            role: "user".to_string(),
            content: vec![InputContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: vec![ToolResultContentBlock::Text {
                    text: "Error occurred".to_string(),
                }],
                is_error: true,
            }],
        };

        let translated = super::translate_message(&message, "gpt-4o");
        assert_eq!(translated.len(), 1);
        let tool_msg = &translated[0];
        assert_eq!(tool_msg["role"], json!("tool"));
        assert_eq!(tool_msg["tool_call_id"], json!("call_1"));
        assert_eq!(tool_msg["content"], json!("Error occurred"));
        assert!(
            tool_msg.get("is_error").is_some(),
            "gpt-4o should include is_error field"
        );
        assert_eq!(tool_msg["is_error"], json!(true));

        // Test with grok-3 (should include is_error)
        let message2 = InputMessage {
            role: "user".to_string(),
            content: vec![InputContentBlock::ToolResult {
                tool_use_id: "call_2".to_string(),
                content: vec![ToolResultContentBlock::Text {
                    text: "Success".to_string(),
                }],
                is_error: false,
            }],
        };

        let translated2 = super::translate_message(&message2, "grok-3");
        assert!(
            translated2[0].get("is_error").is_some(),
            "grok-3 should include is_error field"
        );
        assert_eq!(translated2[0]["is_error"], json!(false));

        // Test with claude model (should include is_error)
        let translated3 = super::translate_message(&message, "claude-sonnet-4-6");
        assert!(
            translated3[0].get("is_error").is_some(),
            "claude should include is_error field"
        );
    }

    #[test]
    fn translate_message_excludes_is_error_for_kimi_models() {
        use crate::types::{InputContentBlock, InputMessage, ToolResultContentBlock};

        // Test with kimi-k2.5 (should EXCLUDE is_error)
        let message = InputMessage {
            role: "user".to_string(),
            content: vec![InputContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: vec![ToolResultContentBlock::Text {
                    text: "Error occurred".to_string(),
                }],
                is_error: true,
            }],
        };

        let translated = super::translate_message(&message, "kimi-k2.5");
        assert_eq!(translated.len(), 1);
        let tool_msg = &translated[0];
        assert_eq!(tool_msg["role"], json!("tool"));
        assert_eq!(tool_msg["tool_call_id"], json!("call_1"));
        assert_eq!(tool_msg["content"], json!("Error occurred"));
        assert!(
            tool_msg.get("is_error").is_none(),
            "kimi-k2.5 must NOT include is_error field (would cause 400 Bad Request)"
        );

        // Test with kimi-k1.5
        let translated2 = super::translate_message(&message, "kimi-k1.5");
        assert!(
            translated2[0].get("is_error").is_none(),
            "kimi-k1.5 must NOT include is_error field"
        );

        // Test with dashscope/kimi-k2.5 (with provider prefix)
        let translated3 = super::translate_message(&message, "dashscope/kimi-k2.5");
        assert!(
            translated3[0].get("is_error").is_none(),
            "dashscope/kimi-k2.5 must NOT include is_error field"
        );
    }

    #[test]
    fn zero_x_requests_never_carry_is_error_on_tool_messages() {
        use crate::types::{InputContentBlock, InputMessage, ToolResultContentBlock};

        let request = MessageRequest {
            model: "auto".to_string(),
            max_tokens: 100,
            messages: vec![
                InputMessage {
                    role: "assistant".to_string(),
                    content: vec![InputContentBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"command": "node test.js"}),
                    }],
                },
                InputMessage {
                    role: "user".to_string(),
                    content: vec![InputContentBlock::ToolResult {
                        tool_use_id: "call_1".to_string(),
                        content: vec![ToolResultContentBlock::Text {
                            text: "exit code 1".to_string(),
                        }],
                        is_error: true,
                    }],
                },
            ],
            stream: false,
            ..Default::default()
        };
        let tool_message = |config| {
            build_chat_completion_request(&request, config)["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|message| message["role"] == "tool")
                .cloned()
                .unwrap()
        };

        let gateway = tool_message(OpenAiCompatConfig::zero_x());
        assert!(gateway.get("is_error").is_none(), "{gateway}");
        assert_eq!(gateway["tool_call_id"], json!("call_1"));
        assert_eq!(gateway["content"], json!("exit code 1"));
        // Other OpenAI-compatible endpoints keep it.
        assert_eq!(
            tool_message(OpenAiCompatConfig::openai())["is_error"],
            json!(true)
        );
    }

    #[test]
    fn build_chat_completion_request_kimi_vs_non_kimi_tool_results() {
        use crate::types::{InputContentBlock, InputMessage, ToolResultContentBlock};

        // Helper to create a request with a tool result
        let make_request = |model: &str| MessageRequest {
            model: model.to_string(),
            max_tokens: 100,
            messages: vec![
                InputMessage {
                    role: "assistant".to_string(),
                    content: vec![InputContentBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({"path": "/tmp/test"}),
                    }],
                },
                InputMessage {
                    role: "user".to_string(),
                    content: vec![InputContentBlock::ToolResult {
                        tool_use_id: "call_1".to_string(),
                        content: vec![ToolResultContentBlock::Text {
                            text: "file contents".to_string(),
                        }],
                        is_error: false,
                    }],
                },
            ],
            stream: false,
            ..Default::default()
        };

        // Non-kimi model: should have is_error field
        let request_gpt = make_request("gpt-4o");
        let payload_gpt = build_chat_completion_request(&request_gpt, OpenAiCompatConfig::openai());
        let messages_gpt = payload_gpt["messages"].as_array().unwrap();
        let tool_msg_gpt = messages_gpt.iter().find(|m| m["role"] == "tool").unwrap();
        assert!(
            tool_msg_gpt.get("is_error").is_some(),
            "gpt-4o request should include is_error in tool result"
        );

        // kimi model: should NOT have is_error field
        let request_kimi = make_request("kimi-k2.5");
        let payload_kimi =
            build_chat_completion_request(&request_kimi, OpenAiCompatConfig::dashscope());
        let messages_kimi = payload_kimi["messages"].as_array().unwrap();
        let tool_msg_kimi = messages_kimi.iter().find(|m| m["role"] == "tool").unwrap();
        assert!(
            tool_msg_kimi.get("is_error").is_none(),
            "kimi-k2.5 request must NOT include is_error in tool result (would cause 400)"
        );

        // Verify both have the essential fields
        assert_eq!(tool_msg_gpt["tool_call_id"], json!("call_1"));
        assert_eq!(tool_msg_kimi["tool_call_id"], json!("call_1"));
        assert_eq!(tool_msg_gpt["content"], json!("file contents"));
        assert_eq!(tool_msg_kimi["content"], json!("file contents"));
    }

    // ============================================================================
    // US-021: Request body size pre-flight check tests
    // ============================================================================

    #[test]
    fn estimate_request_body_size_returns_reasonable_estimate() {
        let request = MessageRequest {
            model: "gpt-4o".to_string(),
            max_tokens: 100,
            messages: vec![InputMessage::user_text("Hello world".to_string())],
            stream: false,
            ..Default::default()
        };

        let size = super::estimate_request_body_size(&request, OpenAiCompatConfig::openai());
        // Should be non-zero and reasonable for a small request
        assert!(size > 0, "estimated size should be positive");
        assert!(size < 10_000, "small request should be under 10KB");
    }

    #[test]
    fn check_request_body_size_passes_for_small_requests() {
        let request = MessageRequest {
            model: "gpt-4o".to_string(),
            max_tokens: 100,
            messages: vec![InputMessage::user_text("Hello".to_string())],
            stream: false,
            ..Default::default()
        };

        // Should pass for all providers with a small request
        assert!(super::check_request_body_size(&request, OpenAiCompatConfig::openai()).is_ok());
        assert!(super::check_request_body_size(&request, OpenAiCompatConfig::xai()).is_ok());
        assert!(super::check_request_body_size(&request, OpenAiCompatConfig::dashscope()).is_ok());
    }

    #[test]
    fn check_request_body_size_fails_for_dashscope_when_exceeds_6mb() {
        // Create a request that exceeds DashScope's 6MB limit
        let large_content = "x".repeat(7_000_000); // 7MB of content
        let request = MessageRequest {
            model: "qwen-plus".to_string(),
            max_tokens: 100,
            messages: vec![InputMessage::user_text(large_content)],
            stream: false,
            ..Default::default()
        };

        let result = super::check_request_body_size(&request, OpenAiCompatConfig::dashscope());
        assert!(result.is_err(), "should fail for 7MB request to DashScope");

        let err = result.unwrap_err();
        match err {
            crate::error::ApiError::RequestBodySizeExceeded {
                estimated_bytes,
                max_bytes,
                provider,
            } => {
                assert_eq!(provider, "DashScope");
                assert_eq!(max_bytes, 6_291_456); // 6MB limit
                assert!(estimated_bytes > max_bytes);
            }
            _ => panic!("expected RequestBodySizeExceeded error, got {err:?}"),
        }
    }

    #[test]
    fn check_request_body_size_allows_large_requests_for_openai() {
        // Create a request that exceeds DashScope's limit but is under OpenAI's 100MB limit
        let large_content = "x".repeat(10_000_000); // 10MB of content
        let request = MessageRequest {
            model: "gpt-4o".to_string(),
            max_tokens: 100,
            messages: vec![InputMessage::user_text(large_content)],
            stream: false,
            ..Default::default()
        };

        // Should pass for OpenAI (100MB limit)
        assert!(
            super::check_request_body_size(&request, OpenAiCompatConfig::openai()).is_ok(),
            "10MB request should pass for OpenAI's 100MB limit"
        );

        // Should fail for DashScope (6MB limit)
        assert!(
            super::check_request_body_size(&request, OpenAiCompatConfig::dashscope()).is_err(),
            "10MB request should fail for DashScope's 6MB limit"
        );
    }

    #[test]
    fn provider_specific_size_limits_are_correct() {
        assert_eq!(
            OpenAiCompatConfig::dashscope().max_request_body_bytes,
            6_291_456
        ); // 6MB
        assert_eq!(
            OpenAiCompatConfig::openai().max_request_body_bytes,
            104_857_600
        ); // 100MB
        assert_eq!(OpenAiCompatConfig::xai().max_request_body_bytes, 52_428_800);
        // 50MB
    }

    #[test]
    fn strip_routing_prefix_strips_kimi_provider_prefix() {
        // US-023: kimi prefix should be stripped for wire format
        assert_eq!(super::strip_routing_prefix("kimi/kimi-k2.5"), "kimi-k2.5");
        assert_eq!(super::strip_routing_prefix("kimi-k2.5"), "kimi-k2.5"); // no prefix, unchanged
        assert_eq!(super::strip_routing_prefix("kimi/kimi-k1.5"), "kimi-k1.5");
    }
}
