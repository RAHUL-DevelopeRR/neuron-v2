//! Anthropic API client implementation.
//!
//! AnthropicRuntimeClient, auth resolution, and error formatting.

// A turn's output goes through a TurnSink or runtime::ui_hooks, so a full-screen UI can own
// the terminal.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use super::*;
use api::{self, AuthSource, RetryProgress};
use runtime::{CancellationToken, RuntimeErrorKind};
use std::collections::BTreeMap;
use std::env;
use std::io;

/// Default time to wait for the first stream event of a fresh request.
pub(crate) const DEFAULT_FIRST_TOKEN_TIMEOUT: Duration = Duration::from_secs(90);
/// Default time to wait for the first stream event after tool results.
pub(crate) const DEFAULT_POST_TOOL_FIRST_TOKEN_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest tolerated gap between two stream events once streaming started.
pub(crate) const DEFAULT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// Overrides both first-token timeouts (seconds).
pub(crate) const FIRST_TOKEN_TIMEOUT_ENV_VAR: &str = "NEURON_FIRST_TOKEN_TIMEOUT_SECS";
/// A stalled or interrupted request is retried once, and only while the
/// provider client's own retry limit allows another request.
const MAX_STREAM_ATTEMPTS: u32 = 2;

/// Deadlines applied while consuming a model stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StreamTimeouts {
    pub first_token: Duration,
    pub post_tool_first_token: Duration,
    pub idle: Duration,
}

impl Default for StreamTimeouts {
    fn default() -> Self {
        Self {
            first_token: DEFAULT_FIRST_TOKEN_TIMEOUT,
            post_tool_first_token: DEFAULT_POST_TOOL_FIRST_TOKEN_TIMEOUT,
            idle: DEFAULT_STREAM_IDLE_TIMEOUT,
        }
    }
}

impl StreamTimeouts {
    /// Defaults with `NEURON_FIRST_TOKEN_TIMEOUT_SECS` applied to both
    /// first-token deadlines when it holds a positive integer.
    pub(crate) fn from_env() -> Self {
        Self::from_override(env::var(FIRST_TOKEN_TIMEOUT_ENV_VAR).ok().as_deref())
    }

    pub(crate) fn from_override(value: Option<&str>) -> Self {
        let mut timeouts = Self::default();
        if let Some(seconds) = value
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|seconds| *seconds > 0)
        {
            timeouts.first_token = Duration::from_secs(seconds);
            timeouts.post_tool_first_token = Duration::from_secs(seconds);
        }
        timeouts
    }

    pub(crate) fn first_token_for(self, after_tool_results: bool) -> Duration {
        if after_tool_results {
            self.post_tool_first_token
        } else {
            self.first_token
        }
    }
}

/// Why consuming one model stream failed. Retry decisions match on this type,
/// never on error text.
#[derive(Debug)]
pub(crate) enum StreamFailure {
    /// No stream event arrived before the first-token deadline.
    FirstTokenTimeout {
        waited: Duration,
        after_tool_results: bool,
    },
    /// Streaming started but then went silent for longer than the idle limit.
    Idle { waited: Duration },
    /// The stream broke off before the response completed (connection reset,
    /// truncated SSE body, provider stream error).
    Interrupted { detail: String },
    /// The turn was cancelled while waiting, after `partial_text` had streamed.
    Cancelled { partial_text: String },
    /// A failure that retrying the same request will not fix.
    Fatal(RuntimeError),
}

impl StreamFailure {
    pub(crate) fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::FirstTokenTimeout { .. } | Self::Idle { .. } | Self::Interrupted { .. }
        )
    }

    pub(crate) fn describe(&self) -> String {
        match self {
            Self::FirstTokenTimeout {
                waited,
                after_tool_results: true,
            } => format!(
                "The model did not respond within {}s after the tool results",
                waited.as_secs()
            ),
            Self::FirstTokenTimeout { waited, .. } => format!(
                "The model did not start responding within {}s",
                waited.as_secs()
            ),
            Self::Idle { waited } => format!(
                "The model stream stalled (no data for {}s)",
                waited.as_secs()
            ),
            Self::Interrupted { detail } => {
                format!("The response stream was interrupted ({detail})")
            }
            Self::Cancelled { .. } => "Request cancelled".to_string(),
            Self::Fatal(error) => error.to_string(),
        }
    }

    /// The error reported for this failure after `retries` retries of the
    /// model call (the provider client's and the stream retry together).
    pub(crate) fn into_runtime_error(self, retries: u32) -> RuntimeError {
        let kind = match &self {
            Self::FirstTokenTimeout { .. } | Self::Idle { .. } => RuntimeErrorKind::Timeout,
            Self::Interrupted { .. } => RuntimeErrorKind::StreamInterrupted,
            Self::Cancelled { .. } => RuntimeErrorKind::Cancelled,
            Self::Fatal(_) => RuntimeErrorKind::Other,
        };
        match self {
            Self::Fatal(error) => error,
            Self::Cancelled { partial_text } => {
                let error = RuntimeError::with_kind(kind, "request cancelled by user");
                if partial_text.is_empty() {
                    error
                } else {
                    error.with_partial_text(partial_text)
                }
            }
            other => {
                let after = match retries {
                    0 => String::new(),
                    1 => " (after one retry)".to_string(),
                    retries => format!(" (after {retries} retries)"),
                };
                // A `neuron prompt` run has no REPL to type `/model` into.
                let switch = if crate::headless::one_shot() {
                    "pick another model with --model"
                } else {
                    "switch models with /model"
                };
                RuntimeError::with_kind(
                    kind,
                    format!(
                        "{}{after}. The provider may be overloaded; try again, or {switch}.",
                        other.describe()
                    ),
                )
            }
        }
    }
}

/// The first-token deadline of one request, measured in active time: the
/// waits the provider client spends between its own retries (honouring
/// `Retry-After`) do not count, so a capacity wait is never cut short and
/// the deadline restarts with each new request the client sends.
struct FirstTokenClock<'a> {
    progress: &'a RetryProgress,
    started: Instant,
    waited_before: Duration,
    budget: Duration,
}

impl<'a> FirstTokenClock<'a> {
    fn start(progress: &'a RetryProgress, budget: Duration) -> Self {
        Self {
            progress,
            started: Instant::now(),
            waited_before: progress.waited(),
            budget,
        }
    }

    /// Time since the start that was not spent waiting between retries.
    fn elapsed(&self) -> Duration {
        let paused = self.progress.waited().saturating_sub(self.waited_before);
        self.started.elapsed().saturating_sub(paused)
    }

    fn remaining(&self) -> Duration {
        self.budget.saturating_sub(self.elapsed())
    }

    /// Resolves once the active time reaches the budget.
    async fn expired(&self) {
        loop {
            let remaining = self.remaining();
            if remaining.is_zero() {
                return;
            }
            // Active time grows at most as fast as wall time, so sleeping
            // for what remains never passes the deadline. When a retry wait
            // fell into the sleep, time remains and the loop sleeps again.
            tokio::time::sleep(remaining).await;
        }
    }
}

