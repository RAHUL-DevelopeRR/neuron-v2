use std::env::VarError;
use std::fmt::{Display, Formatter};
use std::time::Duration;

const GENERIC_FATAL_WRAPPER_MARKERS: &[&str] = &[
    "something went wrong while processing your request",
    "please try again, or use /new to start a fresh session",
];

const CONTEXT_WINDOW_ERROR_MARKERS: &[&str] = &[
    "maximum context length",
    "context window",
    "context length",
    "too many tokens",
    "prompt is too long",
    "input is too long",
    "request is too large",
];

#[derive(Debug)]
pub enum ApiError {
    MissingCredentials {
        provider: &'static str,
        env_vars: &'static [&'static str],
        /// Optional, runtime-computed hint appended to the error Display
        /// output. Populated when the provider resolver can infer what the
        /// user probably intended (e.g. an `OpenAI` key is set but Anthropic
        /// was selected because no Anthropic credentials exist).
        hint: Option<String>,
    },
    ContextWindowExceeded {
        model: String,
        estimated_input_tokens: u32,
        requested_output_tokens: u32,
        estimated_total_tokens: u32,
        context_window_tokens: u32,
    },
    ExpiredOAuthToken,
    Auth(String),
    InvalidApiKeyEnv(VarError),
    Http(reqwest::Error),
    Io(std::io::Error),
    Json {
        provider: String,
        model: String,
        body_snippet: String,
        source: serde_json::Error,
    },
    Api {
        status: reqwest::StatusCode,
        error_type: Option<String>,
        message: Option<String>,
        request_id: Option<String>,
        body: String,
        retryable: bool,
        /// Suggested user action based on error type (e.g., "Reduce prompt size" for 413)
        suggested_action: Option<String>,
    },
    RetriesExhausted {
        attempts: u32,
        last_error: Box<ApiError>,
    },
    InvalidSseFrame(&'static str),
    BackoffOverflow {
        attempt: u32,
        base_delay: Duration,
    },
    RequestBodySizeExceeded {
        estimated_bytes: usize,
        max_bytes: usize,
        provider: &'static str,
    },
    /// Actionable, OpenAI-shaped error envelope returned by the Zero-X
    /// gateway (contract §3.3) or any backend that adds the same hints.
    Gateway(Box<GatewayError>),
    /// The stream ended without a `finish_reason` and without `[DONE]`, so
    /// the partial text it carried must not be treated as a complete reply.
    StreamTruncated {
        provider: String,
        model: String,
    },
}

