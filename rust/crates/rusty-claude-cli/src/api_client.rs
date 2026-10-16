//! Anthropic API client implementation.
//!
//! AnthropicRuntimeClient, auth resolution, and error formatting.

use super::*;
use api::{self, AuthSource};
use runtime::{CancellationToken, RuntimeErrorKind};
use std::collections::BTreeMap;
use std::env;
use std::io::{self, Write};

/// Default time to wait for the first stream event of a fresh request.
pub(crate) const DEFAULT_FIRST_TOKEN_TIMEOUT: Duration = Duration::from_secs(90);
/// Default time to wait for the first stream event after tool results.
pub(crate) const DEFAULT_POST_TOOL_FIRST_TOKEN_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest tolerated gap between two stream events once streaming started.
pub(crate) const DEFAULT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// Overrides both first-token timeouts (seconds).
pub(crate) const FIRST_TOKEN_TIMEOUT_ENV_VAR: &str = "NEURON_FIRST_TOKEN_TIMEOUT_SECS";
/// A stalled or interrupted request is retried once.
const MAX_STREAM_ATTEMPTS: usize = 2;

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
    /// The turn was cancelled while waiting.
    Cancelled,
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
            Self::Cancelled => "Request cancelled".to_string(),
            Self::Fatal(error) => error.to_string(),
        }
    }

    pub(crate) fn into_runtime_error(self) -> RuntimeError {
        let kind = match &self {
            Self::FirstTokenTimeout { .. } | Self::Idle { .. } => RuntimeErrorKind::Timeout,
            Self::Interrupted { .. } => RuntimeErrorKind::StreamInterrupted,
            Self::Cancelled => RuntimeErrorKind::Cancelled,
            Self::Fatal(_) => RuntimeErrorKind::Other,
        };
        match self {
            Self::Fatal(error) => error,
            Self::Cancelled => RuntimeError::with_kind(kind, "request cancelled by user"),
            other => RuntimeError::with_kind(
                kind,
                format!(
                    "{} (after one retry). The provider may be overloaded; try again, or switch models with /model.",
                    other.describe()
                ),
            ),
        }
    }
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
    StreamFailure::Fatal(RuntimeError::with_kind(
        kind,
        format_user_visible_api_error(session_id, error),
    ))
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

/// Terminal line for a tool call that was cut off at the output limit.
fn format_cut_off_tool_call(name: &str, input: &str) -> String {
    if serde_json::from_str::<serde_json::Value>(input).is_ok() {
        format_tool_call_start(name, input)
    } else {
        format!(
            "\x1b[33m\u{26a0}\x1b[0m `{name}` call was cut off at the output token limit \u{2014} not run; the model will be asked to split it into smaller steps"
        )
    }
}