/// The text deltas streamed so far, which the user has already seen.
fn streamed_text(events: &[AssistantEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            AssistantEvent::TextDelta(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// Classifies an error returned while reading the stream. Provider status
/// errors keep their own retry semantics (`is_retryable`); transport-level
/// failures, including truncated streams, are treated as interruptions.
pub(crate) fn classify_stream_read_error(session_id: &str, error: &api::ApiError) -> StreamFailure {
    let detail = format_user_visible_api_error(session_id, error);
    match error {
        api::ApiError::Api { .. }
        | api::ApiError::RetriesExhausted { .. }
        | api::ApiError::MissingCredentials { .. }
        | api::ApiError::ContextWindowExceeded { .. }
        | api::ApiError::ExpiredOAuthToken
        | api::ApiError::Auth(_)
        | api::ApiError::InvalidApiKeyEnv(_)
        | api::ApiError::BackoffOverflow { .. }
        | api::ApiError::RequestBodySizeExceeded { .. } => {
            if error.is_retryable() {
                StreamFailure::Interrupted { detail }
            } else {
                fatal_api_error(session_id, error)
            }
        }
        _ => StreamFailure::Interrupted { detail },
    }
}

/// A provider error that resending the identical request will not fix.
/// Context-window rejections keep a typed kind so the runtime can shrink the
/// context and retry once.
pub(crate) fn fatal_api_error(session_id: &str, error: &api::ApiError) -> StreamFailure {
    let kind = if error.is_context_window_failure() {
        RuntimeErrorKind::ContextWindowExceeded
    } else {
        RuntimeErrorKind::Other
    };
    let mut message = format_user_visible_api_error(session_id, error);
    if let Some(hint) = switch_model_hint(error, crate::headless::one_shot()) {
        message.push_str(hint);
    }
    StreamFailure::Fatal(RuntimeError::with_kind(kind, message))
}

/// For the gateway's `capacity_exhausted`, whose advice is to wait or upgrade (even on
/// the highest plan): another model's shared pool may well have room now. A one-shot
/// `neuron prompt` run has no REPL to type `/model` into: it names the flag.
fn switch_model_hint(error: &api::ApiError, one_shot: bool) -> Option<&'static str> {
    (error.gateway_error().and_then(api::GatewayError::code_kind)
        == Some(api::GatewayErrorCode::CapacityExhausted))
    .then_some(if one_shot {
        ". Other models may have capacity now: pick another with --model."
    } else {
        ". Other models may have capacity now: switch with /model."
    })
}

/// Stop reason reported for tool calls the provider closed without
/// completing them (the output token limit cut them off).
const OUTPUT_LIMIT_STOP_REASON: &str = "max_tokens";

/// Whether a stream error means the provider ended the message while
/// tool-call blocks were still open. The OpenAI-compatible stream reports a
/// tool call that ends with a finish reason other than `tool_calls` (such as
/// `length`) as an invalid frame once the body is complete. Retrying the
/// identical request would hit the same output limit again, so this is not
/// an interruption.
pub(crate) fn ends_unterminated_tool_call(error: &api::ApiError, tool_blocks_open: bool) -> bool {
    tool_blocks_open && matches!(error, api::ApiError::InvalidSseFrame(_))
}

/// Terminal line for a tool call of a response the output limit cut off. None of
/// them runs: one with incomplete arguments is to be split, a complete one made again.
pub(crate) fn format_cut_off_tool_call(name: &str, input: &str) -> String {
    if serde_json::from_str::<serde_json::Value>(input).is_ok() {
        format!(
            "\x1b[33m\u{26a0}\x1b[0m `{name}` call not run: the response hit the output token limit \u{2014} the model will be asked to make it again"
        )
    } else {
        format!(
            "\x1b[33m\u{26a0}\x1b[0m `{name}` call was cut off at the output token limit \u{2014} not run; the model will be asked to split it into smaller steps"
        )
    }
}

/// Shows the finished tool calls whose display waited for the stop reason: a call
/// of a response the output limit cut off is shown as one that will not run.
fn show_tool_calls(
    sink: &mut dyn TurnSink,
    calls: &mut Vec<(String, String, String)>,
    cut_off: bool,
) -> io::Result<()> {
    for (id, name, input) in calls.drain(..) {
        sink.tool_call(&id, &name, &input, cut_off)?;
    }
    Ok(())
}

pub(crate) struct AnthropicRuntimeClient {
    runtime: tokio::runtime::Runtime,
    client: ApiProviderClient,
    session_id: String,
    model: String,
    enable_tools: bool,
    output: TurnOutput,
    allowed_tools: Option<AllowedToolSet>,
    tool_registry: GlobalToolRegistry,
    progress_reporter: Option<InternalPromptProgressReporter>,
    reasoning_effort: Option<String>,
    timeouts: StreamTimeouts,
    /// The turn (by the index of its prompt) whose request was too large with the
    /// deferred MCP tool definitions: its later requests leave them out at once.
    oversized_with_deferred_tools: Option<usize>,
}

/// The notice shown when a request that was too large is sent again without the
/// `count` MCP tool definitions `ToolSearch` brought in.
pub(crate) fn deferred_tools_dropped_notice(count: usize) -> String {
    let definitions = if count == 1 {
        "definition"
    } else {
        "definitions"
    };
    format!(
        "\n\u{26a0} The request is too large for this model's limit \u{2014} retrying once without the {count} MCP tool {definitions} found by ToolSearch (MCPTool can still call them)\u{2026}"
    )
}

/// Builds the provider client for `model`, routing Anthropic models through
/// the CLI auth source (and `ANTHROPIC_BASE_URL`) and everything else through
/// the OpenAI-compatible client configured from the environment.
pub(crate) fn build_provider_client(
    model: &str,
    session_id: Option<&str>,
) -> Result<ApiProviderClient, Box<dyn std::error::Error>> {
    // Dispatch to the correct provider at construction time.
    // `ApiProviderClient` (exposed by the api crate as `ProviderClient`) is
    // an enum over Anthropic / xAI / OpenAI variants, where xAI and OpenAI
    // both use the OpenAI-compat wire format. `detect_provider_kind` is
    // consulted so model-name prefix routing (`openai/`, `gpt-`, `grok`,
    // `qwen/`) wins over env-var presence.
    //
    // For Anthropic we build the client directly so `api::read_base_url()`
    // (ANTHROPIC_BASE_URL, required by the mock-server harness) applies, and
    // attach a session-scoped prompt cache when a session id is known.
    let resolved_model = api::resolve_model_alias(model);
    Ok(match detect_provider_kind(&resolved_model) {
        ProviderKind::Anthropic => {
            let auth = resolve_cli_auth_source()?;
            let inner = AnthropicClient::from_auth(auth).with_base_url(api::read_base_url());
            let inner = match session_id {
                Some(session_id) => inner.with_prompt_cache(PromptCache::new(session_id)),
                None => inner,
            };
            ApiProviderClient::Anthropic(inner)
        }
        // `from_model_with_anthropic_auth(.., None)` builds an
        // `OpenAiCompatClient::from_env` with the matching config (openai /
        // xai / dashscope), covering OpenAI, OpenRouter, xAI, DashScope,
        // Ollama and the Zero-X gateway.
        ProviderKind::Xai | ProviderKind::OpenAi => {
            ApiProviderClient::from_model_with_anthropic_auth(&resolved_model, None)?
        }
    })
}

impl AnthropicRuntimeClient {
    pub fn new(
        session_id: &str,
        model: String,
        enable_tools: bool,
        output: TurnOutput,
        allowed_tools: Option<AllowedToolSet>,
        tool_registry: GlobalToolRegistry,
        progress_reporter: Option<InternalPromptProgressReporter>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let client = build_provider_client(&model, Some(session_id))?;
        Ok(Self {
            runtime: tokio::runtime::Runtime::new()?,
            client,
            session_id: session_id.to_string(),
            model,
            enable_tools,
            output,
            allowed_tools,
            tool_registry,
            progress_reporter,
            reasoning_effort: None,
            timeouts: StreamTimeouts::from_env(),
            oversized_with_deferred_tools: None,
        })
    }

    pub fn set_reasoning_effort(&mut self, effort: Option<String>) {
        self.reasoning_effort = effort;
    }

    pub fn reasoning_effort(&self) -> Option<&str> {
        self.reasoning_effort.as_deref()
    }

    pub(crate) fn output(&self) -> &TurnOutput {
        &self.output
    }

    pub fn allowed_tools(&self) -> Option<&AllowedToolSet> {
        self.allowed_tools.as_ref()
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn set_timeouts(&mut self, timeouts: StreamTimeouts) {
        self.timeouts = timeouts;
    }

    /// The deferred MCP tool definitions `request` carries: the ones its conversation
    /// brought into play. None with an allow-list, which names every tool itself.
    fn deferred_mcp_tools(&self, request: &ApiRequest) -> Vec<ToolDefinition> {
        if !self.enable_tools || self.allowed_tools.is_some() {
            return Vec::new();
        }
        self.tool_registry
            .deferred_mcp_definitions(&request.messages)
    }

    fn message_request(
        &self,
        request: &ApiRequest,
        deferred_mcp_tools: Vec<ToolDefinition>,
    ) -> MessageRequest {
        MessageRequest {
            model: self.model.clone(),
            max_tokens: max_tokens_for_model(&self.model),
            messages: convert_messages(&request.messages),
            system: (!request.system_prompt.is_empty()).then(|| request.system_prompt.join("\n\n")),
            tools: self.enable_tools.then(|| {
                let mut definitions =
                    filter_tool_specs(&self.tool_registry, self.allowed_tools.as_ref());
                definitions.extend(deferred_mcp_tools);
                definitions
            }),
            tool_choice: self.enable_tools.then_some(ToolChoice::Auto),
            stream: true,
            reasoning_effort: self.reasoning_effort.clone(),
            ..Default::default()
        }
    }
}

pub(crate) fn resolve_cli_auth_source() -> Result<AuthSource, Box<dyn std::error::Error>> {
    Ok(resolve_cli_auth_source_for_cwd()?)
}

pub(crate) fn resolve_cli_auth_source_for_cwd() -> Result<AuthSource, api::ApiError> {
    resolve_startup_auth_source(|| Ok(None))
}

impl ApiClient for AnthropicRuntimeClient {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        self.stream_with_cancellation(request, &CancellationToken::new())
    }

    fn stream_with_cancellation(
        &mut self,
        request: ApiRequest,
        cancel: &CancellationToken,
    ) -> Result<Vec<AssistantEvent>, RuntimeError> {
        if let Some(progress_reporter) = &self.progress_reporter {
            progress_reporter.mark_model_phase();
        }
        let after_tool_results = request_ends_with_tool_result(&request);
        let turn = tools::current_turn_start(&request.messages);
        let deferred_mcp_tools = if self.oversized_with_deferred_tools == Some(turn) {
            Vec::new()
        } else {
            self.deferred_mcp_tools(&request)
        };
        let deferred_count = deferred_mcp_tools.len();
        let mut message_request = self.message_request(&request, deferred_mcp_tools);

        let (result, dropped_deferred_tools) = self.runtime.block_on(async {
            // One retry state for the whole model call: the stream retry
            // below continues the provider client's retry count and budget
            // instead of starting a fresh one.
            let progress = RetryProgress::new();
            let mut attempt = 1;
            let mut dropped_deferred_tools = false;
            loop {
                match self
                    .consume_stream(&message_request, after_tool_results, cancel, &progress)
                    .await
                {
                    Ok(events) => return (Ok(events), dropped_deferred_tools),
                    Err(StreamFailure::Fatal(error))
                        if error.kind() == RuntimeErrorKind::ContextWindowExceeded
                            && deferred_count > 0
                            && !dropped_deferred_tools =>
                    {
                        // A small budget (such as a free per-minute token limit)
                        // can fit the conversation but not the MCP tool schemas
                        // ToolSearch added: send it once more without them before
                        // the runtime shrinks the conversation itself.
                        runtime::ui_hooks::notice(&deferred_tools_dropped_notice(deferred_count));
                        if let Some(definitions) = message_request.tools.as_mut() {
                            definitions.retain(|tool| !tools::is_deferred_runtime_tool(&tool.name));
                        }
                        dropped_deferred_tools = true;
                    }
                    Err(failure)
                        if failure.is_retryable()
                            && attempt < MAX_STREAM_ATTEMPTS
                            && progress.can_resend() =>
                    {
                        // A stalled or broken stream is retried once with the
                        // same request; typed failures decide, not messages.
                        runtime::ui_hooks::notice(&format!(
                            "\n\u{26a0} {} \u{2014} retrying once\u{2026}",
                            failure.describe()
                        ));
                        attempt += 1;
                    }
                    Err(failure) => {
                        let retries = progress.retries().max(attempt - 1);
                        return (
                            Err(failure.into_runtime_error(retries)),
                            dropped_deferred_tools,
                        );
                    }
                }
            }
        });
        if dropped_deferred_tools {
            self.oversized_with_deferred_tools = Some(turn);
        }
        result
    }
}

impl AnthropicRuntimeClient {
    /// Consumes a single streaming response. The first-token deadline covers
    /// connecting and the first event, leaving out the provider client's
    /// waits between retries (see [`FirstTokenClock`]); afterwards every gap
    /// between events is bounded by the idle timeout. Both waits race the
    /// cancellation token. `progress` carries the model call's retry state.
    #[allow(clippy::too_many_lines)]
    async fn consume_stream(
        &self,
        message_request: &MessageRequest,
        after_tool_results: bool,
        cancel: &CancellationToken,
        progress: &RetryProgress,
    ) -> Result<Vec<AssistantEvent>, StreamFailure> {
        if std::env::var("NEURON_DEBUG").is_ok() {
            runtime::ui_hooks::notice("[DEBUG] sending stream_message to API...");
        }
        let first_token_timeout = self.timeouts.first_token_for(after_tool_results);
        let clock = FirstTokenClock::start(progress, first_token_timeout);
        let first_token_failure = || StreamFailure::FirstTokenTimeout {
            waited: first_token_timeout,
            after_tool_results,
        };
        let mut stream = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(StreamFailure::Cancelled { partial_text: String::new() }),
            result = self.client.stream_message_with_progress(message_request, progress) => match result {
                Ok(stream) => stream,
                Err(error) => return Err(fatal_api_error(&self.session_id, &error)),
            },
            () = clock.expired() => return Err(first_token_failure()),
        };
        if std::env::var("NEURON_DEBUG").is_ok() {
            runtime::ui_hooks::notice("[DEBUG] stream_message connected, reading events...");
        }
        let mut sink = self.output.stream_sink();
        let mut events = Vec::new();
        let mut pending_tools = BTreeMap::<u32, (String, String, String)>::new();
        // Finished tool calls not shown yet: whether they run depends on the stop
        // reason, which comes after them.
        let mut unshown_tools = Vec::<(String, String, String)>::new();
        let mut block_has_thinking_summary = false;
        let mut saw_stop = false;
        let mut received_any_event = false;
        let mut stop_reason: Option<String> = None;
        let mut tool_calls_cut_off = false;
        let render_error =
            |error: io::Error| StreamFailure::Fatal(RuntimeError::new(error.to_string()));

        loop {
            let deadline = if received_any_event {
                self.timeouts.idle
            } else {
                clock.remaining()
            };
            let next = tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(StreamFailure::Cancelled {
                    partial_text: streamed_text(&events),
                }),
                result = tokio::time::timeout(deadline, stream.next_event()) => match result {
                    Ok(Ok(next)) => next,
                    Ok(Err(error)) if ends_unterminated_tool_call(&error, !pending_tools.is_empty()) => {
                        // Deterministic (the output limit cut the call off):
                        // hand the partial calls to the runtime instead of
                        // retrying the identical request.
                        tool_calls_cut_off = true;
                        None
                    }
                    Ok(Err(error)) => return Err(classify_stream_read_error(&self.session_id, &error)),
                    Err(_elapsed) if received_any_event => {
                        return Err(StreamFailure::Idle { waited: self.timeouts.idle })
                    }
                    Err(_elapsed) => return Err(first_token_failure()),
                },
            };

            let Some(event) = next else {
                break;
            };
            received_any_event = true;

            match event {
                ApiStreamEvent::MessageStart(start) => {
                    for (index, block) in start.message.content.into_iter().enumerate() {
                        let mut pending_tool = None;
                        push_output_block(
                            block,
                            sink.as_mut(),
                            &mut events,
                            &mut pending_tool,
                            true,
                            &mut block_has_thinking_summary,
                        )
                        .map_err(StreamFailure::Fatal)?;
                        if let Some(tool) = pending_tool {
                            pending_tools.insert(
                                u32::try_from(index).map_err(|error| {
                                    StreamFailure::Fatal(RuntimeError::new(error.to_string()))
                                })?,
                                tool,
                            );
                        }
                    }
                }
                ApiStreamEvent::ContentBlockStart(start) => {
                    // Text or reasoning after a call: show the call first, in order.
                    if !matches!(start.content_block, OutputContentBlock::ToolUse { .. }) {
                        show_tool_calls(sink.as_mut(), &mut unshown_tools, false)
                            .map_err(render_error)?;
                    }
                    let mut pending_tool = None;
                    push_output_block(
                        start.content_block,
                        sink.as_mut(),
                        &mut events,
                        &mut pending_tool,
                        true,
                        &mut block_has_thinking_summary,
                    )
                    .map_err(StreamFailure::Fatal)?;
                    if let Some(tool) = pending_tool {
                        pending_tools.insert(start.index, tool);
                    }
                }
                ApiStreamEvent::ContentBlockDelta(delta) => match delta.delta {
                    ContentBlockDelta::TextDelta { text } => {
                        if !text.is_empty() {
                            if let Some(progress_reporter) = &self.progress_reporter {
                                progress_reporter.mark_text_phase(&text);
                            }
                            sink.text_delta(&text).map_err(render_error)?;
                            events.push(AssistantEvent::TextDelta(text));
                        }
                    }
                    ContentBlockDelta::InputJsonDelta { partial_json } => {
                        if let Some((_, _, input)) = pending_tools.get_mut(&delta.index) {
                            input.push_str(&partial_json);
                        }
                    }
                    ContentBlockDelta::ThinkingDelta { thinking } => {
                        if !block_has_thinking_summary {
                            sink.reasoning_started(None, false).map_err(render_error)?;
                            block_has_thinking_summary = true;
                        }
                        if !thinking.is_empty() {
                            sink.reasoning_delta(&thinking).map_err(render_error)?;
                        }
                    }
                    ContentBlockDelta::SignatureDelta { .. } => {}
                },
                ApiStreamEvent::ContentBlockStop(stop) => {
                    block_has_thinking_summary = false;
                    sink.end_block().map_err(render_error)?;
                    if let Some((id, name, input)) = pending_tools.remove(&stop.index) {
                        if let Some(progress_reporter) = &self.progress_reporter {
                            progress_reporter.mark_tool_phase(&name, &input);
                        }
                        // Shown once the stop reason says whether it runs.
                        unshown_tools.push((id.clone(), name.clone(), input.clone()));
                        events.push(AssistantEvent::ToolUse { id, name, input });
                    }
                }
                ApiStreamEvent::MessageDelta(delta) => {
                    events.push(AssistantEvent::Usage(delta.usage.token_usage()));
                    if let Some(reason) = delta.delta.stop_reason {
                        stop_reason = Some(reason.clone());
                        events.push(AssistantEvent::StopReason(reason));
                    }
                    if let Some(note) = delta.delta.refusal {
                        events.push(AssistantEvent::Refusal(note));
                    }
                    sink.end_block().map_err(render_error)?;
                    show_tool_calls(
                        sink.as_mut(),
                        &mut unshown_tools,
                        runtime::is_truncation_stop_reason(stop_reason.as_deref()),
                    )
                    .map_err(render_error)?;
                }
                ApiStreamEvent::MessageStop(_) => {
                    saw_stop = true;
                    sink.end_block().map_err(render_error)?;
                    show_tool_calls(sink.as_mut(), &mut unshown_tools, false)
                        .map_err(render_error)?;
                    events.push(AssistantEvent::MessageStop);
                }
            }
        }
        let cut_off =
            tool_calls_cut_off || runtime::is_truncation_stop_reason(stop_reason.as_deref());
        sink.end_block().map_err(render_error)?;
        show_tool_calls(sink.as_mut(), &mut unshown_tools, cut_off).map_err(render_error)?;

        if !pending_tools.is_empty()
            && (tool_calls_cut_off || runtime::is_truncation_stop_reason(stop_reason.as_deref()))
        {
            // The output limit cut the tool call(s) off. Report them with a
            // truncation stop reason: the runtime runs none whose arguments are
            // incomplete and tells the model to split the work instead.
            sink.end_block().map_err(render_error)?;
            for (id, name, input) in std::mem::take(&mut pending_tools).into_values() {
                sink.tool_call(&id, &name, &input, true)
                    .map_err(render_error)?;
                events.push(AssistantEvent::ToolUse { id, name, input });
            }
            if !runtime::is_truncation_stop_reason(stop_reason.as_deref()) {
                events.push(AssistantEvent::StopReason(
                    OUTPUT_LIMIT_STOP_REASON.to_string(),
                ));
            }
            if !saw_stop {
                events.push(AssistantEvent::MessageStop);
            }
            push_prompt_cache_record(&self.client, &mut events);
            return Ok(events);
        }
        if !pending_tools.is_empty() {
            return Err(StreamFailure::Interrupted {
                detail: "stream ended before all tool argument blocks completed".to_string(),
            });
        }
        if !saw_stop
            && events
                .iter()
                .any(|event| matches!(event, AssistantEvent::ToolUse { .. }))
        {
            return Err(StreamFailure::Interrupted {
                detail: "stream ended before the tool message completed".to_string(),
            });
        }
        push_prompt_cache_record(&self.client, &mut events);

        if !saw_stop
            && events
                .iter()
                .any(|event| matches!(event, AssistantEvent::TextDelta(text) if !text.is_empty()))
        {
            events.push(AssistantEvent::MessageStop);
        }

        if events
            .iter()
            .any(|event| matches!(event, AssistantEvent::MessageStop))
        {
            return Ok(events);
        }

        let fallback_request = MessageRequest {
            stream: false,
            ..message_request.clone()
        };
        let response = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(StreamFailure::Cancelled { partial_text: String::new() }),
            response = self.client.send_message(&fallback_request) => response
                .map_err(|error| fatal_api_error(&self.session_id, &error))?,
        };
        let mut events =
            response_to_events(response, sink.as_mut()).map_err(StreamFailure::Fatal)?;
        push_prompt_cache_record(&self.client, &mut events);
        Ok(events)
    }
}