impl ApiError {
    #[must_use]
    pub const fn missing_credentials(
        provider: &'static str,
        env_vars: &'static [&'static str],
    ) -> Self {
        Self::MissingCredentials {
            provider,
            env_vars,
            hint: None,
        }
    }

    /// Build a `MissingCredentials` error carrying an extra, runtime-computed
    /// hint string that the Display impl appends after the canonical "missing
    /// <provider> credentials" message. Used by the provider resolver to
    /// suggest the likely fix when the user has credentials for a different
    /// provider already in the environment.
    #[must_use]
    pub fn missing_credentials_with_hint(
        provider: &'static str,
        env_vars: &'static [&'static str],
        hint: impl Into<String>,
    ) -> Self {
        Self::MissingCredentials {
            provider,
            env_vars,
            hint: Some(hint.into()),
        }
    }

    /// Build a `Self::Json` enriched with the provider name, the model that
    /// was requested, and the first 200 characters of the raw response body so
    /// that callers can diagnose deserialization failures without re-running
    /// the request.
    #[must_use]
    pub fn json_deserialize(
        provider: impl Into<String>,
        model: impl Into<String>,
        body: &str,
        source: serde_json::Error,
    ) -> Self {
        Self::Json {
            provider: provider.into(),
            model: model.into(),
            body_snippet: truncate_body_snippet(body, 200),
            source,
        }
    }

    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Http(error) => error.is_connect() || error.is_timeout() || error.is_request(),
            Self::Api { retryable, .. } => *retryable,
            Self::Gateway(error) => error.is_retryable(),
            Self::RetriesExhausted { last_error, .. } => last_error.is_retryable(),
            Self::MissingCredentials { .. }
            | Self::ContextWindowExceeded { .. }
            | Self::ExpiredOAuthToken
            | Self::Auth(_)
            | Self::InvalidApiKeyEnv(_)
            | Self::Io(_)
            | Self::Json { .. }
            | Self::InvalidSseFrame(_)
            | Self::BackoffOverflow { .. }
            | Self::RequestBodySizeExceeded { .. }
            | Self::StreamTruncated { .. } => false,
        }
    }

    /// The gateway error envelope carried by this error, looking through
    /// [`ApiError::RetriesExhausted`].
    #[must_use]
    pub fn gateway_error(&self) -> Option<&GatewayError> {
        match self {
            Self::Gateway(error) => Some(error),
            Self::RetriesExhausted { last_error, .. } => last_error.gateway_error(),
            _ => None,
        }
    }

    /// Delay the server asked for before retrying, when it sent one.
    #[must_use]
    pub fn retry_after(&self) -> Option<Duration> {
        self.gateway_error().and_then(GatewayError::retry_after)
    }

    /// `true` when the gateway rejected the credential (`invalid_session`).
    #[must_use]
    pub fn is_invalid_session(&self) -> bool {
        self.gateway_error()
            .is_some_and(|error| error.code_kind() == Some(GatewayErrorCode::InvalidSession))
    }

    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Api { request_id, .. } => request_id.as_deref(),
            Self::Gateway(error) => error.request_id.as_deref(),
            Self::RetriesExhausted { last_error, .. } => last_error.request_id(),
            Self::MissingCredentials { .. }
            | Self::ContextWindowExceeded { .. }
            | Self::ExpiredOAuthToken
            | Self::Auth(_)
            | Self::InvalidApiKeyEnv(_)
            | Self::Http(_)
            | Self::Io(_)
            | Self::Json { .. }
            | Self::InvalidSseFrame(_)
            | Self::BackoffOverflow { .. }
            | Self::RequestBodySizeExceeded { .. }
            | Self::StreamTruncated { .. } => None,
        }
    }

    #[must_use]
    pub fn safe_failure_class(&self) -> &'static str {
        match self {
            Self::RetriesExhausted { .. } if self.is_context_window_failure() => "context_window",
            Self::RetriesExhausted { .. } if self.is_generic_fatal_wrapper() => {
                "provider_retry_exhausted"
            }
            Self::RetriesExhausted { last_error, .. } => last_error.safe_failure_class(),
            Self::MissingCredentials { .. } | Self::ExpiredOAuthToken | Self::Auth(_) => {
                "provider_auth"
            }
            Self::Api { status, .. } if matches!(status.as_u16(), 401 | 403) => "provider_auth",
            Self::ContextWindowExceeded { .. } => "context_window",
            Self::Gateway(error) => error.failure_class(),
            Self::Api { .. } if self.is_context_window_failure() => "context_window",
            Self::Api { status, .. } if status.as_u16() == 429 => "provider_rate_limit",
            Self::Api { .. } if self.is_generic_fatal_wrapper() => "provider_internal",
            Self::Api { .. } => "provider_error",
            Self::Http(_)
            | Self::InvalidSseFrame(_)
            | Self::BackoffOverflow { .. }
            | Self::StreamTruncated { .. } => "provider_transport",
            Self::InvalidApiKeyEnv(_) | Self::Io(_) | Self::Json { .. } => "runtime_io",
            Self::RequestBodySizeExceeded { .. } => "request_size",
        }
    }

    #[must_use]
    pub fn is_generic_fatal_wrapper(&self) -> bool {
        match self {
            Self::Api { message, body, .. } => {
                message
                    .as_deref()
                    .is_some_and(looks_like_generic_fatal_wrapper)
                    || looks_like_generic_fatal_wrapper(body)
            }
            Self::RetriesExhausted { last_error, .. } => last_error.is_generic_fatal_wrapper(),
            Self::MissingCredentials { .. }
            | Self::ContextWindowExceeded { .. }
            | Self::ExpiredOAuthToken
            | Self::Auth(_)
            | Self::InvalidApiKeyEnv(_)
            | Self::Http(_)
            | Self::Io(_)
            | Self::Json { .. }
            | Self::InvalidSseFrame(_)
            | Self::BackoffOverflow { .. }
            | Self::RequestBodySizeExceeded { .. }
            | Self::Gateway(_)
            | Self::StreamTruncated { .. } => false,
        }
    }

    #[must_use]
    pub fn is_context_window_failure(&self) -> bool {
        match self {
            Self::ContextWindowExceeded { .. } => true,
            Self::Api {
                status,
                message,
                body,
                ..
            } => {
                matches!(status.as_u16(), 400 | 413 | 422)
                    && (message
                        .as_deref()
                        .is_some_and(looks_like_context_window_error)
                        || looks_like_context_window_error(body))
            }
            Self::Gateway(error) => {
                error.code_kind() == Some(GatewayErrorCode::ContextLengthExceeded)
            }
            Self::RetriesExhausted { last_error, .. } => last_error.is_context_window_failure(),
            Self::MissingCredentials { .. }
            | Self::ExpiredOAuthToken
            | Self::Auth(_)
            | Self::InvalidApiKeyEnv(_)
            | Self::Http(_)
            | Self::Io(_)
            | Self::Json { .. }
            | Self::InvalidSseFrame(_)
            | Self::BackoffOverflow { .. }
            | Self::RequestBodySizeExceeded { .. }
            | Self::StreamTruncated { .. } => false,
        }
    }
}

