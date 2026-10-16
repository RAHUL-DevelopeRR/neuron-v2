#![allow(
    clippy::collapsible_match,
    clippy::duration_suboptimal_units,
    clippy::map_unwrap_or
)]

mod client;
mod error;
mod http_client;
mod prompt_cache;
mod providers;
mod retry;
mod sse;
mod types;

pub use client::{
    oauth_token_is_expired, read_base_url, read_xai_base_url, resolve_saved_oauth_token,
    resolve_startup_auth_source, MessageStream, OAuthTokenSet, ProviderClient,
};
pub use error::{ApiError, GatewayError, GatewayErrorCode, INVALID_SESSION_MESSAGE};
pub use http_client::{
    build_http_client, build_http_client_or_default, build_http_client_with, ProxyConfig,
    USER_AGENT,
};
pub use prompt_cache::{
    CacheBreakEvent, PromptCache, PromptCacheConfig, PromptCachePaths, PromptCacheRecord,
    PromptCacheStats,
};
pub use providers::anthropic::{AnthropicClient, AnthropicClient as ApiClient, AuthSource};
pub use providers::openai_compat::{
    build_chat_completion_request, flatten_tool_result_content, install_openai_compat_endpoint,
    installed_openai_compat_endpoint, is_reasoning_model, model_rejects_is_error_field,
    translate_message, OpenAiCompatClient, OpenAiCompatConfig, OpenAiCompatEndpoint, UsageObserver,
    DEFAULT_ZERO_X_BASE_URL, ZERO_X_PROVIDER_NAME,
};
pub use providers::{
    detect_provider_kind, max_tokens_for_model, max_tokens_for_model_with_override,
    model_token_limit, preflight_message_request, resolve_model_alias, ModelTokenLimit,
    ProviderKind,
};
pub use retry::{
    RetryNotice, RetryObserver, CAPACITY_MAX_WAIT, CAPACITY_TOTAL_BACKOFF, MAX_TOTAL_BACKOFF,
};
pub use sse::{parse_frame, SseParser};
pub use types::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    InputContentBlock, InputMessage, MessageDelta, MessageDeltaEvent, MessageRequest,
    MessageResponse, MessageStartEvent, MessageStopEvent, OutputContentBlock, StreamEvent,
    ToolChoice, ToolDefinition, ToolResultContentBlock, Usage,
};

pub use telemetry::{
    AnalyticsEvent, AnthropicRequestProfile, ClientIdentity, JsonlTelemetrySink,
    MemoryTelemetrySink, SessionTraceRecord, SessionTracer, TelemetryEvent, TelemetrySink,
    DEFAULT_ANTHROPIC_VERSION,
};