/// Returns `true` when the conversation ends with a tool-result message,
/// meaning the model is expected to continue after tool execution.
pub(crate) fn request_ends_with_tool_result(request: &ApiRequest) -> bool {
    request
        .messages
        .last()
        .is_some_and(|message| message.role == MessageRole::Tool)
}

pub(crate) fn format_user_visible_api_error(session_id: &str, error: &api::ApiError) -> String {
    if error.is_context_window_failure() {
        format_context_window_blocked_error(session_id, error)
    } else if error.is_generic_fatal_wrapper() {
        let mut qualifiers = vec![format!("session {session_id}")];
        if let Some(request_id) = error.request_id() {
            qualifiers.push(format!("trace {request_id}"));
        }
        let mut detail = error.to_string();
        if crate::headless::one_shot() {
            // The provider's advice is a REPL command; a `neuron prompt` run starts a
            // fresh session every time.
            detail = detail.replace(", or use /new to start a fresh session", "");
        }
        format!(
            "{} ({}): {detail}",
            error.safe_failure_class(),
            qualifiers.join(", "),
        )
    } else {
        error.to_string()
    }
}

pub(crate) fn format_context_window_blocked_error(
    session_id: &str,
    error: &api::ApiError,
) -> String {
    let mut lines = vec![
        "Context window blocked".to_string(),
        "  Failure class    context_window_blocked".to_string(),
        format!("  Session          {session_id}"),
    ];

    if let Some(request_id) = error.request_id() {
        lines.push(format!("  Trace            {request_id}"));
    }

    match error {
        api::ApiError::ContextWindowExceeded {
            model,
            estimated_input_tokens,
            requested_output_tokens,
            estimated_total_tokens,
            context_window_tokens,
        } => {
            lines.push(format!(
                "  Model            {}",
                crate::provider::shown_model(model)
            ));
            lines.push(format!(
                "  Input estimate   ~{estimated_input_tokens} tokens (heuristic)"
            ));
            lines.push(format!(
                "  Requested output {requested_output_tokens} tokens"
            ));
            lines.push(format!(
                "  Total estimate   ~{estimated_total_tokens} tokens (heuristic)"
            ));
            lines.push(format!("  Context window   {context_window_tokens} tokens"));
        }
        api::ApiError::Api { message, body, .. } => {
            let detail = message.as_deref().unwrap_or(body).trim();
            if !detail.is_empty() {
                lines.push(format!(
                    "  Detail           {}",
                    truncate_for_summary(detail, 120)
                ));
            }
        }
        api::ApiError::RetriesExhausted { last_error, .. } => {
            let detail = match last_error.as_ref() {
                api::ApiError::Api { message, body, .. } => message.as_deref().unwrap_or(body),
                other => return format_context_window_blocked_error(session_id, other),
            }
            .trim();
            if !detail.is_empty() {
                lines.push(format!(
                    "  Detail           {}",
                    truncate_for_summary(detail, 120)
                ));
            }
        }
        api::ApiError::Gateway(gateway) => {
            // Written for the user (for example the free per-minute token
            // budget the prompt went over), so shown in full, with the
            // upgrade link when a paid pool would take the request.
            let detail = gateway.message.trim();
            if !detail.is_empty() {
                lines.push(format!("  Detail           {detail}"));
            }
            if let Some(upgrade_url) = &gateway.upgrade_url {
                lines.push(format!("  Upgrade          {upgrade_url}"));
            }
        }
        _ => {}
    }

    lines.push(String::new());
    lines.extend(context_window_recovery_lines(
        session_id,
        crate::headless::one_shot(),
    ));

    lines.join("\n")
}