impl Display for ApiError {
    #[allow(clippy::too_many_lines)]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingCredentials {
                provider,
                env_vars,
                hint,
            } => {
                write!(
                    f,
                    "missing {provider} credentials; export {} before calling the {provider} API",
                    env_vars.join(" or ")
                )?;
                if cfg!(target_os = "windows") {
                    if let Some(primary) = env_vars.first() {
                        write!(
                            f,
                            " (on Windows, environment variables set in PowerShell only persist for the current session; use `setx {primary} <value>` to make it permanent, then open a new terminal, or place a `.env` file containing `{primary}=<value>` in the current working directory)"
                        )?;
                    } else {
                        write!(
                            f,
                            " (on Windows, environment variables set in PowerShell only persist for the current session; use `setx` to make them permanent, then open a new terminal, or place a `.env` file in the current working directory)"
                        )?;
                    }
                }
                if let Some(hint) = hint {
                    write!(f, " — hint: {hint}")?;
                }
                Ok(())
            }
            Self::ContextWindowExceeded {
                model,
                estimated_input_tokens,
                requested_output_tokens,
                estimated_total_tokens,
                context_window_tokens,
            } => write!(
                f,
                "context_window_blocked for {model}: estimated input {estimated_input_tokens} + requested output {requested_output_tokens} = {estimated_total_tokens} tokens exceeds the {context_window_tokens}-token context window; compact the session or reduce request size before retrying"
            ),
            Self::ExpiredOAuthToken => {
                write!(
                    f,
                    "saved OAuth token is expired and no refresh token is available"
                )
            }
            Self::Auth(message) => write!(f, "auth error: {message}"),
            Self::InvalidApiKeyEnv(error) => {
                write!(f, "failed to read credential environment variable: {error}")
            }
            Self::Http(error) => write!(f, "http error: {error}"),
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json {
                provider,
                model,
                body_snippet,
                source,
            } => write!(
                f,
                "failed to parse {provider} response for model {model}: {source}; first 200 chars of body: {body_snippet}"
            ),
            Self::Api {
                status,
                error_type,
                message,
                request_id,
                body,
                ..
            } => {
                if let (Some(error_type), Some(message)) = (error_type, message) {
                    write!(f, "api returned {status} ({error_type})")?;
                    if let Some(request_id) = request_id {
                        write!(f, " [trace {request_id}]")?;
                    }
                    write!(f, ": {message}")
                } else {
                    write!(f, "api returned {status}")?;
                    if let Some(request_id) = request_id {
                        write!(f, " [trace {request_id}]")?;
                    }
                    write!(f, ": {body}")
                }
            }
            Self::RetriesExhausted {
                attempts,
                last_error,
            } => write!(f, "api failed after {attempts} attempts: {last_error}"),
            Self::InvalidSseFrame(message) => write!(f, "invalid sse frame: {message}"),
            Self::BackoffOverflow {
                attempt,
                base_delay,
            } => write!(
                f,
                "retry backoff overflowed on attempt {attempt} with base delay {base_delay:?}"
            ),
            Self::RequestBodySizeExceeded {
                estimated_bytes,
                max_bytes,
                provider,
            } => write!(
                f,
                "request body size ({estimated_bytes} bytes) exceeds {provider} limit ({max_bytes} bytes); reduce prompt length or context before retrying"
            ),
            Self::Gateway(error) => write!(f, "{error}"),
            Self::StreamTruncated { provider, model } => write!(
                f,
                "the {provider} stream for {model} ended before the reply completed (no finish_reason and no [DONE]); the partial reply was discarded, retry the request"
            ),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<reqwest::Error> for ApiError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

impl From<std::io::Error> for ApiError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json {
            provider: "unknown".to_string(),
            model: "unknown".to_string(),
            body_snippet: String::new(),
            source: value,
        }
    }
}

impl From<VarError> for ApiError {
    fn from(value: VarError) -> Self {
        Self::InvalidApiKeyEnv(value)
    }
}

/// Message shown whenever the gateway rejects the stored credential.
pub const INVALID_SESSION_MESSAGE: &str =
    "Your Zero-X session expired or was revoked — run neuron auth login";