pub(crate) struct AnthropicRuntimeClient {
    runtime: tokio::runtime::Runtime,
    client: ApiProviderClient,
    session_id: String,
    model: String,
    enable_tools: bool,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    tool_registry: GlobalToolRegistry,
    progress_reporter: Option<InternalPromptProgressReporter>,
    reasoning_effort: Option<String>,
    timeouts: StreamTimeouts,
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
        emit_output: bool,
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
            emit_output,
            allowed_tools,
            tool_registry,
            progress_reporter,
            reasoning_effort: None,
            timeouts: StreamTimeouts::from_env(),
        })
    }

    pub fn set_reasoning_effort(&mut self, effort: Option<String>) {
        self.reasoning_effort = effort;
    }

    pub fn reasoning_effort(&self) -> Option<&str> {
        self.reasoning_effort.as_deref()
    }

    pub fn emit_output(&self) -> bool {
        self.emit_output
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

    fn message_request(&self, request: &ApiRequest) -> MessageRequest {
        MessageRequest {
            model: self.model.clone(),
            max_tokens: max_tokens_for_model(&self.model),
            messages: convert_messages(&request.messages),
            system: (!request.system_prompt.is_empty()).then(|| request.system_prompt.join("\n\n")),
            tools: self
                .enable_tools
                .then(|| filter_tool_specs(&self.tool_registry, self.allowed_tools.as_ref())),
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
        let message_request = self.message_request(&request);

        self.runtime.block_on(async {
            let mut attempt = 1;
            loop {
                match self
                    .consume_stream(&message_request, after_tool_results, cancel)
                    .await
                {
                    Ok(events) => return Ok(events),
                    Err(failure) if failure.is_retryable() && attempt < MAX_STREAM_ATTEMPTS => {
                        // A stalled or broken stream is retried once with the
                        // same request; typed failures decide, not messages.
                        eprintln!(
                            "\n\u{26a0} {} \u{2014} retrying once\u{2026}",
                            failure.describe()
                        );
                        attempt += 1;
                    }
                    Err(failure) => return Err(failure.into_runtime_error()),
                }
            }
        })
    }
}

impl AnthropicRuntimeClient {
    /// Consumes a single streaming response. The first-token deadline covers
    /// connecting and the first event; afterwards every gap between events is
    /// bounded by the idle timeout. Both waits race the cancellation token.
    #[allow(clippy::too_many_lines)]
    async fn consume_stream(
        &self,
        message_request: &MessageRequest,
        after_tool_results: bool,
        cancel: &CancellationToken,
    ) -> Result<Vec<AssistantEvent>, StreamFailure> {
        if std::env::var("NEURON_DEBUG").is_ok() {
            eprintln!("[DEBUG] sending stream_message to API...");
        }
        let started = Instant::now();
        let first_token_timeout = self.timeouts.first_token_for(after_tool_results);
        let mut stream = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(StreamFailure::Cancelled),
            result = tokio::time::timeout(first_token_timeout, self.client.stream_message(message_request)) => match result {
                Ok(Ok(stream)) => stream,
                Ok(Err(error)) => return Err(fatal_api_error(&self.session_id, &error)),
                Err(_elapsed) => {
                    return Err(StreamFailure::FirstTokenTimeout {
                        waited: first_token_timeout,
                        after_tool_results,
                    })
                }
            },
        };
        if std::env::var("NEURON_DEBUG").is_ok() {
            eprintln!("[DEBUG] stream_message connected, reading events...");
        }
        let mut render_buf = crate::stream_buffer::SyncRenderBuffer::new();
        let mut sink = io::sink();
        let out: &mut dyn Write = if self.emit_output {
            &mut render_buf
        } else {
            &mut sink
        };
        let renderer = TerminalRenderer::new();
        let mut markdown_stream = MarkdownStreamState::default();
        let mut events = Vec::new();
        let mut pending_tools = BTreeMap::<u32, (String, String, String)>::new();
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
                first_token_timeout.saturating_sub(started.elapsed())
            };
            let next = tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(StreamFailure::Cancelled),
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
                    Err(_elapsed) => {
                        return Err(StreamFailure::FirstTokenTimeout {
                            waited: first_token_timeout,
                            after_tool_results,
                        })
                    }
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
                            out,
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
                    let mut pending_tool = None;
                    push_output_block(
                        start.content_block,
                        out,
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
                            if let Some(rendered) = markdown_stream.push(&renderer, &text) {
                                write!(out, "{rendered}")
                                    .and_then(|()| out.flush())
                                    .map_err(render_error)?;
                            }
                            events.push(AssistantEvent::TextDelta(text));
                        }
                    }
                    ContentBlockDelta::InputJsonDelta { partial_json } => {
                        if let Some((_, _, input)) = pending_tools.get_mut(&delta.index) {
                            input.push_str(&partial_json);
                        }
                    }
                    ContentBlockDelta::ThinkingDelta { .. } => {
                        if !block_has_thinking_summary {
                            render_thinking_block_summary(out, None, false)
                                .map_err(StreamFailure::Fatal)?;
                            block_has_thinking_summary = true;
                        }
                    }
                    ContentBlockDelta::SignatureDelta { .. } => {}
                },
                ApiStreamEvent::ContentBlockStop(stop) => {
                    block_has_thinking_summary = false;
                    if let Some(rendered) = markdown_stream.flush(&renderer) {
                        write!(out, "{rendered}")
                            .and_then(|()| out.flush())
                            .map_err(render_error)?;
                    }
                    if let Some((id, name, input)) = pending_tools.remove(&stop.index) {
                        if let Some(progress_reporter) = &self.progress_reporter {
                            progress_reporter.mark_tool_phase(&name, &input);
                        }
                        // Display tool call now that input is fully accumulated
                        writeln!(out, "\n{}", format_tool_call_start(&name, &input))
                            .and_then(|()| out.flush())
                            .map_err(render_error)?;
                        events.push(AssistantEvent::ToolUse { id, name, input });
                    }
                }
                ApiStreamEvent::MessageDelta(delta) => {
                    events.push(AssistantEvent::Usage(delta.usage.token_usage()));
                    if let Some(reason) = delta.delta.stop_reason {
                        stop_reason = Some(reason.clone());
                        events.push(AssistantEvent::StopReason(reason));
                    }
                }
                ApiStreamEvent::MessageStop(_) => {
                    saw_stop = true;
                    if let Some(rendered) = markdown_stream.flush(&renderer) {
                        write!(out, "{rendered}")
                            .and_then(|()| out.flush())
                            .map_err(render_error)?;
                    }
                    events.push(AssistantEvent::MessageStop);
                }
            }
        }

        if !pending_tools.is_empty()
            && (tool_calls_cut_off || runtime::is_truncation_stop_reason(stop_reason.as_deref()))
        {
            // The output limit cut the tool call(s) off. Report them with a
            // truncation stop reason: the runtime runs none whose arguments are
            // incomplete and tells the model to split the work instead.
            if let Some(rendered) = markdown_stream.flush(&renderer) {
                write!(out, "{rendered}")
                    .and_then(|()| out.flush())
                    .map_err(render_error)?;
            }
            for (id, name, input) in std::mem::take(&mut pending_tools).into_values() {
                writeln!(out, "\n{}", format_cut_off_tool_call(&name, &input))
                    .and_then(|()| out.flush())
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
            () = cancel.cancelled() => return Err(StreamFailure::Cancelled),
            response = self.client.send_message(&fallback_request) => response
                .map_err(|error| fatal_api_error(&self.session_id, &error))?,
        };
        let mut events = response_to_events(response, out).map_err(StreamFailure::Fatal)?;
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
        format!(
            "{} ({}): {}",
            error.safe_failure_class(),
            qualifiers.join(", "),
            error
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
            lines.push(format!("  Model            {model}"));
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
        _ => {}
    }

    lines.push(String::new());
    lines.push("Recovery".to_string());
    lines.push("  Compact          /compact".to_string());
    lines.push(format!(
        "  Resume compact   neuron --resume {session_id} /compact"
    ));
    lines.push("  Fresh session    /clear --confirm".to_string());
    lines.push(
        "  Reduce scope     remove large pasted context/files or ask for a smaller slice"
            .to_string(),
    );
    lines.push("  Retry            rerun after compacting or reducing the request".to_string());

    lines.join("\n")
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
pub(crate) fn render_thinking_block_summary(
    out: &mut (impl Write + ?Sized),
    char_count: Option<usize>,
    redacted: bool,
) -> Result<(), RuntimeError> {
    let summary = if redacted {
        "\n▶ Thinking block hidden by provider\n".to_string()
    } else if let Some(char_count) = char_count {
        format!("\n▶ Thinking ({char_count} chars hidden)\n")
    } else {
        "\n▶ Thinking hidden\n".to_string()
    };
    write!(out, "{summary}")
        .and_then(|()| out.flush())
        .map_err(|error| RuntimeError::new(error.to_string()))
}

pub(crate) fn push_output_block(
    block: OutputContentBlock,
    out: &mut (impl Write + ?Sized),
    events: &mut Vec<AssistantEvent>,
    pending_tool: &mut Option<(String, String, String)>,
    streaming_tool_input: bool,
    block_has_thinking_summary: &mut bool,
) -> Result<(), RuntimeError> {
    match block {
        OutputContentBlock::Text { text } => {
            if !text.is_empty() {
                let rendered = TerminalRenderer::new().markdown_to_ansi(&text);
                write!(out, "{rendered}")
                    .and_then(|()| out.flush())
                    .map_err(|error| RuntimeError::new(error.to_string()))?;
                events.push(AssistantEvent::TextDelta(text));
            }
        }
        OutputContentBlock::ToolUse { id, name, input } => {
            // During streaming, the initial content_block_start has an empty input ({}).
            // The real input arrives via input_json_delta events. In
            // non-streaming responses, preserve a legitimate empty object.
            let initial_input = if streaming_tool_input
                && input.is_object()
                && input.as_object().is_some_and(serde_json::Map::is_empty)
            {
                String::new()
            } else {
                input.to_string()
            };
            *pending_tool = Some((id, name, initial_input));
        }
        OutputContentBlock::Thinking { thinking, .. } => {
            render_thinking_block_summary(out, Some(thinking.chars().count()), false)?;
            *block_has_thinking_summary = true;
        }
        OutputContentBlock::RedactedThinking { .. } => {
            render_thinking_block_summary(out, None, true)?;
            *block_has_thinking_summary = true;
        }
    }
    Ok(())
}

pub(crate) fn response_to_events(
    response: MessageResponse,
    out: &mut (impl Write + ?Sized),
) -> Result<Vec<AssistantEvent>, RuntimeError> {
    let mut events = Vec::new();
    let mut pending_tool = None;

    for block in response.content {
        let mut block_has_thinking_summary = false;
        push_output_block(
            block,
            out,
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
    }

    struct FakeGateway {
        base_url: String,
        connections: Arc<AtomicUsize>,
        _held: Arc<Mutex<Vec<TcpStream>>>,
    }

    fn read_request(stream: &mut TcpStream) {
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
                    break;
                }
            }
        }
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
        let held = Arc::new(Mutex::new(Vec::new()));
        let counter = Arc::clone(&connections);
        let holder = Arc::clone(&held);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                counter.fetch_add(1, Ordering::SeqCst);
                read_request(&mut stream);
                match behavior {
                    Behavior::Hang => {}
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
            _held: held,
        }
    }

    fn client_for(gateway: &FakeGateway, timeouts: StreamTimeouts) -> AnthropicRuntimeClient {
        let client = OpenAiCompatClient::new("test-key", OpenAiCompatConfig::openai())
            .with_base_url(gateway.base_url.clone())
            .with_retry_policy(0, Duration::from_millis(1), Duration::from_millis(1));
        AnthropicRuntimeClient {
            runtime: tokio::runtime::Runtime::new().expect("tokio"),
            client: ApiProviderClient::OpenAi(client),
            session_id: "stream-test".to_string(),
            model: "openai/test-model".to_string(),
            enable_tools: false,
            emit_output: false,
            allowed_tools: None,
            tool_registry: tools::GlobalToolRegistry::builtin(),
            progress_reporter: None,
            reasoning_effort: None,
            timeouts,
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
            rejected_by_provider.into_runtime_error().kind(),
            RuntimeErrorKind::ContextWindowExceeded
        ));
        assert_eq!(
            StreamFailure::Interrupted {
                detail: "x".to_string()
            }
            .into_runtime_error()
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
        };
        let events = response_to_events(response, &mut std::io::sink()).expect("events");
        assert!(events.contains(&AssistantEvent::StopReason("max_tokens".to_string())));
        assert_eq!(events.last(), Some(&AssistantEvent::MessageStop));
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