/// What to do about a blocked context window: REPL commands, or for a one-shot
/// `neuron prompt` run (`one_shot`, no REPL afterwards) the commands to run instead.
pub(crate) fn context_window_recovery_lines(session_id: &str, one_shot: bool) -> Vec<String> {
    let mut lines = vec!["Recovery".to_string()];
    if one_shot {
        lines.push(format!(
            "  Compact          neuron --resume {session_id} /compact"
        ));
        lines.push(
            "  Fresh session    run `neuron prompt` again: each run starts a new session"
                .to_string(),
        );
    } else {
        lines.push("  Compact          /compact".to_string());
        lines.push(format!(
            "  Resume compact   neuron --resume {session_id} /compact"
        ));
        lines.push("  Fresh session    /clear --confirm".to_string());
    }
    lines.push(
        "  Reduce scope     remove large pasted context/files or ask for a smaller slice"
            .to_string(),
    );
    lines.push("  Retry            rerun after compacting or reducing the request".to_string());
    lines
}

pub(crate) fn final_assistant_text(summary: &runtime::TurnSummary) -> String {
    summary
        .assistant_messages
        .last()
        .map(|message| {
            message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

pub(crate) fn collect_tool_uses(summary: &runtime::TurnSummary) -> Vec<serde_json::Value> {
    summary
        .assistant_messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => Some(json!({
                "id": id,
                "name": name,
                "input": input,
            })),
            _ => None,
        })
        .collect()
}

pub(crate) fn collect_tool_results(summary: &runtime::TurnSummary) -> Vec<serde_json::Value> {
    summary
        .tool_results
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                tool_name,
                output,
                is_error,
            } => Some(json!({
                "tool_use_id": tool_use_id,
                "tool_name": tool_name,
                "output": output,
                "is_error": is_error,
            })),
            _ => None,
        })
        .collect()
}

pub(crate) fn collect_prompt_cache_events(
    summary: &runtime::TurnSummary,
) -> Vec<serde_json::Value> {
    summary
        .prompt_cache_events
        .iter()
        .map(|event| {
            json!({
                "unexpected": event.unexpected,
                "reason": event.reason,
                "previous_cache_read_input_tokens": event.previous_cache_read_input_tokens,
                "current_cache_read_input_tokens": event.current_cache_read_input_tokens,
                "token_drop": event.token_drop,
            })
        })
        .collect()
}

/// Slash commands that are registered in the spec list but not yet implemented
/// in this build. Used to filter both REPL completions and help output so the
/// discovery surface only shows commands that actually work (ROADMAP #39).
pub(crate) const STUB_COMMANDS: &[&str] = &[
    "vim",
    "share",
    "feedback",
    "files",
    "fast",
    "summary",
    "desktop",
    "brief",
    "advisor",
    "stickers",
    "insights",
    "thinkback",
    "release-notes",
    "security-review",
    "keybindings",
    "privacy-settings",
    "review",
    "tasks",
    "theme",
    "voice",
    "rename",
    "copy",
    "hooks",
    "color",
    "effort",
    "branch",
    "rewind",
    "ide",
    "tag",
    "output-style",
    "add-dir",
    // Spec entries with no parse arm — produce circular "Did you mean" error
    // without this guard. Adding here routes them to the proper unsupported
    // message and excludes them from REPL completions / help.
    // NOTE: do NOT add "stats", "tokens", "cache" — they are implemented.
    "allowed-tools",
    "bookmarks",
    "workspace",
    "reasoning",
    "budget",
    "rate-limit",
    "changelog",
    "diagnostics",
    "metrics",
    "tool-details",
    "focus",
    "unfocus",
    "pin",
    "unpin",
    "language",
    "profile",
    "max-tokens",
    "temperature",
    "system-prompt",
    "notifications",
    "telemetry",
    "env",
    "project",
    "terminal-setup",
    "api-key",
    "reset",
    "undo",
    "stop",
    "retry",
    "paste",
    "screenshot",
    "image",
    "search",
    "listen",
    "speak",
    "format",
    "test",
    "lint",
    "build",
    "run",
    "git",
    "stash",
    "blame",
    "log",
    "cron",
    "team",
    "benchmark",
    "migrate",
    "templates",
    "explain",
    "refactor",
    "docs",
    "fix",
    "perf",
    "chat",
    "web",
    "map",
    "symbols",
    "references",
    "definition",
    "hover",
    "autofix",
    "multi",
    "macro",
    "alias",
    "parallel",
    "subagent",
    "agent",
];

pub(crate) fn slash_command_completion_candidates_with_sessions(
    model: &str,
    active_session_id: Option<&str>,
    recent_session_ids: Vec<String>,
) -> Vec<String> {
    let mut completions = BTreeSet::new();

    for spec in slash_command_specs() {
        if STUB_COMMANDS.contains(&spec.name) {
            continue;
        }
        completions.insert(format!("/{}", spec.name));
        for alias in spec.aliases {
            if !STUB_COMMANDS.contains(alias) {
                completions.insert(format!("/{alias}"));
            }
        }
    }

    for candidate in [
        "/bughunter ",
        "/clear --confirm",
        "/config ",
        "/config env",
        "/config hooks",
        "/config model",
        "/config plugins",
        "/mcp ",
        "/mcp list",
        "/mcp show ",
        "/export ",
        "/issue ",
        "/model ",
        "/model opus",
        "/model sonnet",
        "/model haiku",
        "/permissions ",
        "/permissions read-only",
        "/permissions workspace-write",
        "/permissions danger-full-access",
        "/plugin list",
        "/plugin install ",
        "/plugin enable ",
        "/plugin disable ",
        "/plugin uninstall ",
        "/plugin update ",
        "/plugins list",
        "/pr ",
        "/resume ",
        "/session list",
        "/session switch ",
        "/session fork ",
        "/teleport ",
        "/ultraplan ",
        "/agents help",
        "/mcp help",
        "/skills help",
    ] {
        completions.insert(candidate.to_string());
    }

    if !model.trim().is_empty() {
        completions.insert(format!("/model {}", resolve_model_alias(model)));
        completions.insert(format!("/model {model}"));
    }

    if let Some(active_session_id) = active_session_id.filter(|value| !value.trim().is_empty()) {
        completions.insert(format!("/resume {active_session_id}"));
        completions.insert(format!("/session switch {active_session_id}"));
    }

    for session_id in recent_session_ids
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .take(10)
    {
        completions.insert(format!("/resume {session_id}"));
        completions.insert(format!("/session switch {session_id}"));
    }

    completions.into_iter().collect()
}

// ── Tool UI (extracted to tool_ui.rs) ───────────────────
pub(crate) fn push_output_block(
    block: OutputContentBlock,
    sink: &mut dyn TurnSink,
    events: &mut Vec<AssistantEvent>,
    pending_tool: &mut Option<(String, String, String)>,
    streaming_tool_input: bool,
    block_has_thinking_summary: &mut bool,
) -> Result<(), RuntimeError> {
    match block {
        OutputContentBlock::Text { text } => {
            if !text.is_empty() {
                sink.text_block(&text).map_err(render_failure)?;
                events.push(AssistantEvent::TextDelta(text));
            }
        }
        OutputContentBlock::ToolUse { id, name, input } => {
            // During streaming, the initial content_block_start has an empty input ({}).
            // The real input arrives via input_json_delta events. In
            // non-streaming responses, preserve a legitimate empty object.
            let initial_input = match input {
                input
                    if streaming_tool_input
                        && input.as_object().is_some_and(serde_json::Map::is_empty) =>
                {
                    String::new()
                }
                // Arguments that did not parse as JSON arrive as their raw text
                // (a JSON string): the runtime answers the call with an error.
                serde_json::Value::String(raw) if !streaming_tool_input => raw,
                input => input.to_string(),
            };
            *pending_tool = Some((id, name, initial_input));
        }
        OutputContentBlock::Thinking { thinking, .. } => {
            sink.reasoning_started(Some(thinking.chars().count()), false)
                .map_err(render_failure)?;
            if !thinking.is_empty() {
                sink.reasoning_delta(&thinking).map_err(render_failure)?;
            }
            *block_has_thinking_summary = true;
        }
        OutputContentBlock::RedactedThinking { .. } => {
            sink.reasoning_started(None, true).map_err(render_failure)?;
            *block_has_thinking_summary = true;
        }
    }
    Ok(())
}

fn render_failure(error: io::Error) -> RuntimeError {
    RuntimeError::new(error.to_string())
}

pub(crate) fn response_to_events(
    response: MessageResponse,
    sink: &mut dyn TurnSink,
) -> Result<Vec<AssistantEvent>, RuntimeError> {
    let mut events = Vec::new();
    let mut pending_tool = None;

    for block in response.content {
        let mut block_has_thinking_summary = false;
        push_output_block(
            block,
            sink,
            &mut events,
            &mut pending_tool,
            false,
            &mut block_has_thinking_summary,
        )?;
        if let Some((id, name, input)) = pending_tool.take() {
            events.push(AssistantEvent::ToolUse { id, name, input });
        }
    }

    events.push(AssistantEvent::Usage(response.usage.token_usage()));
    if let Some(stop_reason) = response.stop_reason {
        events.push(AssistantEvent::StopReason(stop_reason));
    }
    if let Some(note) = response.refusal {
        events.push(AssistantEvent::Refusal(note));
    }
    events.push(AssistantEvent::MessageStop);
    Ok(events)
}

pub(crate) fn push_prompt_cache_record(
    client: &ApiProviderClient,
    events: &mut Vec<AssistantEvent>,
) {
    // `ApiProviderClient::take_last_prompt_cache_record` is a pass-through
    // to the Anthropic variant and returns `None` for OpenAI-compat /
    // xAI variants, which do not have a prompt cache. So this helper
    // remains a no-op on non-Anthropic providers without any extra
    // branching here.
    if let Some(record) = client.take_last_prompt_cache_record() {
        if let Some(event) = prompt_cache_record_to_runtime_event(record) {
            events.push(AssistantEvent::PromptCache(event));
        }
    }
}

pub(crate) fn prompt_cache_record_to_runtime_event(
    record: api::PromptCacheRecord,
) -> Option<PromptCacheEvent> {
    let cache_break = record.cache_break?;
    Some(PromptCacheEvent {
        unexpected: cache_break.unexpected,
        reason: cache_break.reason,
        previous_cache_read_input_tokens: cache_break.previous_cache_read_input_tokens,
        current_cache_read_input_tokens: cache_break.current_cache_read_input_tokens,
        token_drop: cache_break.token_drop,
    })
}