/// Error codes defined by the gateway contract (`docs/GATEWAY_CONTRACT.md` §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayErrorCode {
    InvalidRequest,
    ContextLengthExceeded,
    InvalidSession,
    InsufficientCredits,
    UpgradeRequired,
    RegionUnavailable,
    AccountSuspended,
    InsufficientQuota,
    RateLimited,
    CapacityExhausted,
    UpstreamError,
    UpstreamTimeout,
    /// Contract v1.1: the requested model id is not in the catalog.
    ModelNotFound,
    /// Contract v1.1/v1.2: the gateway cannot serve the request right now
    /// (for example a session it could not index); safe to retry.
    ServiceUnavailable,
    /// Contract v1.1: a personal API key used where only a session works.
    ApiKeyNotAllowed,
}

impl GatewayErrorCode {
    #[must_use]
    pub fn parse(code: &str) -> Option<Self> {
        Some(match code {
            "invalid_request" => Self::InvalidRequest,
            "context_length_exceeded" => Self::ContextLengthExceeded,
            "invalid_session" => Self::InvalidSession,
            "insufficient_credits" => Self::InsufficientCredits,
            "upgrade_required" => Self::UpgradeRequired,
            "region_unavailable" => Self::RegionUnavailable,
            "account_suspended" => Self::AccountSuspended,
            "insufficient_quota" => Self::InsufficientQuota,
            "rate_limited" => Self::RateLimited,
            "capacity_exhausted" => Self::CapacityExhausted,
            "upstream_error" => Self::UpstreamError,
            "upstream_timeout" => Self::UpstreamTimeout,
            "model_not_found" => Self::ModelNotFound,
            "service_unavailable" => Self::ServiceUnavailable,
            "api_key_not_allowed" => Self::ApiKeyNotAllowed,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::ContextLengthExceeded => "context_length_exceeded",
            Self::InvalidSession => "invalid_session",
            Self::InsufficientCredits => "insufficient_credits",
            Self::UpgradeRequired => "upgrade_required",
            Self::RegionUnavailable => "region_unavailable",
            Self::AccountSuspended => "account_suspended",
            Self::InsufficientQuota => "insufficient_quota",
            Self::RateLimited => "rate_limited",
            Self::CapacityExhausted => "capacity_exhausted",
            Self::UpstreamError => "upstream_error",
            Self::UpstreamTimeout => "upstream_timeout",
            Self::ModelNotFound => "model_not_found",
            Self::ServiceUnavailable => "service_unavailable",
            Self::ApiKeyNotAllowed => "api_key_not_allowed",
        }
    }

    /// Codes a client may retry after the advertised delay. Every other code
    /// (quota, credits, plan, region, suspension, session, bad request) is
    /// final: retrying cannot succeed and may burn the user's quota.
    #[must_use]
    pub const fn is_transient(self) -> bool {
        matches!(
            self,
            Self::RateLimited
                | Self::CapacityExhausted
                | Self::UpstreamError
                | Self::UpstreamTimeout
                | Self::ServiceUnavailable
        )
    }

    /// HTTP status the contract pairs with this code, used when the code
    /// arrives inside a streamed `data:` frame without an HTTP status.
    #[must_use]
    pub const fn default_status(self) -> u16 {
        match self {
            Self::InvalidRequest | Self::ContextLengthExceeded => 400,
            Self::InvalidSession => 401,
            Self::InsufficientCredits => 402,
            Self::UpgradeRequired
            | Self::RegionUnavailable
            | Self::AccountSuspended
            | Self::ApiKeyNotAllowed => 403,
            Self::ModelNotFound => 404,
            Self::InsufficientQuota | Self::RateLimited => 429,
            Self::CapacityExhausted | Self::ServiceUnavailable => 503,
            Self::UpstreamError => 502,
            Self::UpstreamTimeout => 504,
        }
    }
}

/// Parsed OpenAI-shaped error envelope with the gateway's actionable hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayError {
    pub status: reqwest::StatusCode,
    pub code: Option<String>,
    pub error_type: Option<String>,
    pub message: String,
    pub request_id: Option<String>,
    pub upgrade_url: Option<String>,
    pub retry_after_seconds: Option<u64>,
    pub resets_at: Option<String>,
}

impl GatewayError {
    /// Parses `{"error": {...}}`, `{"error": "text"}` or a bare
    /// `{"message", "code"}` object. Returns `None` for non-JSON bodies.
    #[must_use]
    pub fn from_body(status: reqwest::StatusCode, body: &str) -> Option<Self> {
        let value = serde_json::from_str::<serde_json::Value>(body).ok()?;
        Self::from_value(status, &value)
    }