#[cfg(test)]
mod stream_resilience_tests {
    use super::{
        classify_stream_read_error, ends_unterminated_tool_call, response_to_events,
        AnthropicRuntimeClient, StreamFailure, StreamTimeouts, DEFAULT_FIRST_TOKEN_TIMEOUT,
        DEFAULT_POST_TOOL_FIRST_TOKEN_TIMEOUT, DEFAULT_STREAM_IDLE_TIMEOUT,
    };
    use crate::{ApiProviderClient, MessageResponse};
    use api::{OpenAiCompatClient, OpenAiCompatConfig, OutputContentBlock, Usage};
    use runtime::{
        ApiClient, ApiRequest, AssistantEvent, CancellationToken, ContentBlock,
        ConversationMessage, ConversationRuntime, PermissionMode, PermissionPolicy,
        RuntimeErrorKind, Session, StaticToolExecutor,
    };
    use std::cell::Cell;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    /// How the fake gateway answers each connection.
    #[derive(Clone, Copy)]
    enum Behavior {
        /// Read the request and never answer.
        Hang,
        /// Answer with headers and one chunk, then go silent.
        StallAfterFirstChunk,
        /// Answer with a complete stream cut off by the output limit.
        TruncatedText,
        /// First answer: a tool call whose arguments the output limit cut
        /// off (`finish_reason: "length"`); later answers: plain text.
        TruncatedToolCallThenText,
        /// One step per connection, in order; the last step repeats.
        Scripted(&'static [Step]),
    }

    /// Recorded Zero-X gateway streams of the Claude adapter (api/tests/fixtures).
    mod fixture {
        pub(super) const REFUSAL_MID_TEXT: &str =
            include_str!("../../api/tests/fixtures/gateway_claude/refusal_mid_text.sse");
        pub(super) const REFUSAL_BEFORE_TEXT: &str =
            include_str!("../../api/tests/fixtures/gateway_claude/refusal_before_text.sse");
        pub(super) const REASONING_THEN_TEXT: &str =
            include_str!("../../api/tests/fixtures/gateway_claude/reasoning_then_text.sse");
        pub(super) const COMPLETE_TOOL_CALL_CUT_BY_LENGTH: &str = include_str!(
            "../../api/tests/fixtures/gateway_claude/complete_tool_call_cut_by_length.sse"
        );
        pub(super) const UNPARSEABLE_TOOL_ARGUMENTS: &str =
            include_str!("../../api/tests/fixtures/gateway_claude/unparseable_tool_arguments.sse");
        pub(super) const ERROR_AFTER_CONTENT: &str =
            include_str!("../../api/tests/fixtures/gateway_claude/error_after_content.sse");
        /// The gateway's fixed note for a Claude refusal in the cyber category.
        pub(super) const REFUSAL_NOTE: &str = "Claude declined this request (safety category: cyber). Rephrase it or choose another model.";
    }

    /// One answer of a [`Behavior::Scripted`] gateway.
    #[derive(Clone, Copy)]
    enum Step {
        /// `503 capacity_exhausted` with this `Retry-After` (seconds).
        Capacity(&'static str),
        /// Read the request and never answer.
        Hang,
        /// A complete text answer.
        Text,
        /// `400 context_length_exceeded`: over the free per-minute token budget.
        TooLarge,
        /// A recorded SSE body, sent as it is.
        Sse(&'static str),
    }

    struct FakeGateway {
        base_url: String,
        connections: Arc<AtomicUsize>,
        /// When each connection was accepted.
        accepted: Arc<Mutex<Vec<Instant>>>,
        /// The body of each request.
        bodies: Arc<Mutex<Vec<String>>>,
        _held: Arc<Mutex<Vec<TcpStream>>>,
    }

    impl FakeGateway {
        /// The tool names each request carried, in order. A connection that sent no
        /// request (one a loaded machine left idle past the read timeout) is skipped.
        fn tool_names(&self) -> Vec<Vec<String>> {
            self.bodies
                .lock()
                .expect("bodies")
                .iter()
                .filter(|body| !body.is_empty())
                .map(|body| {
                    let body: serde_json::Value =
                        serde_json::from_str(body).expect("request body is json");
                    body["tools"]
                        .as_array()
                        .map(|tools| {
                            tools
                                .iter()
                                .filter_map(|tool| tool["function"]["name"].as_str())
                                .map(ToOwned::to_owned)
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .collect()
        }

        /// Time between the starts of consecutive requests.
        fn gaps(&self) -> Vec<Duration> {
            let accepted = self.accepted.lock().expect("accepted");
            accepted
                .windows(2)
                .map(|pair| pair[1].duration_since(pair[0]))
                .collect()
        }
    }

    fn write_capacity_exhausted(stream: &mut TcpStream, retry_after: &str) {
        let body = serde_json::json!({"error": {
            "message": "The shared free pool is busy",
            "type": "server_error",
            "code": "capacity_exhausted",
            "param": null,
            "request_id": "req_capacity"
        }})
        .to_string();
        let _ = write!(
            stream,
            "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nRetry-After: {retry_after}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.flush();
    }

    fn write_text_answer(stream: &mut TcpStream, text: &str) {
        let body = [
            sse_chunk(&serde_json::json!({
                "id": "t", "model": "m",
                "choices": [{"delta": {"content": text}}]
            })),
            sse_chunk(&serde_json::json!({
                "id": "t",
                "choices": [{"delta": {}, "finish_reason": "stop"}]
            })),
            "data: [DONE]\n\n".to_string(),
        ]
        .concat();
        write_sse_body(stream, &body);
    }

    fn write_too_large(stream: &mut TcpStream) {
        let body = serde_json::json!({"error": {
            "message": "This request is too large for the free per-minute token budget (8,000 tokens, prompt plus max_tokens). Shorten the conversation or lower max_tokens.",
            "type": "invalid_request_error",
            "code": "context_length_exceeded",
            "param": null,
            "request_id": "req_too_large"
        }})
        .to_string();
        let _ = write!(
            stream,
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.flush();
    }

    /// Reads one request and returns its body.
    fn read_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 4096];
        while let Ok(read) = stream.read(&mut chunk) {
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
            let text = String::from_utf8_lossy(&buffer);
            if let Some(header_end) = text.find("\r\n\r\n") {
                let length = text[..header_end]
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if buffer.len() >= header_end + 4 + length {
                    return String::from_utf8_lossy(&buffer[header_end + 4..]).into_owned();
                }
            }
        }
        String::new()
    }

    fn sse_chunk(value: &serde_json::Value) -> String {
        format!("data: {value}\n\n")
    }

    fn write_sse_body(stream: &mut TcpStream, body: &str) {
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.flush();
    }

    fn spawn_gateway(behavior: Behavior) -> FakeGateway {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let base_url = format!("http://{}/v1", listener.local_addr().expect("addr"));
        let connections = Arc::new(AtomicUsize::new(0));
        let accepted = Arc::new(Mutex::new(Vec::new()));
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let held = Arc::new(Mutex::new(Vec::new()));
        let counter = Arc::clone(&connections);
        let accepted_at = Arc::clone(&accepted);
        let request_bodies = Arc::clone(&bodies);
        let holder = Arc::clone(&held);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                accepted_at.lock().expect("accepted").push(Instant::now());
                let index = counter.fetch_add(1, Ordering::SeqCst);
                let body = read_request(&mut stream);
                request_bodies.lock().expect("bodies").push(body);
                match behavior {
                    Behavior::Hang => {}
                    Behavior::Scripted(steps) => match steps.get(index).or(steps.last()).copied() {
                        Some(Step::Capacity(retry_after)) => {
                            write_capacity_exhausted(&mut stream, retry_after);
                        }
                        Some(Step::Text) => write_text_answer(&mut stream, "after the wait"),
                        Some(Step::TooLarge) => write_too_large(&mut stream),
                        Some(Step::Sse(body)) => write_sse_body(&mut stream, body),
                        Some(Step::Hang) | None => {}
                    },
                    Behavior::StallAfterFirstChunk => {
                        let first = sse_chunk(&serde_json::json!({
                            "id": "t", "model": "m",
                            "choices": [{"delta": {"content": "partial"}}]
                        }));
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{first}\r\n",
                            first.len()
                        );
                        let _ = stream.flush();
                    }
                    Behavior::TruncatedText => {
                        let body = [
                            sse_chunk(&serde_json::json!({
                                "id": "t", "model": "m",
                                "choices": [{"delta": {"content": "cut off"}}]
                            })),
                            sse_chunk(&serde_json::json!({
                                "id": "t",
                                "choices": [{"delta": {}, "finish_reason": "length"}]
                            })),
                            "data: [DONE]\n\n".to_string(),
                        ]
                        .concat();
                        write_sse_body(&mut stream, &body);
                    }
                    Behavior::TruncatedToolCallThenText if counter.load(Ordering::SeqCst) == 1 => {
                        let body = [
                            sse_chunk(&serde_json::json!({
                                "id": "t", "model": "m",
                                "choices": [{"delta": {"content": "Writing the file."}}]
                            })),
                            sse_chunk(&serde_json::json!({
                                "id": "t", "model": "m",
                                "choices": [{"delta": {"tool_calls": [{
                                    "index": 0, "id": "call_big", "type": "function",
                                    "function": {
                                        "name": "write_file",
                                        "arguments": "{\"path\":\"big.txt\",\"content\":\"line 1\\nline 2"
                                    }
                                }]}}]
                            })),
                            sse_chunk(&serde_json::json!({
                                "id": "t",
                                "choices": [{"delta": {}, "finish_reason": "length"}]
                            })),
                            "data: [DONE]\n\n".to_string(),
                        ]
                        .concat();
                        write_sse_body(&mut stream, &body);
                    }
                    Behavior::TruncatedToolCallThenText => {
                        let body = [
                            sse_chunk(&serde_json::json!({
                                "id": "t", "model": "m",
                                "choices": [{"delta": {"content": "I will use smaller edits."}}]
                            })),
                            sse_chunk(&serde_json::json!({
                                "id": "t",
                                "choices": [{"delta": {}, "finish_reason": "stop"}]
                            })),
                            "data: [DONE]\n\n".to_string(),
                        ]
                        .concat();
                        write_sse_body(&mut stream, &body);
                    }
                }
                holder.lock().expect("hold").push(stream);
            }
        });
        FakeGateway {
            base_url,
            connections,
            accepted,
            bodies,
            _held: held,
        }
    }

    /// A client allowing two provider retries (the Zero-X limit); the stream
    /// retry counts as one of them.
    fn client_for(gateway: &FakeGateway, timeouts: StreamTimeouts) -> AnthropicRuntimeClient {
        let client = OpenAiCompatClient::new("test-key", OpenAiCompatConfig::openai())
            .with_base_url(gateway.base_url.clone())
            .with_retry_policy(2, Duration::from_millis(1), Duration::from_millis(1))
            .with_retry_observer(None);
        runtime_client(ApiProviderClient::OpenAi(client), timeouts)
    }

    /// A Zero-X gateway client with its default retry policy (two retries,
    /// capacity waits honoured).
    fn zero_x_client_for(
        gateway: &FakeGateway,
        timeouts: StreamTimeouts,
    ) -> AnthropicRuntimeClient {
        let client = OpenAiCompatClient::new("ses_test", OpenAiCompatConfig::zero_x())
            .with_base_url(gateway.base_url.clone())
            .with_retry_observer(None);
        runtime_client(ApiProviderClient::OpenAi(client), timeouts)
    }

    fn runtime_client(
        client: ApiProviderClient,
        timeouts: StreamTimeouts,
    ) -> AnthropicRuntimeClient {
        runtime_client_showing(client, timeouts, crate::turn_sink::TurnOutput::Silent)
    }

    /// A client whose turn output goes to `output` (the UI's events, for one).
    fn runtime_client_showing(
        client: ApiProviderClient,
        timeouts: StreamTimeouts,
        output: crate::turn_sink::TurnOutput,
    ) -> AnthropicRuntimeClient {
        AnthropicRuntimeClient {
            runtime: tokio::runtime::Runtime::new().expect("tokio"),
            client,
            session_id: "stream-test".to_string(),
            model: "openai/test-model".to_string(),
            enable_tools: false,
            output,
            allowed_tools: None,
            tool_registry: tools::GlobalToolRegistry::builtin(),
            progress_reporter: None,
            reasoning_effort: None,
            timeouts,
            oversized_with_deferred_tools: None,
        }
    }

    fn request(after_tool: bool) -> ApiRequest {
        let mut messages = vec![ConversationMessage::user_text("hi")];
        if after_tool {
            messages.push(ConversationMessage::tool_result("t1", "bash", "ok", false));
        }
        ApiRequest {
            system_prompt: Vec::new(),
            messages,
        }
    }

    fn fast_timeouts() -> StreamTimeouts {
        StreamTimeouts {
            first_token: Duration::from_millis(300),
            post_tool_first_token: Duration::from_millis(200),
            idle: Duration::from_millis(300),
        }
    }

    #[test]
    fn timeouts_default_and_env_override() {
        let defaults = StreamTimeouts::from_override(None);
        assert_eq!(defaults.first_token, DEFAULT_FIRST_TOKEN_TIMEOUT);
        assert_eq!(defaults.first_token, Duration::from_secs(90));
        assert_eq!(
            defaults.post_tool_first_token,
            DEFAULT_POST_TOOL_FIRST_TOKEN_TIMEOUT
        );
        assert_eq!(defaults.post_tool_first_token, Duration::from_secs(60));
        assert_eq!(defaults.idle, DEFAULT_STREAM_IDLE_TIMEOUT);
        assert_eq!(defaults.idle, Duration::from_secs(120));
        let overridden = StreamTimeouts::from_override(Some(" 30 "));
        assert_eq!(overridden.first_token, Duration::from_secs(30));
        assert_eq!(overridden.post_tool_first_token, Duration::from_secs(30));
        assert_eq!(StreamTimeouts::from_override(Some("0")), defaults);
        assert_eq!(StreamTimeouts::from_override(Some("soon")), defaults);
        assert_eq!(defaults.first_token_for(true), Duration::from_secs(60));
    }

    #[test]
    fn stalled_first_token_is_retried_once_then_reported_as_typed_timeout() {
        let gateway = spawn_gateway(Behavior::Hang);
        let mut client = client_for(&gateway, fast_timeouts());

        let error = client
            .stream(request(false))
            .expect_err("a silent gateway must time out");

        assert_eq!(error.kind(), RuntimeErrorKind::Timeout);
        assert!(error.to_string().contains("after one retry"), "{error}");
        assert_eq!(gateway.connections.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn no_stream_retry_when_the_client_allows_no_retries() {
        let gateway = spawn_gateway(Behavior::Hang);
        let client = OpenAiCompatClient::new("test-key", OpenAiCompatConfig::openai())
            .with_base_url(gateway.base_url.clone())
            .with_retry_policy(0, Duration::from_millis(1), Duration::from_millis(1))
            .with_retry_observer(None);
        let mut client = runtime_client(ApiProviderClient::OpenAi(client), fast_timeouts());

        let error = client
            .stream(request(false))
            .expect_err("a silent gateway must time out");

        assert_eq!(error.kind(), RuntimeErrorKind::Timeout);
        assert!(!error.to_string().contains("retry"), "{error}");
        // On a loaded machine the client can give up before the gateway's thread has
        // counted the connection; wait for it, then check there was no second one.
        let deadline = Instant::now() + Duration::from_secs(2);
        while gateway.connections.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        thread::sleep(Duration::from_millis(100));
        assert_eq!(gateway.connections.load(Ordering::SeqCst), 1);
    }

    /// The failed live run: after the tool results the gateway asked for
    /// longer waits than the first-token deadline (36 s and 58 s against
    /// 60 s, scaled down here). Each wait must run in full, and the deadline
    /// must not send extra requests on top of the gateway's two retries.
    #[test]
    fn capacity_waits_do_not_count_against_the_first_token_deadline() {
        static STEPS: &[Step] = &[Step::Capacity("0.6"), Step::Capacity("0.9"), Step::Text];
        let gateway = spawn_gateway(Behavior::Scripted(STEPS));
        let mut client = zero_x_client_for(&gateway, fast_timeouts());

        let events = client
            .stream(request(true))
            .expect("the third request answers");

        assert!(events.contains(&AssistantEvent::TextDelta("after the wait".to_string())));
        assert_eq!(gateway.connections.load(Ordering::SeqCst), 3);
        let gaps = gateway.gaps();
        assert!(gaps[0] >= Duration::from_millis(600), "{gaps:?}");
        assert!(gaps[1] >= Duration::from_millis(900), "{gaps:?}");
    }

    #[test]
    fn stream_retry_is_not_sent_once_the_gateway_retries_are_used() {
        static STEPS: &[Step] = &[Step::Capacity("0.4"), Step::Capacity("0.4"), Step::Hang];
        let gateway = spawn_gateway(Behavior::Scripted(STEPS));
        let mut client = zero_x_client_for(&gateway, fast_timeouts());

        let error = client
            .stream(request(false))
            .expect_err("the third request never answers");

        assert_eq!(error.kind(), RuntimeErrorKind::Timeout);
        assert!(error.to_string().contains("after 2 retries"), "{error}");
        // Give a resend, if one were sent, time to arrive.
        thread::sleep(Duration::from_millis(200));
        assert_eq!(gateway.connections.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn stream_retry_continues_the_gateway_retry_count() {
        static STEPS: &[Step] = &[
            Step::Hang,
            Step::Capacity("0.4"),
            Step::Capacity("0.4"),
            Step::Text,
        ];
        let gateway = spawn_gateway(Behavior::Scripted(STEPS));
        let mut client = zero_x_client_for(&gateway, fast_timeouts());

        let error = client
            .stream(request(false))
            .expect_err("the stream retry and one gateway retry use the limit");

        assert!(error.to_string().contains("after 3 attempts"), "{error}");
        thread::sleep(Duration::from_millis(200));
        assert_eq!(
            gateway.connections.load(Ordering::SeqCst),
            3,
            "one request and two retries in total"
        );
        assert!(gateway.gaps()[1] >= Duration::from_millis(400));
    }

    const PROBE_TOOL: &str = "mcp__probe__navigate";

    /// A client with tools on and one MCP tool, `mcp__probe__navigate`, behind ToolSearch.
    fn client_with_mcp_probe(gateway: &FakeGateway) -> AnthropicRuntimeClient {
        let mut client = zero_x_client_for(gateway, fast_timeouts());
        client.enable_tools = true;
        client.tool_registry = tools::GlobalToolRegistry::builtin()
            .with_runtime_tools(vec![tools::RuntimeToolDefinition {
                name: PROBE_TOOL.to_string(),
                description: Some("Open a page in the probe browser".to_string()),
                input_schema: serde_json::json!({ "type": "object" }),
                required_permission: PermissionMode::ReadOnly,
            }])
            .expect("runtime tools register");
        client
    }

    /// A turn whose ToolSearch call matched the probe tool.
    fn request_after_tool_search(prompt: &str) -> ApiRequest {
        let search = serde_json::json!({
            "matches": [PROBE_TOOL],
            "query": "probe",
            "normalized_query": "probe",
            "total_deferred_tools": 1
        });
        ApiRequest {
            system_prompt: Vec::new(),
            messages: vec![
                ConversationMessage::user_text(prompt),
                ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                    id: "s1".to_string(),
                    name: "ToolSearch".to_string(),
                    input: r#"{"query":"probe"}"#.to_string(),
                }]),
                ConversationMessage::tool_result("s1", "ToolSearch", search.to_string(), false),
            ],
        }
    }

    /// The failed live run: once ToolSearch matched MCP tools, their schemas made every
    /// request too large for the free per-minute token budget. The request goes out
    /// once more without them, and the rest of the turn leaves them out at once.
    #[test]
    fn a_request_too_large_with_deferred_mcp_tools_is_resent_once_without_them() {
        static STEPS: &[Step] = &[Step::TooLarge, Step::Text];
        let gateway = spawn_gateway(Behavior::Scripted(STEPS));
        let mut client = client_with_mcp_probe(&gateway);
        let request = request_after_tool_search("open the probe page");

        let events = client
            .stream(request.clone())
            .expect("the request without the MCP tools fits");

        assert!(events.contains(&AssistantEvent::TextDelta("after the wait".to_string())));
        let sent = gateway.tool_names();
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert!(sent[0].iter().any(|name| name == PROBE_TOOL), "{sent:?}");
        assert!(!sent[1].iter().any(|name| name == PROBE_TOOL), "{sent:?}");
        assert!(sent[1]
            .iter()
            .any(|name| name == "MCPTool" || name == "bash"));
        assert_eq!(
            sent[1].len() + 1,
            sent[0].len(),
            "only the MCP tool is left out"
        );

        // Later requests of the same turn leave the MCP tools out at once.
        client.stream(request).expect("the next request answers");
        let sent = gateway.tool_names();
        assert_eq!(sent.len(), 3, "no second rejected request: {sent:?}");
        assert!(!sent[2].iter().any(|name| name == PROBE_TOOL), "{sent:?}");
        assert!(super::deferred_tools_dropped_notice(10).contains("10 MCP tool definitions"));
        assert!(super::deferred_tools_dropped_notice(1).contains("1 MCP tool definition "));
    }

    #[test]
    fn a_too_large_request_without_mcp_tools_is_not_resent() {
        static STEPS: &[Step] = &[Step::TooLarge, Step::Text];
        let gateway = spawn_gateway(Behavior::Scripted(STEPS));
        let mut client = client_with_mcp_probe(&gateway);

        let error = client
            .stream(request(false))
            .expect_err("nothing to leave out, so the rejection stands");

        // The runtime shrinks the conversation and retries on this kind.
        assert_eq!(error.kind(), RuntimeErrorKind::ContextWindowExceeded);
        assert_eq!(gateway.connections.load(Ordering::SeqCst), 1);
        assert!(!gateway.tool_names()[0]
            .iter()
            .any(|name| name == PROBE_TOOL));
    }

    #[test]
    fn post_tool_requests_use_the_post_tool_deadline() {
        let gateway = spawn_gateway(Behavior::Hang);
        let mut client = client_for(
            &gateway,
            StreamTimeouts {
                first_token: Duration::from_secs(30),
                post_tool_first_token: Duration::from_millis(150),
                idle: Duration::from_secs(30),
            },
        );
        let started = Instant::now();
        let error = client
            .stream(request(true))
            .expect_err("post-tool stall times out");
        assert_eq!(error.kind(), RuntimeErrorKind::Timeout);
        assert!(
            error.to_string().contains("after the tool results"),
            "{error}"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn idle_gap_between_events_is_bounded() {
        let gateway = spawn_gateway(Behavior::StallAfterFirstChunk);
        let mut client = client_for(&gateway, fast_timeouts());

        let error = client
            .stream(request(false))
            .expect_err("a stalled stream must time out");

        assert_eq!(error.kind(), RuntimeErrorKind::Timeout);
        assert!(error.to_string().contains("stalled"), "{error}");
        assert_eq!(gateway.connections.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn cancellation_aborts_a_waiting_stream_promptly() {
        let gateway = spawn_gateway(Behavior::Hang);
        let mut client = client_for(
            &gateway,
            StreamTimeouts {
                first_token: Duration::from_secs(60),
                post_tool_first_token: Duration::from_secs(60),
                idle: Duration::from_secs(60),
            },
        );
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            trigger.cancel();
        });
        let started = Instant::now();

        let error = client
            .stream_with_cancellation(request(false), &cancel)
            .expect_err("cancelled");

        assert_eq!(error.kind(), RuntimeErrorKind::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(gateway.connections.load(Ordering::SeqCst), 1, "no retry");
        assert_eq!(error.partial_text(), None, "nothing streamed");
    }

    #[test]
    fn a_cancelled_stream_hands_back_the_text_streamed_so_far() {
        let gateway = spawn_gateway(Behavior::StallAfterFirstChunk);
        let mut client = client_for(
            &gateway,
            StreamTimeouts {
                first_token: Duration::from_secs(60),
                post_tool_first_token: Duration::from_secs(60),
                idle: Duration::from_secs(60),
            },
        );
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            trigger.cancel();
        });

        let error = client
            .stream_with_cancellation(request(false), &cancel)
            .expect_err("cancelled");

        assert_eq!(error.kind(), RuntimeErrorKind::Cancelled);
        assert_eq!(error.partial_text(), Some("partial"));
    }

    #[test]
    fn stop_reason_is_propagated_from_the_stream() {
        let gateway = spawn_gateway(Behavior::TruncatedText);
        let mut client = client_for(&gateway, fast_timeouts());

        let events = client.stream(request(false)).expect("stream completes");

        assert!(events.contains(&AssistantEvent::StopReason("length".to_string())));
        assert!(events.contains(&AssistantEvent::TextDelta("cut off".to_string())));
    }

    #[test]
    fn tool_call_cut_off_by_the_output_limit_is_reported_not_retried() {
        let gateway = spawn_gateway(Behavior::TruncatedToolCallThenText);
        let mut client = client_for(&gateway, fast_timeouts());

        let events = client
            .stream(request(false))
            .expect("a cut-off tool call is a response, not a stream failure");

        assert_eq!(
            gateway.connections.load(Ordering::SeqCst),
            1,
            "deterministic truncation must not resend the request"
        );
        let tool_inputs = events
            .iter()
            .filter_map(|event| match event {
                AssistantEvent::ToolUse { id, name, input } => {
                    Some((id.as_str(), name.as_str(), input.as_str()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            tool_inputs,
            vec![(
                "call_big",
                "write_file",
                "{\"path\":\"big.txt\",\"content\":\"line 1\\nline 2"
            )]
        );
        assert!(events.iter().any(|event| matches!(
            event,
            AssistantEvent::StopReason(reason) if runtime::is_truncation_stop_reason(Some(reason))
        )));
        assert_eq!(events.last(), Some(&AssistantEvent::MessageStop));
    }

    #[test]
    fn cut_off_tool_call_is_not_executed_and_the_model_is_asked_to_split_it() {
        let gateway = spawn_gateway(Behavior::TruncatedToolCallThenText);
        let client = client_for(&gateway, fast_timeouts());
        let executed = Rc::new(Cell::new(false));
        let executed_flag = Rc::clone(&executed);
        let tools = StaticToolExecutor::new().register("write_file", move |_| {
            executed_flag.set(true);
            Ok("written".to_string())
        });
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            client,
            tools,
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            Vec::new(),
        );

        let summary = runtime
            .run_turn("write a large file", None)
            .expect("the turn recovers from the cut-off call");

        assert!(!executed.get(), "a truncated tool call must never run");
        assert_eq!(gateway.connections.load(Ordering::SeqCst), 2);
        assert_eq!(summary.tool_results.len(), 1);
        let ContentBlock::ToolResult {
            output, is_error, ..
        } = &summary.tool_results[0].blocks[0]
        else {
            panic!("expected a tool result block");
        };
        assert!(*is_error);
        assert!(output.contains("NOT executed"), "{output}");
        assert!(output.contains("smaller"), "{output}");
        assert_eq!(
            super::final_assistant_text(&summary),
            "I will use smaller edits."
        );
    }

    /// The UI's events of `client`'s turns, and the client.
    fn showing_client(
        gateway: &FakeGateway,
    ) -> (
        AnthropicRuntimeClient,
        std::sync::mpsc::Receiver<neuron_tui::protocol::UiEvent>,
    ) {
        let (sender, receiver) = std::sync::mpsc::channel();
        let client = OpenAiCompatClient::new("ses_test", OpenAiCompatConfig::zero_x())
            .with_base_url(gateway.base_url.clone())
            .with_retry_observer(None);
        (
            runtime_client_showing(
                ApiProviderClient::OpenAi(client),
                fast_timeouts(),
                crate::turn_sink::TurnOutput::channel(sender),
            ),
            receiver,
        )
    }

    fn runtime_over<T: runtime::ToolExecutor>(
        client: AnthropicRuntimeClient,
        tools: T,
    ) -> ConversationRuntime<AnthropicRuntimeClient, T> {
        ConversationRuntime::new(
            Session::new(),
            client,
            tools,
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            Vec::new(),
        )
    }

    #[test]
    fn a_gateway_refusal_mid_text_keeps_the_text_and_ends_the_turn_with_its_note() {
        static STEPS: &[Step] = &[Step::Sse(fixture::REFUSAL_MID_TEXT)];
        let gateway = spawn_gateway(Behavior::Scripted(STEPS));
        let mut runtime = runtime_over(
            zero_x_client_for(&gateway, fast_timeouts()),
            StaticToolExecutor::new(),
        );

        let summary = runtime
            .run_turn("explain", None)
            .expect("a refusal is no error");

        assert_eq!(
            summary.outcome,
            runtime::TurnOutcome::Refused {
                note: Some(fixture::REFUSAL_NOTE.to_string()),
                skipped_tool_calls: 0,
            }
        );
        assert_eq!(summary.stop_reason.as_deref(), Some("refusal"));
        assert_eq!(
            super::final_assistant_text(&summary),
            "Here is how the lock mechanism works. First, the pins"
        );
        assert_eq!(gateway.connections.load(Ordering::SeqCst), 1);
        assert_eq!(
            crate::repl_loop::refusal_notice(&summary.outcome).as_deref(),
            Some(
                "The model declined to continue this request. Claude declined this request (safety category: cyber). Rephrase it or choose another model."
            )
        );
    }

    #[test]
    fn a_gateway_refusal_before_any_text_is_neither_an_empty_reply_nor_retried() {
        static STEPS: &[Step] = &[Step::Sse(fixture::REFUSAL_BEFORE_TEXT)];
        let gateway = spawn_gateway(Behavior::Scripted(STEPS));
        let mut runtime = runtime_over(
            zero_x_client_for(&gateway, fast_timeouts()),
            StaticToolExecutor::new(),
        );

        let summary = runtime
            .run_turn("do it", None)
            .expect("a refusal before any text is no error");

        assert!(matches!(
            summary.outcome,
            runtime::TurnOutcome::Refused { note: Some(_), .. }
        ));
        assert_eq!(
            summary.assistant_messages,
            Vec::<ConversationMessage>::new()
        );
        assert_eq!(
            gateway.connections.load(Ordering::SeqCst),
            1,
            "neither a stream retry nor the non-streaming fallback"
        );
    }

    #[test]
    fn gateway_reasoning_reaches_the_ui_apart_from_the_answer() {
        use neuron_tui::protocol::UiEvent;
        static STEPS: &[Step] = &[Step::Sse(fixture::REASONING_THEN_TEXT)];
        let gateway = spawn_gateway(Behavior::Scripted(STEPS));
        let (mut client, ui) = showing_client(&gateway);

        let events = client.stream(request(false)).expect("stream completes");
        drop(client);

        let answer = events
            .iter()
            .filter_map(|event| match event {
                AssistantEvent::TextDelta(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(answer, "17 * 23 = 391.", "reasoning is never answer text");
        let shown = ui.try_iter().collect::<Vec<_>>();
        let reasoning = shown
            .iter()
            .filter_map(|event| match event {
                UiEvent::ReasoningDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        let text = shown
            .iter()
            .filter_map(|event| match event {
                UiEvent::TextDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(reasoning, "The user asks for 17 * 23. That is 391.\n\n");
        assert_eq!(text, "17 * 23 = 391.");
        let first_reasoning = shown
            .iter()
            .position(|event| matches!(event, UiEvent::ReasoningStarted { .. }))
            .expect("a reasoning block");
        let first_text = shown
            .iter()
            .position(|event| matches!(event, UiEvent::TextDelta { .. }))
            .expect("answer text");
        assert!(first_reasoning < first_text, "{shown:?}");
    }

    #[test]
    fn no_call_of_a_response_cut_by_length_runs_and_the_ui_shows_them_cut_off() {
        use neuron_tui::protocol::UiEvent;
        static STEPS: &[Step] = &[
            Step::Sse(fixture::COMPLETE_TOOL_CALL_CUT_BY_LENGTH),
            Step::Text,
        ];
        let gateway = spawn_gateway(Behavior::Scripted(STEPS));
        let (client, ui) = showing_client(&gateway);
        let ran = Rc::new(Cell::new(0));
        let (read_ran, write_ran) = (Rc::clone(&ran), Rc::clone(&ran));
        let tools = StaticToolExecutor::new()
            .register("read_file", move |_| {
                read_ran.set(read_ran.get() + 1);
                Ok("notes".to_string())
            })
            .register("write_file", move |_| {
                write_ran.set(write_ran.get() + 1);
                Ok("written".to_string())
            });
        let mut runtime = runtime_over(client, tools);

        let summary = runtime
            .run_turn("read then write", None)
            .expect("the turn goes on after the cut-off calls");

        assert_eq!(ran.get(), 0, "no call of a cut-off response runs");
        assert_eq!(summary.tool_results.len(), 2);
        assert!(summary.tool_results.iter().all(|result| matches!(
            &result.blocks[0],
            ContentBlock::ToolResult { is_error: true, output, .. } if output.contains("NOT executed")
        )));
        assert_eq!(gateway.connections.load(Ordering::SeqCst), 2);
        drop(runtime);
        let started = ui
            .try_iter()
            .filter_map(|event| match event {
                UiEvent::ToolCallStarted { name, cut_off, .. } => Some((name, cut_off)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            started,
            [
                ("read_file".to_string(), true),
                ("write_file".to_string(), true)
            ]
        );
    }

    #[test]
    fn unparseable_tool_arguments_become_a_tool_error_not_a_stream_retry() {
        static STEPS: &[Step] = &[Step::Sse(fixture::UNPARSEABLE_TOOL_ARGUMENTS), Step::Text];
        let gateway = spawn_gateway(Behavior::Scripted(STEPS));
        let tools = StaticToolExecutor::new().register("read_file", |_| {
            panic!("garbled arguments must never reach the tool")
        });
        let mut runtime = runtime_over(zero_x_client_for(&gateway, fast_timeouts()), tools);

        let summary = runtime.run_turn("read main.rs", None).expect("no crash");

        assert_eq!(summary.outcome, runtime::TurnOutcome::Completed);
        let ContentBlock::ToolResult {
            output, is_error, ..
        } = &summary.tool_results[0].blocks[0]
        else {
            panic!("expected a tool result");
        };
        assert!(*is_error);
        assert!(output.contains("not a valid JSON object"), "{output}");
        assert_eq!(
            gateway.connections.load(Ordering::SeqCst),
            2,
            "the model heard about it; the stream itself did not fail"
        );
        assert_eq!(super::final_assistant_text(&summary), "after the wait");
    }

    #[test]
    fn the_gateways_error_event_after_content_is_retried_once_then_reported() {
        static STEPS: &[Step] = &[Step::Sse(fixture::ERROR_AFTER_CONTENT)];
        let gateway = spawn_gateway(Behavior::Scripted(STEPS));
        let mut client = zero_x_client_for(&gateway, fast_timeouts());

        let error = client
            .stream(request(false))
            .expect_err("a failed stream fails the model call");

        assert_eq!(error.kind(), RuntimeErrorKind::StreamInterrupted);
        assert!(
            error
                .to_string()
                .contains("Generation interrupted. Retry your request."),
            "{error}"
        );
        assert!(error.to_string().contains("after one retry"), "{error}");
        assert_eq!(gateway.connections.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_stalled_stream_reports_the_idle_deadline() {
        let failure = classify_stream_read_error(
            "s",
            &api::ApiError::StreamIdle {
                provider: "Zero-X".to_string(),
                model: "auto".to_string(),
                idle: Duration::from_secs(120),
            },
        );
        assert!(failure.is_retryable());
        let StreamFailure::Interrupted { detail } = &failure else {
            panic!("a stall is an interruption: {failure:?}");
        };
        assert!(detail.contains("stalled: no data for 120s"), "{detail}");
    }

    #[test]
    fn stream_errors_are_classified_by_type() {
        let interrupted = classify_stream_read_error(
            "s",
            &api::ApiError::InvalidSseFrame("Tool stream ended without a tool_calls finish reason"),
        );
        assert!(matches!(interrupted, StreamFailure::Interrupted { .. }));
        assert!(interrupted.is_retryable());
        // With tool-call blocks still open the same error is an output-limit
        // cut-off, which the stream reports as a truncated response instead.
        let cut_off =
            api::ApiError::InvalidSseFrame("Tool stream ended without a tool_calls finish reason");
        assert!(ends_unterminated_tool_call(&cut_off, true));
        assert!(!ends_unterminated_tool_call(&cut_off, false));
        assert!(!ends_unterminated_tool_call(
            &api::ApiError::Io(std::io::Error::other("connection reset")),
            true
        ));

        let io = classify_stream_read_error(
            "s",
            &api::ApiError::Io(std::io::Error::other("connection reset")),
        );
        assert!(io.is_retryable());

        let quota = classify_stream_read_error(
            "s",
            &api::ApiError::Api {
                status: reqwest::StatusCode::TOO_MANY_REQUESTS,
                error_type: Some("insufficient_quota".to_string()),
                message: Some("daily cap reached".to_string()),
                request_id: None,
                body: String::new(),
                retryable: false,
                suggested_action: None,
            },
        );
        assert!(
            matches!(&quota, StreamFailure::Fatal(error) if error.kind() == RuntimeErrorKind::Other)
        );
        assert!(!quota.is_retryable());

        // Context overflow is fatal for the stream but typed, so the runtime
        // can shrink the context and retry once.
        let overflow = classify_stream_read_error(
            "s",
            &api::ApiError::ContextWindowExceeded {
                model: "m".to_string(),
                estimated_input_tokens: 140_000,
                requested_output_tokens: 8_192,
                estimated_total_tokens: 148_192,
                context_window_tokens: 131_072,
            },
        );
        assert!(matches!(
            &overflow,
            StreamFailure::Fatal(error) if error.kind() == RuntimeErrorKind::ContextWindowExceeded
        ));
        let rejected_by_provider = classify_stream_read_error(
            "s",
            &api::ApiError::Api {
                status: reqwest::StatusCode::BAD_REQUEST,
                error_type: Some("context_length_exceeded".to_string()),
                message: Some("This model's maximum context length is 131072 tokens".to_string()),
                request_id: None,
                body: String::new(),
                retryable: false,
                suggested_action: None,
            },
        );
        assert!(matches!(
            rejected_by_provider.into_runtime_error(0).kind(),
            RuntimeErrorKind::ContextWindowExceeded
        ));
        assert_eq!(
            StreamFailure::Interrupted {
                detail: "x".to_string()
            }
            .into_runtime_error(1)
            .kind(),
            RuntimeErrorKind::StreamInterrupted
        );
    }

    #[test]
    fn non_streaming_fallback_reports_stop_reason() {
        let response = MessageResponse {
            id: "msg".to_string(),
            kind: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![OutputContentBlock::Text {
                text: "hello".to_string(),
            }],
            model: "m".to_string(),
            stop_reason: Some("max_tokens".to_string()),
            stop_sequence: None,
            usage: Usage::default(),
            request_id: None,
            refusal: None,
        };
        let events =
            response_to_events(response, &mut crate::turn_sink::SilentSink).expect("events");
        assert!(events.contains(&AssistantEvent::StopReason("max_tokens".to_string())));
        assert_eq!(events.last(), Some(&AssistantEvent::MessageStop));
    }

    #[test]
    fn non_streaming_fallback_reports_a_refusal_and_keeps_bad_arguments_raw() {
        let response = MessageResponse {
            id: "msg".to_string(),
            kind: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![OutputContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "read_file".to_string(),
                input: serde_json::Value::String("{\"path\":".to_string()),
            }],
            model: "m".to_string(),
            stop_reason: Some("refusal".to_string()),
            stop_sequence: None,
            usage: Usage::default(),
            request_id: None,
            refusal: Some("declined".to_string()),
        };
        let events =
            response_to_events(response, &mut crate::turn_sink::SilentSink).expect("events");
        assert!(events.contains(&AssistantEvent::ToolUse {
            id: "call_1".to_string(),
            name: "read_file".to_string(),
            input: "{\"path\":".to_string(),
        }));
        assert!(events.contains(&AssistantEvent::StopReason("refusal".to_string())));
        assert!(events.contains(&AssistantEvent::Refusal("declined".to_string())));
    }

    #[test]
    fn max_tokens_follow_the_shared_registry() {
        assert_eq!(
            crate::max_tokens_for_model("openai/gemini/gemini-2.5-flash"),
            32_000
        );
        assert_eq!(crate::max_tokens_for_model("groq/qwen/qwen3.8-27b"), 16_384);
        assert_eq!(crate::max_tokens_for_model("claude-opus-4-6"), 32_000);
        assert_eq!(
            crate::max_tokens_for_model("openai/@cf/meta/llama-3.3-70b-instruct-fp8-fast"),
            8_192
        );
        assert_eq!(crate::max_tokens_for_model("openai/auto"), 8_192);
        assert_eq!(crate::max_tokens_for_model("vendor/unknown"), 8_192);
    }
}

#[cfg(test)]
mod recovery_hint_tests {
    use super::context_window_recovery_lines;

    #[test]
    fn a_one_shot_run_is_told_commands_it_can_run_not_repl_ones() {
        let repl = context_window_recovery_lines("session-7", false).join("\n");
        assert!(repl.contains("  Compact          /compact\n"), "{repl}");
        assert!(repl.contains("/clear --confirm"), "{repl}");

        let one_shot = context_window_recovery_lines("session-7", true).join("\n");
        assert!(
            one_shot.contains("  Compact          neuron --resume session-7 /compact\n"),
            "{one_shot}"
        );
        assert!(!one_shot.contains("/clear"), "{one_shot}");
        assert!(!one_shot.contains("          /compact"), "{one_shot}");
    }
}

#[cfg(test)]
mod switch_model_hint_tests {
    use super::{fatal_api_error, switch_model_hint, StreamFailure};

    fn gateway_error(code: &str) -> api::ApiError {
        let body = serde_json::json!({
            "error": {
                "code": code,
                "message": "Shared free capacity is exhausted right now. Retry later or upgrade for dedicated capacity.",
                "upgrade_url": "https://zero-x.live/pricing",
            }
        })
        .to_string();
        api::ApiError::Gateway(Box::new(
            api::GatewayError::from_body(reqwest::StatusCode::SERVICE_UNAVAILABLE, &body)
                .expect("an envelope"),
        ))
    }

    /// The gateway tells everyone to upgrade, max-plan users included; switching
    /// models is what they can do now.
    #[test]
    fn exhausted_capacity_suggests_another_model() {
        let StreamFailure::Fatal(error) =
            fatal_api_error("session-1", &gateway_error("capacity_exhausted"))
        else {
            panic!("a fatal error");
        };
        let message = error.to_string();
        assert!(message.contains("capacity_exhausted"), "{message}");
        assert!(
            message.contains("Other models may have capacity now"),
            "{message}"
        );

        let StreamFailure::Fatal(error) =
            fatal_api_error("session-1", &gateway_error("insufficient_credits"))
        else {
            panic!("a fatal error");
        };
        assert!(!error.to_string().contains("/model"), "{error}");
    }

    /// The REPL and the UI have `/model`; a `neuron prompt` run has only the flag.
    #[test]
    fn the_hint_names_what_the_user_can_use() {
        let error = gateway_error("capacity_exhausted");
        let interactive = switch_model_hint(&error, false).expect("a hint");
        assert!(interactive.contains("switch with /model"), "{interactive}");
        assert!(!interactive.contains("--model"), "{interactive}");
        let one_shot = switch_model_hint(&error, true).expect("a hint");
        assert!(one_shot.contains("--model"), "{one_shot}");
        assert!(!one_shot.contains("/model"), "{one_shot}");
    }
}