    /// Same as [`GatewayError::from_body`] for an already-parsed value.
    #[must_use]
    pub fn from_value(status: reqwest::StatusCode, value: &serde_json::Value) -> Option<Self> {
        let object = value.as_object()?;
        let error = match object.get("error") {
            Some(serde_json::Value::String(message)) => {
                return Some(Self::bare(status, message.clone()));
            }
            Some(error @ serde_json::Value::Object(_)) => error,
            None if object.contains_key("message") => value,
            Some(_) | None => return None,
        };
        let text = |key: &str| match error.get(key) {
            Some(serde_json::Value::String(text)) if !text.trim().is_empty() => {
                Some(text.trim().to_string())
            }
            Some(serde_json::Value::Number(number)) => Some(number.to_string()),
            _ => None,
        };
        let retry_after_seconds = match error.get("retry_after_seconds") {
            Some(serde_json::Value::Number(number)) => number.as_u64().or_else(|| {
                // Round fractional delays up so the client never retries early.
                number
                    .as_f64()
                    .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
                    .map(|delay| delay.as_secs() + u64::from(delay.subsec_nanos() > 0))
            }),
            Some(serde_json::Value::String(text)) => text.trim().parse().ok(),
            _ => None,
        };
        Some(Self {
            status,
            code: text("code"),
            error_type: text("type"),
            message: text("message").unwrap_or_default(),
            request_id: text("request_id"),
            upgrade_url: text("upgrade_url"),
            retry_after_seconds,
            resets_at: text("resets_at"),
        })
    }

    fn bare(status: reqwest::StatusCode, message: String) -> Self {
        Self {
            status,
            code: None,
            error_type: None,
            message,
            request_id: None,
            upgrade_url: None,
            retry_after_seconds: None,
            resets_at: None,
        }
    }

    /// Envelope for a response whose body carried no usable error object.
    #[must_use]
    pub fn from_status(status: reqwest::StatusCode) -> Self {
        Self::bare(
            status,
            status
                .canonical_reason()
                .unwrap_or("request failed")
                .to_string(),
        )
    }

    /// Fills the request id from the `x-request-id` header when the body
    /// did not carry one.
    #[must_use]
    pub fn with_request_id_fallback(mut self, request_id: Option<String>) -> Self {
        if self.request_id.is_none() {
            self.request_id = request_id;
        }
        self
    }

    /// Fills `retry_after_seconds` from the `Retry-After` header when the
    /// body did not carry it.
    #[must_use]
    pub fn with_retry_after_fallback(mut self, retry_after: Option<Duration>) -> Self {
        if self.retry_after_seconds.is_none() {
            // A saturated header value is not a meaningful delay to show.
            self.retry_after_seconds = retry_after
                .filter(|delay| *delay != Duration::MAX)
                .map(|delay| delay.as_secs());
        }
        self
    }

    #[must_use]
    pub fn code_kind(&self) -> Option<GatewayErrorCode> {
        self.code.as_deref().and_then(GatewayErrorCode::parse)
    }

    /// Whether the envelope carries anything beyond a plain message: a
    /// contract code, an upgrade link, a reset time or a retry delay.
    #[must_use]
    pub fn has_actionable_hints(&self) -> bool {
        self.code_kind().is_some()
            || self.upgrade_url.is_some()
            || self.resets_at.is_some()
            || self.retry_after_seconds.is_some()
    }

    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self.code_kind() {
            Some(code) => code.is_transient(),
            None => matches!(
                self.status.as_u16(),
                408 | 409 | 429 | 500 | 502 | 503 | 504
            ),
        }
    }

    #[must_use]
    pub fn retry_after(&self) -> Option<Duration> {
        self.retry_after_seconds.map(Duration::from_secs)
    }

    #[must_use]
    pub fn failure_class(&self) -> &'static str {
        match self.code_kind() {
            Some(
                GatewayErrorCode::InvalidSession
                | GatewayErrorCode::AccountSuspended
                | GatewayErrorCode::ApiKeyNotAllowed,
            ) => "provider_auth",
            Some(GatewayErrorCode::ContextLengthExceeded) => "context_window",
            Some(
                GatewayErrorCode::InsufficientQuota
                | GatewayErrorCode::RateLimited
                | GatewayErrorCode::CapacityExhausted,
            ) => "provider_rate_limit",
            Some(
                GatewayErrorCode::InsufficientCredits
                | GatewayErrorCode::UpgradeRequired
                | GatewayErrorCode::RegionUnavailable,
            ) => "provider_plan",
            Some(
                GatewayErrorCode::InvalidRequest
                | GatewayErrorCode::UpstreamError
                | GatewayErrorCode::UpstreamTimeout
                | GatewayErrorCode::ModelNotFound
                | GatewayErrorCode::ServiceUnavailable,
            ) => "provider_error",
            None => match self.status.as_u16() {
                401 | 403 => "provider_auth",
                429 => "provider_rate_limit",
                _ => "provider_error",
            },
        }
    }
}

impl Display for GatewayError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.code_kind() == Some(GatewayErrorCode::InvalidSession) {
            f.write_str(INVALID_SESSION_MESSAGE)?;
        } else {
            let message = self.message.trim();
            if message.is_empty() {
                write!(f, "gateway returned {}", self.status)?;
            } else {
                f.write_str(message)?;
            }
            if let Some(code) = self.code.as_deref().filter(|code| !message.contains(code)) {
                write!(f, " ({code})")?;
            }
        }
        if let Some(resets_at) = &self.resets_at {
            write!(f, "; resets at {resets_at}")?;
        } else if let Some(seconds) = self.retry_after_seconds {
            write!(f, "; retry in {seconds}s")?;
        }
        if let Some(upgrade_url) = &self.upgrade_url {
            write!(f, "; upgrade: {upgrade_url}")?;
        }
        if let Some(request_id) = &self.request_id {
            write!(f, "; request {request_id}")?;
        }
        Ok(())
    }
}

fn looks_like_generic_fatal_wrapper(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    GENERIC_FATAL_WRAPPER_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
}

fn looks_like_context_window_error(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    CONTEXT_WINDOW_ERROR_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
}

/// Truncate `body` so the resulting snippet contains at most `max_chars`
/// characters (counted by Unicode scalar values, not bytes), preserving the
/// leading slice of the body that the caller most often needs to inspect.
fn truncate_body_snippet(body: &str, max_chars: usize) -> String {
    let mut taken_chars = 0;
    let mut byte_end = 0;
    for (offset, character) in body.char_indices() {
        if taken_chars >= max_chars {
            break;
        }
        taken_chars += 1;
        byte_end = offset + character.len_utf8();
    }
    if taken_chars >= max_chars && byte_end < body.len() {
        format!("{}…", &body[..byte_end])
    } else {
        body[..byte_end].to_string()
    }
}

#[cfg(test)]
mod gateway_error_tests {
    use std::time::Duration;

    use super::{ApiError, GatewayError, GatewayErrorCode, INVALID_SESSION_MESSAGE};

    fn status(code: u16) -> reqwest::StatusCode {
        reqwest::StatusCode::from_u16(code).unwrap()
    }

    #[test]
    fn parses_every_contract_field_and_renders_actionable_display() {
        let body = r#"{"error":{"message":"Daily request limit reached for the Free plan","type":"insufficient_quota","code":"insufficient_quota","param":null,"request_id":"req_quota_1","upgrade_url":"https://zero-x.live/pricing","retry_after_seconds":3600,"resets_at":"2026-10-17T00:00:00Z"}}"#;
        let error = GatewayError::from_body(status(429), body).expect("envelope parses");
        assert_eq!(error.code_kind(), Some(GatewayErrorCode::InsufficientQuota));
        assert_eq!(error.error_type.as_deref(), Some("insufficient_quota"));
        assert_eq!(error.request_id.as_deref(), Some("req_quota_1"));
        assert_eq!(
            error.upgrade_url.as_deref(),
            Some("https://zero-x.live/pricing")
        );
        assert_eq!(error.retry_after_seconds, Some(3600));
        assert_eq!(error.resets_at.as_deref(), Some("2026-10-17T00:00:00Z"));
        assert!(!error.is_retryable(), "insufficient_quota must never retry");

        let rendered = ApiError::Gateway(Box::new(error)).to_string();
        assert!(rendered.starts_with("Daily request limit reached for the Free plan"));
        assert!(rendered.contains("(insufficient_quota)"));
        assert!(rendered.contains("resets at 2026-10-17T00:00:00Z"));
        assert!(rendered.contains("upgrade: https://zero-x.live/pricing"));
        assert!(rendered.contains("request req_quota_1"));
    }

    #[test]
    fn terminal_codes_never_retry_and_transient_codes_do() {
        for code in [
            "insufficient_quota",
            "insufficient_credits",
            "upgrade_required",
            "region_unavailable",
            "account_suspended",
            "invalid_session",
            "invalid_request",
            "context_length_exceeded",
            "model_not_found",
            "api_key_not_allowed",
        ] {
            let body = format!(r#"{{"error":{{"message":"no","code":"{code}"}}}}"#);
            // Even with a normally-retryable status the code wins.
            let error = GatewayError::from_body(status(503), &body).unwrap();
            assert!(!error.is_retryable(), "{code} must not be retried");
            assert!(!ApiError::Gateway(Box::new(error)).is_retryable());
        }
        for code in [
            "rate_limited",
            "capacity_exhausted",
            "upstream_error",
            "upstream_timeout",
            "service_unavailable",
        ] {
            let body = format!(r#"{{"error":{{"message":"busy","code":"{code}"}}}}"#);
            let error = GatewayError::from_body(status(503), &body).unwrap();
            assert!(error.is_retryable(), "{code} should be retried");
        }
    }

    #[test]
    fn invalid_session_renders_the_login_instruction() {
        let body = r#"{"error":{"message":"Session expired. Run `neuron auth login`.","type":"authentication_error","code":"invalid_session","request_id":"req_401"}}"#;
        let error = ApiError::Gateway(Box::new(
            GatewayError::from_body(status(401), body).unwrap(),
        ));
        assert!(error.is_invalid_session());
        assert_eq!(error.safe_failure_class(), "provider_auth");
        let rendered = error.to_string();
        assert!(rendered.starts_with(INVALID_SESSION_MESSAGE), "{rendered}");
        assert!(rendered.ends_with("; request req_401"), "{rendered}");
    }

    #[test]
    fn accepts_string_errors_numeric_codes_and_header_fallbacks() {
        let legacy =
            GatewayError::from_body(status(401), r#"{"error":"Invalid or expired session"}"#)
                .unwrap();
        assert_eq!(legacy.message, "Invalid or expired session");
        assert_eq!(legacy.code, None);

        let numeric = GatewayError::from_body(
            status(429),
            r#"{"error":{"message":"slow down","code":429,"retry_after_seconds":1.2}}"#,
        )
        .unwrap();
        assert_eq!(numeric.code.as_deref(), Some("429"));
        assert_eq!(numeric.code_kind(), None);
        assert_eq!(numeric.retry_after_seconds, Some(2), "fractions round up");
        assert!(numeric.is_retryable(), "unknown codes fall back to status");

        let filled = GatewayError::from_body(status(503), r#"{"error":{"message":"x"}}"#)
            .unwrap()
            .with_request_id_fallback(Some("req_header".to_string()))
            .with_retry_after_fallback(Some(Duration::from_secs(7)));
        assert_eq!(filled.request_id.as_deref(), Some("req_header"));
        assert_eq!(filled.retry_after(), Some(Duration::from_secs(7)));

        assert!(GatewayError::from_body(status(502), "<html>Bad gateway</html>").is_none());
        assert_eq!(
            GatewayError::from_status(status(502)).to_string(),
            "Bad Gateway"
        );
    }

    #[test]
    fn context_length_code_is_a_context_window_failure() {
        let error = ApiError::Gateway(Box::new(
            GatewayError::from_body(
                status(400),
                r#"{"error":{"message":"No model fits","code":"context_length_exceeded"}}"#,
            )
            .unwrap(),
        ));
        assert!(error.is_context_window_failure());
        assert_eq!(error.safe_failure_class(), "context_window");
    }

    #[test]
    fn stream_truncated_is_distinct_and_not_retryable() {
        let error = ApiError::StreamTruncated {
            provider: "Zero-X".to_string(),
            model: "auto".to_string(),
        };
        assert!(!error.is_retryable());
        assert_eq!(error.safe_failure_class(), "provider_transport");
        assert!(error
            .to_string()
            .contains("ended before the reply completed"));
    }
}

#[cfg(test)]
mod tests {
    use super::{truncate_body_snippet, ApiError};

    #[test]
    fn json_deserialize_error_includes_provider_model_and_truncated_body_snippet() {
        let raw_body = format!("{}{}", "x".repeat(190), "_TAIL_PAST_200_CHARS_MARKER_");
        let source = serde_json::from_str::<serde_json::Value>("{not json")
            .expect_err("invalid json should fail to parse");

        let error = ApiError::json_deserialize("Anthropic", "claude-opus-4-6", &raw_body, source);
        let rendered = error.to_string();

        assert!(
            rendered.starts_with("failed to parse Anthropic response for model claude-opus-4-6: "),
            "rendered error should lead with provider and model: {rendered}"
        );
        assert!(
            rendered.contains("first 200 chars of body: "),
            "rendered error should label the body snippet: {rendered}"
        );
        let snippet = rendered
            .split("first 200 chars of body: ")
            .nth(1)
            .expect("snippet section should be present");
        assert!(
            snippet.starts_with(&"x".repeat(190)),
            "snippet should preserve the leading characters of the body: {snippet}"
        );
        assert!(
            snippet.ends_with('…'),
            "snippet should signal truncation with an ellipsis: {snippet}"
        );
        assert!(
            !snippet.contains("_TAIL_PAST_200_CHARS_MARKER_"),
            "snippet should drop characters past the 200-char cap: {snippet}"
        );
        assert_eq!(error.safe_failure_class(), "runtime_io");
        assert_eq!(error.request_id(), None);
        assert!(!error.is_retryable());
    }

    #[test]
    fn truncate_body_snippet_keeps_short_bodies_intact() {
        assert_eq!(truncate_body_snippet("hello", 200), "hello");
        assert_eq!(truncate_body_snippet("", 200), "");
    }

    #[test]
    fn truncate_body_snippet_caps_long_bodies_at_max_chars() {
        let body = "a".repeat(250);
        let snippet = truncate_body_snippet(&body, 200);
        assert_eq!(snippet.chars().count(), 201, "200 chars + ellipsis");
        assert!(snippet.ends_with('…'));
        assert!(snippet.starts_with(&"a".repeat(200)));
    }

    #[test]
    fn truncate_body_snippet_does_not_split_multibyte_characters() {
        let body = "한글한글한글한글한글한글";
        let snippet = truncate_body_snippet(body, 4);
        assert_eq!(snippet, "한글한글…");
    }

    #[test]
    fn detects_generic_fatal_wrapper_and_classifies_it_as_provider_internal() {
        let error = ApiError::Api {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            error_type: Some("api_error".to_string()),
            message: Some(
                "Something went wrong while processing your request. Please try again, or use /new to start a fresh session."
                    .to_string(),
            ),
            request_id: Some("req_jobdori_123".to_string()),
            body: String::new(),
            retryable: true,
            suggested_action: None,
        };

        assert!(error.is_generic_fatal_wrapper());
        assert_eq!(error.safe_failure_class(), "provider_internal");
        assert_eq!(error.request_id(), Some("req_jobdori_123"));
        assert!(error.to_string().contains("[trace req_jobdori_123]"));
    }

    #[test]
    fn retries_exhausted_preserves_nested_request_id_and_failure_class() {
        let error = ApiError::RetriesExhausted {
            attempts: 3,
            last_error: Box::new(ApiError::Api {
                status: reqwest::StatusCode::BAD_GATEWAY,
                error_type: Some("api_error".to_string()),
                message: Some(
                    "Something went wrong while processing your request. Please try again, or use /new to start a fresh session."
                        .to_string(),
                ),
                request_id: Some("req_nested_456".to_string()),
                body: String::new(),
                retryable: true,
                suggested_action: None,
            }),
        };

        assert!(error.is_generic_fatal_wrapper());
        assert_eq!(error.safe_failure_class(), "provider_retry_exhausted");
        assert_eq!(error.request_id(), Some("req_nested_456"));
    }

    #[test]
    fn classifies_provider_context_window_errors() {
        let error = ApiError::Api {
            status: reqwest::StatusCode::BAD_REQUEST,
            error_type: Some("invalid_request_error".to_string()),
            message: Some(
                "This model's maximum context length is 200000 tokens, but your request used 230000 tokens."
                    .to_string(),
            ),
            request_id: Some("req_ctx_123".to_string()),
            body: String::new(),
            retryable: false,
            suggested_action: None,
        };

        assert!(error.is_context_window_failure());
        assert_eq!(error.safe_failure_class(), "context_window");
        assert_eq!(error.request_id(), Some("req_ctx_123"));
    }

    #[test]
    fn missing_credentials_without_hint_renders_the_canonical_message() {
        // given
        let error = ApiError::missing_credentials(
            "Anthropic",
            &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"],
        );

        // when
        let rendered = error.to_string();

        // then
        assert!(
            rendered.starts_with(
                "missing Anthropic credentials; export ANTHROPIC_AUTH_TOKEN or ANTHROPIC_API_KEY before calling the Anthropic API"
            ),
            "rendered error should lead with the canonical missing-credential message: {rendered}"
        );
        assert!(
            !rendered.contains(" — hint: "),
            "no hint should be appended when none is supplied: {rendered}"
        );
    }

    #[test]
    fn missing_credentials_with_hint_appends_the_hint_after_base_message() {
        // given
        let error = ApiError::missing_credentials_with_hint(
            "Anthropic",
            &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"],
            "I see OPENAI_API_KEY is set — if you meant to use the OpenAI-compat provider, prefix your model name with `openai/` so prefix routing selects it.",
        );

        // when
        let rendered = error.to_string();

        // then
        assert!(
            rendered.starts_with("missing Anthropic credentials;"),
            "hint should be appended, not replace the base message: {rendered}"
        );
        let hint_marker = " — hint: I see OPENAI_API_KEY is set — if you meant to use the OpenAI-compat provider, prefix your model name with `openai/` so prefix routing selects it.";
        assert!(
            rendered.ends_with(hint_marker),
            "rendered error should end with the hint: {rendered}"
        );
        // Classification semantics are unaffected by the presence of a hint.
        assert_eq!(error.safe_failure_class(), "provider_auth");
        assert!(!error.is_retryable());
        assert_eq!(error.request_id(), None);
    }
}
