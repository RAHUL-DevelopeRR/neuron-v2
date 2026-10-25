use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

use serde_json::{Map, Value};
use telemetry::SessionTracer;

use crate::cancel::CancellationToken;
use crate::compact::{
    compact_session_with_policy, estimate_session_tokens, should_compact, CompactionConfig,
    CompactionResult, TokenCompaction, AUTO_CONTINUE_PREFIX, DEFAULT_MAX_SUMMARY_TOKENS,
};
use crate::config::RuntimeFeatureConfig;
use crate::context_manager::{
    ContextBudget, ContextManager, ContextPolicy, ContextPreparation, ContextSnapshot,
};
use crate::hooks::{HookAbortSignal, HookProgressReporter, HookRunResult, HookRunner};
use crate::permissions::{
    PermissionContext, PermissionOutcome, PermissionPolicy, PermissionPrompter,
};
use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};
use crate::transcript::{normalize_messages, normalized_messages};
use crate::usage::{TokenUsage, UsageTracker};

const DEFAULT_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD: u32 = 100_000;
const AUTO_COMPACTION_THRESHOLD_ENV_VAR: &str = "CLAUDE_CODE_AUTO_COMPACT_INPUT_TOKENS";
/// Default cap on model calls per turn.
pub const DEFAULT_MAX_ITERATIONS: usize = 50;
/// Environment variable overriding the per-turn model-call cap.
pub const MAX_ITERATIONS_ENV_VAR: &str = "NEURON_MAX_ITERATIONS";
/// Output recorded for tool calls that never ran because the user interrupted.
pub const INTERRUPTED_TOOL_RESULT: &str = "Interrupted by user";
/// Ends the assistant's reply of an interrupted turn in the transcript: after the
/// partial answer, or alone when nothing was streamed yet. The next request then sees
/// that the old one was cut short instead of an unanswered user message (and two
/// user messages never follow each other). Models copied the note as their next
/// answer, so model requests never show it (see [`request_messages`]).
pub const INTERRUPTED_RESPONSE_NOTE: &str = "[response interrupted]";
/// What a request shows instead of a lone [`INTERRUPTED_RESPONSE_NOTE`] that closes a
/// turn which ran tools (that turn stays in the request): an out-of-band note, not
/// something that reads like an answer.
const INTERRUPTED_TURN_NOTE_FOR_MODEL: &str =
    "(Note from Neuron, not a reply: the user interrupted this turn before an answer was written.)";
/// The assistant's turn in a request, after a prompt the user stopped before the model
/// wrote anything: it says what happened in the past tense, so the model neither takes
/// the old prompt as still pending nor copies it as an answer.
const STOPPED_PROMPT_REPLY_FOR_MODEL: &str = "(no answer: the user stopped this request)";
/// Starts the assistant's turn in a request after a prompt that failed before the
/// model wrote anything; a short form of the error follows.
const FAILED_PROMPT_REPLY_PREFIX_FOR_MODEL: &str = "(no answer: this request failed: ";
/// Longest error a request shows after [`FAILED_PROMPT_REPLY_PREFIX_FOR_MODEL`].
const FAILED_PROMPT_REPLY_ERROR_MAX_CHARS: usize = 160;
/// Ends a partial answer the user stopped, in a request, in place of the
/// transcript's [`INTERRUPTED_RESPONSE_NOTE`].
const STOPPED_ANSWER_NOTE_FOR_MODEL: &str = "(answer cut off: the user stopped this request)";
/// Starts the assistant note that closes a failed turn in the transcript, for the
/// same reason as [`INTERRUPTED_RESPONSE_NOTE`]; requests never show it either.
pub const FAILED_RESPONSE_NOTE_PREFIX: &str = "[response failed: ";
/// Longest error excerpt kept in a failed turn's note.
const FAILED_RESPONSE_NOTE_MAX_CHARS: usize = 300;
/// Identical consecutive tool calls tolerated before the loop guard warns.
const LOOP_GUARD_THRESHOLD: usize = 3;
const MIN_MANUAL_SUMMARY_TOKENS: usize = 800;
const TRUNCATED_STOP_REASONS: &[&str] = &["max_tokens", "length", "model_context_window_exceeded"];
/// Stop reason of a response the model declined to continue: Anthropic's own
/// `refusal`, and what the OpenAI-compatible `content_filter` finish reason is
/// normalized to.
pub const REFUSAL_STOP_REASON: &str = "refusal";
const REFUSAL_STOP_REASONS: &[&str] = &[REFUSAL_STOP_REASON, "content_filter"];
const TRUNCATED_TOOL_CALL_MESSAGE: &str = "This tool call was NOT executed: your response hit the output token limit (max_tokens) while writing it, so its arguments are incomplete JSON. Split the work into smaller steps — for example write or edit the file in several smaller edit_file calls instead of one large write — and try again.";
/// The result of a call whose arguments did arrive complete, in a response the
/// output limit cut off: no call of a cut-off response runs.
const TRUNCATED_COMPLETE_TOOL_CALL_MESSAGE: &str = "This tool call was NOT executed: your response hit the output token limit (max_tokens), and no tool call of a response that was cut off is run. If it is still needed, make the call again in a shorter response.";
/// The result of every call in a response the model declined to continue.
const REFUSED_TOOL_CALL_MESSAGE: &str = "Not executed: the model declined to continue this response (the provider ended it with a refusal), so none of its tool calls were run.";
/// Starts the result of a call whose arguments are not a JSON object (cut off or
/// garbled while streaming); what is wrong with them follows.
const INVALID_TOOL_ARGUMENTS_PREFIX: &str =
    "This tool call was NOT executed: its arguments are not a valid JSON object";
/// What the note that closes a refused turn says, in place of an error (see
/// [`FAILED_RESPONSE_NOTE_PREFIX`]).
const REFUSAL_TURN_NOTE: &str = "the model declined to continue this request";
const AUTO_CONTINUE_MESSAGE: &str = "Your previous response was cut off because it reached the maximum output length. Continue exactly where you stopped, without repeating what you already wrote.";
const LOOP_STOPPED_REMAINING_MESSAGE: &str =
    "Not executed: the turn was stopped by the repeated-tool-call guard.";

/// Fully assembled request payload sent to the upstream model client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiRequest {
    pub system_prompt: Vec<String>,
    pub messages: Vec<ConversationMessage>,
}

/// Streamed events emitted while processing a single assistant turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssistantEvent {
    TextDelta(String),
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    Usage(TokenUsage),
    PromptCache(PromptCacheEvent),
    /// Provider stop reason (`end_turn`, `tool_use`, `max_tokens`, `length`,
    /// [`REFUSAL_STOP_REASON`], ...).
    StopReason(String),
    /// Why the model declined, as the provider explained it, with the
    /// [`REFUSAL_STOP_REASON`] stop reason.
    Refusal(String),
    MessageStop,
}

/// Prompt-cache telemetry captured from the provider response stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCacheEvent {
    pub unexpected: bool,
    pub reason: String,
    pub previous_cache_read_input_tokens: u32,
    pub current_cache_read_input_tokens: u32,
    pub token_drop: u32,
}

/// Minimal streaming API contract required by [`ConversationRuntime`].
pub trait ApiClient {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError>;

    /// Streams a response while observing `cancel`. Clients that can abort an
    /// in-flight request should override this; the default ignores the token
    /// and the runtime discards or annotates the result if the turn was
    /// cancelled meanwhile.
    fn stream_with_cancellation(
        &mut self,
        request: ApiRequest,
        _cancel: &CancellationToken,
    ) -> Result<Vec<AssistantEvent>, RuntimeError> {
        self.stream(request)
    }
}

/// Trait implemented by tool dispatchers that execute model-requested tools.
pub trait ToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError>;

    /// Executes a tool while observing `cancel` (for example to kill a child
    /// process). The default ignores the token; the runtime still checks it
    /// between tool calls.
    fn execute_with_cancellation(
        &mut self,
        tool_name: &str,
        input: &str,
        _cancel: &CancellationToken,
    ) -> Result<String, ToolError> {
        self.execute(tool_name, input)
    }

    /// Every tool call's final result, as the model will see it: one that ran, one that
    /// was denied or blocked by a hook, and one that never started (interrupted, cut off
    /// at the output limit, stopped by the loop guard). The default does nothing.
    fn tool_call_finished(
        &mut self,
        _tool_use_id: &str,
        _tool_name: &str,
        _output: &str,
        _is_error: bool,
    ) {
    }
}

/// Error returned when a tool invocation fails locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    message: String,
}

impl ToolError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for ToolError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ToolError {}

/// Machine-readable classification of a [`RuntimeError`], so callers never
/// need to match on message text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RuntimeErrorKind {
    #[default]
    Other,
    /// The turn was cancelled (Ctrl-C).
    Cancelled,
    /// The model did not produce a first token (or went idle) in time.
    Timeout,
    /// The response stream broke off before it completed.
    StreamInterrupted,
    /// The per-turn model-call cap was reached.
    MaxIterations,
    /// Reading or writing the session file failed.
    Session,
    /// The provider rejected the request because the input does not fit the
    /// model's context window. The runtime shrinks the context and retries
    /// once before reporting it.
    ContextWindowExceeded,
}

/// Error returned when a conversation turn cannot be completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    message: String,
    kind: RuntimeErrorKind,
    partial_text: Option<String>,
}

impl RuntimeError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self::with_kind(RuntimeErrorKind::Other, message)
    }

    #[must_use]
    pub fn with_kind(kind: RuntimeErrorKind, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind,
            partial_text: None,
        }
    }

    /// The text the model had streamed before the request stopped (a cancelled
    /// request): the runtime keeps it in the transcript.
    #[must_use]
    pub fn with_partial_text(mut self, text: impl Into<String>) -> Self {
        self.partial_text = Some(text.into());
        self
    }

    #[must_use]
    pub fn partial_text(&self) -> Option<&str> {
        self.partial_text.as_deref()
    }

    #[must_use]
    pub fn kind(&self) -> RuntimeErrorKind {
        self.kind
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.kind == RuntimeErrorKind::Cancelled
    }
}

impl Display for RuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RuntimeError {}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TurnOutcome {
    /// The model finished normally.
    #[default]
    Completed,
    /// The user interrupted the turn; `interrupted_tool_calls` calls received a
    /// synthetic "Interrupted by user" result instead of running.
    Cancelled { interrupted_tool_calls: usize },
    /// The model repeated an identical tool call after being warned.
    LoopDetected { tool_name: String, repeats: usize },
    /// The response hit the output limit again after one automatic
    /// continuation.
    OutputTruncated,
    /// The model declined to continue (stop reason [`REFUSAL_STOP_REASON`];
    /// `content_filter` on OpenAI-compatible providers). What it wrote before
    /// stays in the transcript, and none of the `skipped_tool_calls` calls of
    /// that response ran. `note` is the provider's explanation, when it sent one.
    Refused {
        note: Option<String>,
        skipped_tool_calls: usize,
    },
}

/// Summary of one completed runtime turn, including tool results and usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnSummary {
    pub assistant_messages: Vec<ConversationMessage>,
    pub tool_results: Vec<ConversationMessage>,
    pub prompt_cache_events: Vec<PromptCacheEvent>,
    pub iterations: usize,
    pub usage: TokenUsage,
    pub auto_compaction: Option<AutoCompactionEvent>,
    pub outcome: TurnOutcome,
    /// Stop reason reported for the last model response.
    pub stop_reason: Option<String>,
    /// Tool results shortened by the context manager during this turn.
    pub pruned_tool_results: usize,
}

/// Details about automatic session compaction applied during a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoCompactionEvent {
    pub removed_message_count: usize,
}

#[derive(Default)]
struct TurnProgress {
    assistant_messages: Vec<ConversationMessage>,
    tool_results: Vec<ConversationMessage>,
    prompt_cache_events: Vec<PromptCacheEvent>,
    iterations: usize,
    compacted_messages: usize,
    pruned_tool_results: usize,
    stop_reason: Option<String>,
    auto_continued: bool,
    recovered_from_overflow: bool,
}

/// Coordinates the model loop, tool execution, hooks, and session updates.
pub struct ConversationRuntime<C, T> {
    session: Session,
    api_client: C,
    tool_executor: T,
    permission_policy: PermissionPolicy,
    system_prompt: Vec<String>,
    max_iterations: usize,
    usage_tracker: UsageTracker,
    hook_runner: HookRunner,
    context_manager: ContextManager,
    cancel: CancellationToken,
    hook_abort_signal: HookAbortSignal,
    hook_progress_reporter: Option<Box<dyn HookProgressReporter>>,
    session_tracer: Option<SessionTracer>,
}

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    #[must_use]
    pub fn new(
        session: Session,
        api_client: C,
        tool_executor: T,
        permission_policy: PermissionPolicy,
        system_prompt: Vec<String>,
    ) -> Self {
        Self::new_with_features(
            session,
            api_client,
            tool_executor,
            permission_policy,
            system_prompt,
            &RuntimeFeatureConfig::default(),
        )
    }

    #[must_use]
    #[allow(clippy::needless_pass_by_value)]
    pub fn new_with_features(
        session: Session,
        api_client: C,
        tool_executor: T,
        permission_policy: PermissionPolicy,
        system_prompt: Vec<String>,
        feature_config: &RuntimeFeatureConfig,
    ) -> Self {
        let usage_tracker = UsageTracker::from_session(&session);
        let mut context_manager =
            ContextManager::new(ContextBudget::default(), ContextPolicy::from_env());
        context_manager.set_threshold_override(
            auto_compaction_threshold_override_from_env().map(|threshold| threshold as usize),
        );
        Self {
            session,
            api_client,
            tool_executor,
            permission_policy,
            system_prompt,
            max_iterations: DEFAULT_MAX_ITERATIONS,
            usage_tracker,
            hook_runner: HookRunner::from_feature_config(feature_config),
            context_manager,
            cancel: CancellationToken::new(),
            hook_abort_signal: HookAbortSignal::default(),
            hook_progress_reporter: None,
            session_tracer: None,
        }
    }

    #[must_use]
    pub fn with_max_iterations(mut self, max_iterations: usize) -> Self {
        self.set_max_iterations(max_iterations);
        self
    }

    /// Sets the per-turn model-call cap (`0` is treated as `1`).
    pub fn set_max_iterations(&mut self, max_iterations: usize) {
        self.max_iterations = max_iterations.max(1);
    }

    #[must_use]
    pub fn max_iterations(&self) -> usize {
        self.max_iterations
    }

    /// Compacts once the estimated transcript reaches `threshold` tokens,
    /// replacing the percentage-of-budget rule. Note that this measures the
    /// current context size, not cumulative billed tokens.
    #[must_use]
    pub fn with_auto_compaction_input_tokens_threshold(mut self, threshold: u32) -> Self {
        self.context_manager
            .set_threshold_override(Some(threshold as usize));
        self
    }

    /// Installs the model-aware context manager. An explicit compaction
    /// threshold configured earlier (or via the environment) is kept unless
    /// `context_manager` sets its own.
    #[must_use]
    pub fn with_context_manager(mut self, context_manager: ContextManager) -> Self {
        self.set_context_manager(context_manager);
        self
    }

    pub fn set_context_manager(&mut self, mut context_manager: ContextManager) {
        if context_manager.policy().threshold_override_tokens.is_none() {
            context_manager
                .set_threshold_override(self.context_manager.policy().threshold_override_tokens);
        }
        self.context_manager = context_manager;
    }

    #[must_use]
    pub fn context_manager(&self) -> &ContextManager {
        &self.context_manager
    }

    /// Current estimated context usage against the model budget.
    #[must_use]
    pub fn context_snapshot(&self) -> ContextSnapshot {
        self.context_manager
            .snapshot(&self.session.messages, &self.system_prompt)
    }

    #[must_use]
    pub fn with_cancellation_token(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    /// Replaces the cancellation token; callers install a fresh token per turn.
    pub fn set_cancellation_token(&mut self, cancel: CancellationToken) {
        self.cancel = cancel;
    }

    #[must_use]
    pub fn cancellation_token(&self) -> &CancellationToken {
        &self.cancel
    }

    #[must_use]
    pub fn with_hook_abort_signal(mut self, hook_abort_signal: HookAbortSignal) -> Self {
        self.hook_abort_signal = hook_abort_signal;
        self
    }

    pub fn set_hook_abort_signal(&mut self, hook_abort_signal: HookAbortSignal) {
        self.hook_abort_signal = hook_abort_signal;
    }

    #[must_use]
    pub fn hook_abort_signal(&self) -> &HookAbortSignal {
        &self.hook_abort_signal
    }

    #[must_use]
    pub fn with_hook_progress_reporter(
        mut self,
        hook_progress_reporter: Box<dyn HookProgressReporter>,
    ) -> Self {
        self.hook_progress_reporter = Some(hook_progress_reporter);
        self
    }

    #[must_use]
    pub fn with_session_tracer(mut self, session_tracer: SessionTracer) -> Self {
        self.session_tracer = Some(session_tracer);
        self
    }

    fn sync_hook_abort(&self) {
        if self.cancel.is_cancelled() {
            self.hook_abort_signal.abort();
        }
    }

    fn run_pre_tool_use_hook(&mut self, tool_name: &str, input: &str) -> HookRunResult {
        self.sync_hook_abort();
        if let Some(reporter) = self.hook_progress_reporter.as_mut() {
            self.hook_runner.run_pre_tool_use_with_context(
                tool_name,
                input,
                Some(&self.hook_abort_signal),
                Some(reporter.as_mut()),
            )
        } else {
            self.hook_runner.run_pre_tool_use_with_context(
                tool_name,
                input,
                Some(&self.hook_abort_signal),
                None,
            )
        }
    }

    fn run_post_tool_use_hook(
        &mut self,
        tool_name: &str,
        input: &str,
        output: &str,
        is_error: bool,
    ) -> HookRunResult {
        self.sync_hook_abort();
        if let Some(reporter) = self.hook_progress_reporter.as_mut() {
            self.hook_runner.run_post_tool_use_with_context(
                tool_name,
                input,
                output,
                is_error,
                Some(&self.hook_abort_signal),
                Some(reporter.as_mut()),
            )
        } else {
            self.hook_runner.run_post_tool_use_with_context(
                tool_name,
                input,
                output,
                is_error,
                Some(&self.hook_abort_signal),
                None,
            )
        }
    }

    fn run_post_tool_use_failure_hook(
        &mut self,
        tool_name: &str,
        input: &str,
        output: &str,
    ) -> HookRunResult {
        self.sync_hook_abort();
        if let Some(reporter) = self.hook_progress_reporter.as_mut() {
            self.hook_runner.run_post_tool_use_failure_with_context(
                tool_name,
                input,
                output,
                Some(&self.hook_abort_signal),
                Some(reporter.as_mut()),
            )
        } else {
            self.hook_runner.run_post_tool_use_failure_with_context(
                tool_name,
                input,
                output,
                Some(&self.hook_abort_signal),
                None,
            )
        }
    }

    /// Runs one user turn: model call, tool execution, repeat until the model
    /// stops requesting tools.
    ///
    /// The transcript stays consistent no matter how the turn ends: every
    /// `tool_use` gets a `tool_result` (synthetic ones for interrupted or
    /// skipped calls), and each message is appended to the session file as
    /// it is produced. Cancellation, the repeated-call guard and output
    /// truncation end the turn with `Ok` and a [`TurnOutcome`]; provider
    /// failures and the iteration cap return `Err`. An interrupted or failed
    /// turn still ends with an assistant message (the partial answer and
    /// [`INTERRUPTED_RESPONSE_NOTE`], or a note about the failure), so the next
    /// turn's request does not answer this one's prompt again.
    pub fn run_turn(
        &mut self,
        user_input: impl Into<String>,
        prompter: Option<&mut dyn PermissionPrompter>,
    ) -> Result<TurnSummary, RuntimeError> {
        let user_input = user_input.into();

        // Repair transcripts left inconsistent by an earlier crash or failure
        // before adding to them.
        if normalize_messages(&mut self.session.messages).changed() {
            self.session.persist_snapshot().map_err(session_error)?;
        }

        self.record_turn_started(&user_input);
        self.session
            .push_user_text(user_input)
            .map_err(session_error)?;

        let result = self.run_turn_loop(prompter);
        if let Err(error) = &result {
            self.close_failed_turn(error);
        }
        result
    }

    /// The model and tool loop of [`Self::run_turn`], once the user message is
    /// in the session.
    #[allow(clippy::too_many_lines)]
    fn run_turn_loop(
        &mut self,
        mut prompter: Option<&mut dyn PermissionPrompter>,
    ) -> Result<TurnSummary, RuntimeError> {
        let mut progress = TurnProgress::default();
        let mut loop_guard = LoopGuard::new(LOOP_GUARD_THRESHOLD);

        loop {
            if self.cancel.is_cancelled() {
                return Ok(self.finish_turn(
                    progress,
                    TurnOutcome::Cancelled {
                        interrupted_tool_calls: 0,
                    },
                ));
            }
            progress.iterations += 1;
            if progress.iterations > self.max_iterations {
                let error = RuntimeError::with_kind(
                    RuntimeErrorKind::MaxIterations,
                    format!(
                        "conversation loop exceeded the maximum number of iterations ({}); stopping this turn. Raise the limit with the {MAX_ITERATIONS_ENV_VAR} environment variable (for example {MAX_ITERATIONS_ENV_VAR}=100) or the \"maxIterations\" setting.",
                        self.max_iterations
                    ),
                );
                self.record_turn_failed(progress.iterations, &error);
                return Err(error);
            }

            self.prepare_context(&mut progress)?;

            let request = ApiRequest {
                system_prompt: self.system_prompt.clone(),
                messages: request_messages(&self.session.messages),
            };
            let events = match self
                .api_client
                .stream_with_cancellation(request, &self.cancel)
            {
                Ok(events) => events,
                Err(error) if error.is_cancelled() || self.cancel.is_cancelled() => {
                    // What the user already saw stays in the transcript.
                    if let Some(text) = error.partial_text().filter(|text| !text.trim().is_empty())
                    {
                        let partial = ConversationMessage::assistant(vec![ContentBlock::Text {
                            text: format!("{}\n\n{INTERRUPTED_RESPONSE_NOTE}", text.trim_end()),
                        }]);
                        self.session
                            .push_message(partial.clone())
                            .map_err(session_error)?;
                        progress.assistant_messages.push(partial);
                    }
                    return Ok(self.finish_turn(
                        progress,
                        TurnOutcome::Cancelled {
                            interrupted_tool_calls: 0,
                        },
                    ));
                }
                Err(error)
                    if error.kind() == RuntimeErrorKind::ContextWindowExceeded
                        && !progress.recovered_from_overflow =>
                {
                    // The provider counted more tokens than the estimate:
                    // shrink the context and retry once.
                    progress.recovered_from_overflow = true;
                    if self.recover_from_context_overflow(&mut progress)? {
                        continue;
                    }
                    self.record_turn_failed(progress.iterations, &error);
                    return Err(error);
                }
                Err(error) => {
                    self.record_turn_failed(progress.iterations, &error);
                    return Err(error);
                }
            };
            let built = match build_assistant_message(events) {
                Ok(result) => result,
                Err(error) => {
                    self.record_turn_failed(progress.iterations, &error);
                    return Err(error);
                }
            };
            if let Some(usage) = built.usage {
                self.usage_tracker.record(usage);
            }
            progress
                .prompt_cache_events
                .extend(built.prompt_cache_events);
            progress.stop_reason.clone_from(&built.stop_reason);
            let truncated = is_truncation_stop_reason(built.stop_reason.as_deref());
            let refused = is_refusal_stop_reason(built.stop_reason.as_deref());
            let refusal_note = built.refusal;
            let assistant_message = built.message;
            let pending_tool_uses = assistant_message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse { id, name, input } => {
                        Some((id.clone(), name.clone(), input.clone()))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            self.record_assistant_iteration(
                progress.iterations,
                &assistant_message,
                pending_tool_uses.len(),
            );

            if refused {
                return self.finish_refused_turn(
                    progress,
                    assistant_message,
                    &pending_tool_uses,
                    refusal_note,
                );
            }

            self.session
                .push_message(assistant_message.clone())
                .map_err(session_error)?;
            progress.assistant_messages.push(assistant_message);

            if pending_tool_uses.is_empty() {
                if truncated && !progress.auto_continued {
                    progress.auto_continued = true;
                    self.session
                        .push_user_text(format!("{AUTO_CONTINUE_PREFIX} {AUTO_CONTINUE_MESSAGE}"))
                        .map_err(session_error)?;
                    continue;
                }
                let outcome = if truncated {
                    TurnOutcome::OutputTruncated
                } else {
                    TurnOutcome::Completed
                };
                return Ok(self.finish_turn(progress, outcome));
            }

            for (index, (tool_use_id, tool_name, input)) in pending_tool_uses.iter().enumerate() {
                if self.cancel.is_cancelled() {
                    let interrupted = self.push_synthetic_results(
                        progress.iterations,
                        &pending_tool_uses[index..],
                        INTERRUPTED_TOOL_RESULT,
                        &mut progress,
                    )?;
                    return Ok(self.finish_turn(
                        progress,
                        TurnOutcome::Cancelled {
                            interrupted_tool_calls: interrupted,
                        },
                    ));
                }

                // No call of a response the output limit cut off runs: its last
                // call is usually incomplete, and the model never finished the turn.
                if truncated {
                    let output = if is_complete_json(input) {
                        TRUNCATED_COMPLETE_TOOL_CALL_MESSAGE
                    } else {
                        TRUNCATED_TOOL_CALL_MESSAGE
                    };
                    let result = ConversationMessage::tool_result(
                        tool_use_id.clone(),
                        tool_name.clone(),
                        output,
                        true,
                    );
                    self.record_tool_result(progress.iterations, result, &mut progress)?;
                    continue;
                }

                // Arguments cut off or garbled while streaming reach neither the
                // hooks nor the tool: the model hears what was wrong and can retry.
                if let Err(problem) = tool_input_object(input) {
                    let result = ConversationMessage::tool_result(
                        tool_use_id.clone(),
                        tool_name.clone(),
                        format!("{INVALID_TOOL_ARGUMENTS_PREFIX} ({problem}). Send the call again with complete, valid JSON arguments."),
                        true,
                    );
                    self.record_tool_result(progress.iterations, result, &mut progress)?;
                    continue;
                }

                match loop_guard.observe(tool_name, input) {
                    LoopVerdict::Stop(repeats) => {
                        let result = ConversationMessage::tool_result(
                            tool_use_id.clone(),
                            tool_name.clone(),
                            format!(
                                "[loop guard] Not executed: this exact `{tool_name}` call was already made {} times in a row with identical arguments. The turn has been stopped.",
                                repeats - 1
                            ),
                            true,
                        );
                        self.record_tool_result(progress.iterations, result, &mut progress)?;
                        self.push_synthetic_results(
                            progress.iterations,
                            &pending_tool_uses[index + 1..],
                            LOOP_STOPPED_REMAINING_MESSAGE,
                            &mut progress,
                        )?;
                        return Ok(self.finish_turn(
                            progress,
                            TurnOutcome::LoopDetected {
                                tool_name: tool_name.clone(),
                                repeats,
                            },
                        ));
                    }
                    verdict => {
                        let ExecutedToolCall {
                            message: mut result,
                            interrupted,
                        } = self.execute_tool_use(
                            progress.iterations,
                            tool_use_id,
                            tool_name,
                            input,
                            reborrow_prompter(&mut prompter),
                        );
                        if let LoopVerdict::Warn(repeats) = verdict {
                            append_to_tool_result(
                                &mut result,
                                &format!(
                                    "[loop guard] You have made this exact `{tool_name}` call with identical arguments {repeats} times in a row. Repeating it will not produce a different result: change your approach (different arguments, a different tool, or answer the user). If you repeat it again, the turn will be stopped."
                                ),
                            );
                        }
                        self.record_tool_result(progress.iterations, result, &mut progress)?;
                        if interrupted {
                            let remaining = self.push_synthetic_results(
                                progress.iterations,
                                &pending_tool_uses[index + 1..],
                                INTERRUPTED_TOOL_RESULT,
                                &mut progress,
                            )?;
                            return Ok(self.finish_turn(
                                progress,
                                TurnOutcome::Cancelled {
                                    interrupted_tool_calls: remaining + 1,
                                },
                            ));
                        }
                    }
                }
            }
        }
    }

    /// Runs permission checks, hooks and the tool itself for one tool call
    /// and returns the (capped) result message.
    ///
    /// The pre-hook and the permission prompt can block for a while, so the
    /// cancellation token is checked again once they return: a call the user
    /// interrupted meanwhile is never started.
    fn execute_tool_use(
        &mut self,
        iteration: usize,
        tool_use_id: &str,
        tool_name: &str,
        input: &str,
        prompter: Option<&mut dyn PermissionPrompter>,
    ) -> ExecutedToolCall {
        let pre_hook_result = self.run_pre_tool_use_hook(tool_name, input);
        let effective_input = pre_hook_result
            .updated_input()
            .map_or_else(|| input.to_string(), ToOwned::to_owned);
        let permission_context = PermissionContext::new(
            pre_hook_result.permission_override(),
            pre_hook_result.permission_reason().map(ToOwned::to_owned),
        );

        let permission_outcome = if pre_hook_result.is_cancelled() {
            PermissionOutcome::Deny {
                reason: format_hook_message(
                    &pre_hook_result,
                    &format!("PreToolUse hook cancelled tool `{tool_name}`"),
                ),
            }
        } else if pre_hook_result.is_failed() {
            PermissionOutcome::Deny {
                reason: format_hook_message(
                    &pre_hook_result,
                    &format!("PreToolUse hook failed for tool `{tool_name}`"),
                ),
            }
        } else if pre_hook_result.is_denied() {
            PermissionOutcome::Deny {
                reason: format_hook_message(
                    &pre_hook_result,
                    &format!("PreToolUse hook denied tool `{tool_name}`"),
                ),
            }
        } else {
            self.permission_policy.authorize_with_context(
                tool_name,
                &effective_input,
                &permission_context,
                prompter,
            )
        };

        let interrupted = self.cancel.is_cancelled();
        let (output, is_error) = match permission_outcome {
            PermissionOutcome::Allow if interrupted => (INTERRUPTED_TOOL_RESULT.to_string(), true),
            PermissionOutcome::Allow => {
                self.record_tool_started(iteration, tool_name);
                let (mut output, mut is_error) = match self.tool_executor.execute_with_cancellation(
                    tool_name,
                    &effective_input,
                    &self.cancel,
                ) {
                    Ok(output) => (output, false),
                    Err(error) => (error.to_string(), true),
                };
                output = merge_hook_feedback(pre_hook_result.messages(), output, false);

                let post_hook_result = if is_error {
                    self.run_post_tool_use_failure_hook(tool_name, &effective_input, &output)
                } else {
                    self.run_post_tool_use_hook(tool_name, &effective_input, &output, false)
                };
                let post_hook_blocked = post_hook_result.is_denied()
                    || post_hook_result.is_failed()
                    || post_hook_result.is_cancelled();
                if post_hook_blocked {
                    is_error = true;
                }
                output =
                    merge_hook_feedback(post_hook_result.messages(), output, post_hook_blocked);
                (output, is_error)
            }
            PermissionOutcome::Deny { reason } => (
                merge_hook_feedback(pre_hook_result.messages(), reason, true),
                true,
            ),
        };
        ExecutedToolCall {
            message: ConversationMessage::tool_result(
                tool_use_id,
                tool_name,
                self.context_manager.cap_tool_result(output),
                is_error,
            ),
            interrupted,
        }
    }

    fn record_tool_result(
        &mut self,
        iteration: usize,
        result_message: ConversationMessage,
        progress: &mut TurnProgress,
    ) -> Result<(), RuntimeError> {
        self.session
            .push_message(result_message.clone())
            .map_err(session_error)?;
        self.record_tool_finished(iteration, &result_message);
        if let Some(ContentBlock::ToolResult {
            tool_use_id,
            tool_name,
            output,
            is_error,
        }) = result_message.blocks.first()
        {
            self.tool_executor
                .tool_call_finished(tool_use_id, tool_name, output, *is_error);
        }
        progress.tool_results.push(result_message);
        Ok(())
    }

    /// Ends a turn whose response the model declined to continue. Its text stays in
    /// the transcript, its tool calls get a result saying they did not run, and a
    /// note closes the turn when it would otherwise end without an answer (a refusal
    /// before any text, or after tool calls), so the next prompt is not taken for a
    /// retry of this one.
    fn finish_refused_turn(
        &mut self,
        mut progress: TurnProgress,
        message: ConversationMessage,
        tool_uses: &[(String, String, String)],
        note: Option<String>,
    ) -> Result<TurnSummary, RuntimeError> {
        if !message.blocks.is_empty() {
            self.session
                .push_message(message.clone())
                .map_err(session_error)?;
            progress.assistant_messages.push(message);
        }
        let skipped_tool_calls = self.push_synthetic_results(
            progress.iterations,
            tool_uses,
            REFUSED_TOOL_CALL_MESSAGE,
            &mut progress,
        )?;
        self.close_turn_with_note(format!("{FAILED_RESPONSE_NOTE_PREFIX}{REFUSAL_TURN_NOTE}]"));
        Ok(self.finish_turn(
            progress,
            TurnOutcome::Refused {
                note,
                skipped_tool_calls,
            },
        ))
    }

    fn push_synthetic_results(
        &mut self,
        iteration: usize,
        tool_uses: &[(String, String, String)],
        output: &str,
        progress: &mut TurnProgress,
    ) -> Result<usize, RuntimeError> {
        for (tool_use_id, tool_name, _) in tool_uses {
            let result = ConversationMessage::tool_result(
                tool_use_id.clone(),
                tool_name.clone(),
                output,
                true,
            );
            self.record_tool_result(iteration, result, progress)?;
        }
        Ok(tool_uses.len())
    }

    /// Checks the context budget before a model call and prunes or compacts
    /// the session when needed.
    fn prepare_context(&mut self, progress: &mut TurnProgress) -> Result<(), RuntimeError> {
        let preparation = self
            .context_manager
            .prepare_session(&mut self.session, &self.system_prompt);
        if !preparation.changed() {
            return Ok(());
        }
        progress.pruned_tool_results +=
            preparation.pruned_tool_results + preparation.truncated_recent_tool_results;
        progress.compacted_messages += preparation.compacted_messages;
        self.record_context_prepared(progress.iterations, &preparation);
        self.session.persist_snapshot().map_err(session_error)
    }

    /// Shrinks the session after the provider reported a context overflow.
    /// Returns `false` when nothing could be removed (retrying is pointless).
    fn recover_from_context_overflow(
        &mut self,
        progress: &mut TurnProgress,
    ) -> Result<bool, RuntimeError> {
        let preparation = self
            .context_manager
            .recover_from_overflow(&mut self.session, &self.system_prompt);
        if !preparation.changed() {
            return Ok(false);
        }
        progress.pruned_tool_results +=
            preparation.pruned_tool_results + preparation.truncated_recent_tool_results;
        progress.compacted_messages += preparation.compacted_messages;
        self.record_context_prepared(progress.iterations, &preparation);
        self.session.persist_snapshot().map_err(session_error)?;
        Ok(true)
    }

    fn finish_turn(&mut self, progress: TurnProgress, outcome: TurnOutcome) -> TurnSummary {
        if matches!(outcome, TurnOutcome::Cancelled { .. }) {
            self.close_turn_with_note(INTERRUPTED_RESPONSE_NOTE.to_string());
        }
        let summary = TurnSummary {
            assistant_messages: progress.assistant_messages,
            tool_results: progress.tool_results,
            prompt_cache_events: progress.prompt_cache_events,
            iterations: progress.iterations,
            usage: self.usage_tracker.cumulative_usage(),
            auto_compaction: (progress.compacted_messages > 0).then_some(AutoCompactionEvent {
                removed_message_count: progress.compacted_messages,
            }),
            outcome,
            stop_reason: progress.stop_reason,
            pruned_tool_results: progress.pruned_tool_results,
        };
        self.record_turn_completed(&summary);
        summary
    }

    /// Ends a failed turn with an assistant note naming the failure, unless the
    /// model's own message already ends it.
    fn close_failed_turn(&mut self, error: &RuntimeError) {
        let reason = error.to_string();
        let reason = reason.lines().next().unwrap_or_default().trim();
        let mut excerpt = reason
            .chars()
            .take(FAILED_RESPONSE_NOTE_MAX_CHARS)
            .collect::<String>();
        if excerpt.len() < reason.len() {
            excerpt.push('\u{2026}');
        }
        self.close_turn_with_note(format!("{FAILED_RESPONSE_NOTE_PREFIX}{excerpt}]"));
    }

    /// Appends `note` as the assistant's message when the transcript ends in a user
    /// or tool message: an unanswered prompt would be answered again by the next
    /// turn, and providers expect user and assistant messages to alternate. A
    /// session write that fails here leaves the transcript as it was.
    fn close_turn_with_note(&mut self, note: String) {
        let ends_unanswered = self
            .session
            .messages
            .last()
            .is_some_and(|message| message.role != MessageRole::Assistant);
        if ends_unanswered {
            let _ = self
                .session
                .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: note,
                }]));
        }
    }

    /// Compacts the session on request (`/compact`). The recent tail is kept
    /// by token budget — at most the context manager's retained share of the
    /// input budget and never more than half of the current transcript — but
    /// always at least `config.preserve_recent_messages` messages, without
    /// splitting tool-use / tool-result pairs. Sessions below
    /// `config.max_estimated_tokens` are left unchanged.
    #[must_use]
    pub fn compact(&self, config: CompactionConfig) -> CompactionResult {
        if !should_compact(&self.session, config) {
            return CompactionResult::unchanged(&self.session);
        }
        let snapshot = self.context_snapshot();
        let retained_share = snapshot
            .input_budget
            .saturating_mul(self.context_manager.policy().retained_tail_percent)
            / 100;
        compact_session_with_policy(
            &self.session,
            TokenCompaction {
                min_recent_messages: config.preserve_recent_messages,
                recent_tokens_budget: retained_share.min(snapshot.estimated_tokens / 2),
                max_summary_tokens: (snapshot.input_budget / 5)
                    .clamp(MIN_MANUAL_SUMMARY_TOKENS, DEFAULT_MAX_SUMMARY_TOKENS),
            },
        )
    }

    #[must_use]
    pub fn estimated_tokens(&self) -> usize {
        estimate_session_tokens(&self.session)
    }

    #[must_use]
    pub fn usage(&self) -> &UsageTracker {
        &self.usage_tracker
    }

    #[must_use]
    pub fn session(&self) -> &Session {
        &self.session
    }

    #[must_use]
    pub fn system_prompt(&self) -> &[String] {
        &self.system_prompt
    }

    #[must_use]
    pub fn permission_policy(&self) -> &PermissionPolicy {
        &self.permission_policy
    }

    #[must_use]
    pub fn api_client(&self) -> &C {
        &self.api_client
    }

    pub fn api_client_mut(&mut self) -> &mut C {
        &mut self.api_client
    }

    pub fn tool_executor_mut(&mut self) -> &mut T {
        &mut self.tool_executor
    }

    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    #[must_use]
    pub fn fork_session(&self, branch_name: Option<String>) -> Session {
        self.session.fork(branch_name)
    }

    #[must_use]
    pub fn into_session(self) -> Session {
        self.session
    }

    fn record_turn_started(&self, user_input: &str) {
        let Some(session_tracer) = &self.session_tracer else {
            return;
        };

        let mut attributes = Map::new();
        attributes.insert(
            "user_input".to_string(),
            Value::String(user_input.to_string()),
        );
        session_tracer.record("turn_started", attributes);
    }

    fn record_context_prepared(&self, iteration: usize, preparation: &ContextPreparation) {
        let Some(session_tracer) = &self.session_tracer else {
            return;
        };

        let mut attributes = Map::new();
        attributes.insert("iteration".to_string(), Value::from(iteration as u64));
        attributes.insert(
            "tokens_before".to_string(),
            Value::from(preparation.tokens_before as u64),
        );
        attributes.insert(
            "tokens_after".to_string(),
            Value::from(preparation.tokens_after as u64),
        );
        attributes.insert(
            "pruned_tool_results".to_string(),
            Value::from(preparation.pruned_tool_results as u64),
        );
        attributes.insert(
            "compacted_messages".to_string(),
            Value::from(preparation.compacted_messages as u64),
        );
        session_tracer.record("context_prepared", attributes);
    }

    fn record_assistant_iteration(
        &self,
        iteration: usize,
        assistant_message: &ConversationMessage,
        pending_tool_use_count: usize,
    ) {
        let Some(session_tracer) = &self.session_tracer else {
            return;
        };

        let mut attributes = Map::new();
        attributes.insert("iteration".to_string(), Value::from(iteration as u64));
        attributes.insert(
            "assistant_blocks".to_string(),
            Value::from(assistant_message.blocks.len() as u64),
        );
        attributes.insert(
            "pending_tool_use_count".to_string(),
            Value::from(pending_tool_use_count as u64),
        );
        session_tracer.record("assistant_iteration_completed", attributes);
    }

    fn record_tool_started(&self, iteration: usize, tool_name: &str) {
        let Some(session_tracer) = &self.session_tracer else {
            return;
        };

        let mut attributes = Map::new();
        attributes.insert("iteration".to_string(), Value::from(iteration as u64));
        attributes.insert(
            "tool_name".to_string(),
            Value::String(tool_name.to_string()),
        );
        session_tracer.record("tool_execution_started", attributes);
    }

    fn record_tool_finished(&self, iteration: usize, result_message: &ConversationMessage) {
        let Some(session_tracer) = &self.session_tracer else {
            return;
        };

        let Some(ContentBlock::ToolResult {
            tool_name,
            is_error,
            ..
        }) = result_message.blocks.first()
        else {
            return;
        };

        let mut attributes = Map::new();
        attributes.insert("iteration".to_string(), Value::from(iteration as u64));
        attributes.insert("tool_name".to_string(), Value::String(tool_name.clone()));
        attributes.insert("is_error".to_string(), Value::Bool(*is_error));
        session_tracer.record("tool_execution_finished", attributes);
    }

    fn record_turn_completed(&self, summary: &TurnSummary) {
        let Some(session_tracer) = &self.session_tracer else {
            return;
        };

        let mut attributes = Map::new();
        attributes.insert(
            "iterations".to_string(),
            Value::from(summary.iterations as u64),
        );
        attributes.insert(
            "assistant_messages".to_string(),
            Value::from(summary.assistant_messages.len() as u64),
        );
        attributes.insert(
            "tool_results".to_string(),
            Value::from(summary.tool_results.len() as u64),
        );
        attributes.insert(
            "prompt_cache_events".to_string(),
            Value::from(summary.prompt_cache_events.len() as u64),
        );
        attributes.insert(
            "outcome".to_string(),
            Value::String(format!("{:?}", summary.outcome)),
        );
        session_tracer.record("turn_completed", attributes);
    }

    fn record_turn_failed(&self, iteration: usize, error: &RuntimeError) {
        let Some(session_tracer) = &self.session_tracer else {
            return;
        };

        let mut attributes = Map::new();
        attributes.insert("iteration".to_string(), Value::from(iteration as u64));
        attributes.insert("error".to_string(), Value::String(error.to_string()));
        session_tracer.record("turn_failed", attributes);
    }
}

/// Reborrows the optional prompter for one tool call. The explicit match lets
/// the trait-object lifetime shrink to the reborrow (a coercion that does not
/// happen inside `Option::as_deref_mut`).
#[allow(clippy::manual_map)]
fn reborrow_prompter<'a>(
    prompter: &'a mut Option<&mut dyn PermissionPrompter>,
) -> Option<&'a mut dyn PermissionPrompter> {
    match prompter {
        Some(prompter) => Some(&mut **prompter),
        None => None,
    }
}

#[allow(clippy::needless_pass_by_value)]
fn session_error(error: crate::session::SessionError) -> RuntimeError {
    RuntimeError::with_kind(RuntimeErrorKind::Session, error.to_string())
}

/// Resolves the per-turn model-call cap: `NEURON_MAX_ITERATIONS` wins over the
/// configured value, which wins over [`DEFAULT_MAX_ITERATIONS`].
/// Non-positive or unparsable values are ignored.
///
/// The CLI passes the `maxIterations` settings key as `configured`.
#[must_use]
pub fn resolve_max_iterations(configured: Option<usize>) -> usize {
    parse_max_iterations(
        std::env::var(MAX_ITERATIONS_ENV_VAR).ok().as_deref(),
        configured,
    )
}

fn parse_max_iterations(env_value: Option<&str>, configured: Option<usize>) -> usize {
    env_value
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .or(configured.filter(|value| *value > 0))
        .unwrap_or(DEFAULT_MAX_ITERATIONS)
}

/// Reads the automatic compaction threshold from the environment.
#[must_use]
pub fn auto_compaction_threshold_from_env() -> u32 {
    parse_auto_compaction_threshold(
        std::env::var(AUTO_COMPACTION_THRESHOLD_ENV_VAR)
            .ok()
            .as_deref(),
    )
}

/// Explicit compaction threshold (estimated context tokens) from
/// `CLAUDE_CODE_AUTO_COMPACT_INPUT_TOKENS`, or `None` when unset or invalid so
/// the model-aware budget applies.
#[must_use]
pub fn auto_compaction_threshold_override_from_env() -> Option<u32> {
    std::env::var(AUTO_COMPACTION_THRESHOLD_ENV_VAR)
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .filter(|threshold| *threshold > 0)
}

#[must_use]
fn parse_auto_compaction_threshold(value: Option<&str>) -> u32 {
    value
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .filter(|threshold| *threshold > 0)
        .unwrap_or(DEFAULT_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD)
}

/// Whether a provider stop reason means the output token limit cut the
/// response off (`max_tokens`, `length`, ...).
#[must_use]
pub fn is_truncation_stop_reason(stop_reason: Option<&str>) -> bool {
    stop_reason.is_some_and(|reason| {
        TRUNCATED_STOP_REASONS
            .iter()
            .any(|candidate| reason.eq_ignore_ascii_case(candidate))
    })
}

/// Whether a provider stop reason means the model declined to continue
/// ([`REFUSAL_STOP_REASON`], or an un-normalized `content_filter`).
#[must_use]
pub fn is_refusal_stop_reason(stop_reason: Option<&str>) -> bool {
    stop_reason.is_some_and(|reason| {
        REFUSAL_STOP_REASONS
            .iter()
            .any(|candidate| reason.eq_ignore_ascii_case(candidate))
    })
}

/// Whether `text` is a note the runtime wrote to close an interrupted or failed
/// turn, not model output.
#[must_use]
pub fn is_turn_end_note(text: &str) -> bool {
    text == INTERRUPTED_RESPONSE_NOTE
        || (text.starts_with(FAILED_RESPONSE_NOTE_PREFIX) && text.ends_with(']'))
}

/// The transcript as the model sees it: [`normalized_messages`] without the notes that
/// close interrupted and failed turns in the session ([`INTERRUPTED_RESPONSE_NOTE`],
/// [`FAILED_RESPONSE_NOTE_PREFIX`]). Models took such an assistant note for an answer
/// and repeated it for the next question, so none reaches the model as the assistant's
/// words; the session keeps them all.
///
/// Every prompt stays a user message of its own, and nothing is added to what the
/// user typed: models carried out a stopped request, or answered a failed one instead
/// of the new question, when the old prompt and a note about it shared the new
/// prompt's message.
///
/// - A prompt answered only by a note keeps its own user turn, answered by a short
///   past-tense placeholder such as [`STOPPED_PROMPT_REPLY_FOR_MODEL`] (with a short
///   form of the error, for a failure). "Write it in Rust instead" still knows what
///   "it" is, and user and assistant messages keep alternating.
/// - A lone note that closes a turn which ran tools becomes an out-of-band assistant
///   note such as [`INTERRUPTED_TURN_NOTE_FOR_MODEL`]: a user message right after tool
///   results is refused by some providers.
/// - A partial answer ends with [`STOPPED_ANSWER_NOTE_FOR_MODEL`] instead of
///   [`INTERRUPTED_RESPONSE_NOTE`].
fn request_messages(messages: &[ConversationMessage]) -> Vec<ConversationMessage> {
    let mut request: Vec<ConversationMessage> = Vec::with_capacity(messages.len());
    for mut message in normalized_messages(messages) {
        if let Some(note) = TurnEndNote::of(&message) {
            // Right after the prompt: the model wrote nothing for it.
            let text = if request.last().is_some_and(is_user_prompt) {
                note.instead_of_answer()
            } else {
                note.after_tools()
            };
            request.push(ConversationMessage::assistant(vec![ContentBlock::Text {
                text,
            }]));
            continue;
        }
        mark_stopped_answer(&mut message);
        request.push(message);
    }
    request
}

/// How the runtime closed a turn the model wrote nothing for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TurnEndNote {
    Interrupted,
    /// The error excerpt the note names.
    Failed(String),
}

impl TurnEndNote {
    /// The note `message` consists of: an assistant message with nothing else.
    fn of(message: &ConversationMessage) -> Option<Self> {
        if message.role != MessageRole::Assistant {
            return None;
        }
        let [ContentBlock::Text { text }] = message.blocks.as_slice() else {
            return None;
        };
        let text = text.trim();
        if text == INTERRUPTED_RESPONSE_NOTE {
            return Some(Self::Interrupted);
        }
        text.strip_prefix(FAILED_RESPONSE_NOTE_PREFIX)
            .and_then(|rest| rest.strip_suffix(']'))
            .map(|reason| Self::Failed(reason.trim().to_string()))
    }

    /// The assistant's turn after the prompt the turn never answered.
    fn instead_of_answer(&self) -> String {
        match self {
            Self::Interrupted => STOPPED_PROMPT_REPLY_FOR_MODEL.to_string(),
            Self::Failed(reason) => format!(
                "{FAILED_PROMPT_REPLY_PREFIX_FOR_MODEL}{})",
                short_error(reason)
            ),
        }
    }

    /// Closes a turn that ran tools, in the assistant's turn: a note, not an answer.
    fn after_tools(&self) -> String {
        match self {
            Self::Interrupted => INTERRUPTED_TURN_NOTE_FOR_MODEL.to_string(),
            Self::Failed(reason) => format!(
                "(Note from Neuron, not a reply: this turn failed before an answer was written. The error: {reason})"
            ),
        }
    }
}

/// The part of a failed turn's error a request shows: up to the first `; ` (what
/// follows is retry timing, links and request ids), at most
/// [`FAILED_PROMPT_REPLY_ERROR_MAX_CHARS`] characters.
fn short_error(reason: &str) -> String {
    let reason = reason
        .split("; ")
        .next()
        .unwrap_or_default()
        .trim()
        .trim_end_matches('\u{2026}')
        .trim_end();
    if reason.chars().count() <= FAILED_PROMPT_REPLY_ERROR_MAX_CHARS {
        return reason.to_string();
    }
    let mut short = reason
        .chars()
        .take(FAILED_PROMPT_REPLY_ERROR_MAX_CHARS)
        .collect::<String>();
    // End on a word, not inside one.
    if let Some(space) = short.rfind(' ') {
        short.truncate(space);
    }
    format!("{}\u{2026}", short.trim_end())
}

/// Ends a partial answer the user stopped with [`STOPPED_ANSWER_NOTE_FOR_MODEL`] in
/// place of [`INTERRUPTED_RESPONSE_NOTE`]. A lone note is left alone ([`TurnEndNote`]
/// handles those).
fn mark_stopped_answer(message: &mut ConversationMessage) {
    if message.role != MessageRole::Assistant {
        return;
    }
    let Some(ContentBlock::Text { text }) = message.blocks.last_mut() else {
        return;
    };
    let Some(answer) = text.trim_end().strip_suffix(INTERRUPTED_RESPONSE_NOTE) else {
        return;
    };
    let answer = answer.trim_end();
    if answer.is_empty() {
        return;
    }
    *text = format!("{answer}\n\n{STOPPED_ANSWER_NOTE_FOR_MODEL}");
}

/// A message the user typed: text only, no tool results.
fn is_user_prompt(message: &ConversationMessage) -> bool {
    message.role == MessageRole::User
        && !message.blocks.is_empty()
        && message
            .blocks
            .iter()
            .all(|block| matches!(block, ContentBlock::Text { .. }))
}

fn is_complete_json(input: &str) -> bool {
    serde_json::from_str::<Value>(input).is_ok()
}

/// `Ok` when a tool call's arguments are a JSON object, as every tool's input is;
/// otherwise what is wrong with them, for the model.
fn tool_input_object(input: &str) -> Result<(), String> {
    match serde_json::from_str::<Value>(input) {
        Ok(Value::Object(_)) => Ok(()),
        Ok(other) => Err(format!(
            "they are a JSON {}",
            match other {
                Value::Null => "null",
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            }
        )),
        Err(_) if input.trim().is_empty() => Err("they are empty".to_string()),
        Err(error) => Err(format!("{error}; they may have been cut off")),
    }
}

fn append_to_tool_result(message: &mut ConversationMessage, note: &str) {
    for block in &mut message.blocks {
        if let ContentBlock::ToolResult { output, .. } = block {
            if !output.is_empty() {
                output.push_str("\n\n");
            }
            output.push_str(note);
        }
    }
}

/// Signature-based detector for a model calling the same tool with the same
/// arguments over and over.
struct LoopGuard {
    threshold: usize,
    last_signature: Option<String>,
    streak: usize,
    warned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopVerdict {
    Proceed,
    /// Execute, but tell the model it is repeating itself.
    Warn(usize),
    /// Do not execute; stop the turn.
    Stop(usize),
}

impl LoopGuard {
    fn new(threshold: usize) -> Self {
        Self {
            threshold,
            last_signature: None,
            streak: 0,
            warned: false,
        }
    }

    fn observe(&mut self, tool_name: &str, input: &str) -> LoopVerdict {
        let signature = tool_call_signature(tool_name, input);
        if self.last_signature.as_deref() == Some(signature.as_str()) {
            self.streak += 1;
        } else {
            self.last_signature = Some(signature);
            self.streak = 1;
        }
        if self.streak < self.threshold {
            LoopVerdict::Proceed
        } else if self.streak == self.threshold && !self.warned {
            self.warned = true;
            LoopVerdict::Warn(self.streak)
        } else {
            LoopVerdict::Stop(self.streak)
        }
    }
}

fn tool_call_signature(tool_name: &str, input: &str) -> String {
    let canonical_input = serde_json::from_str::<Value>(input)
        .map_or_else(|_| input.trim().to_string(), |value| value.to_string());
    format!("{tool_name}\u{0}{canonical_input}")
}

/// The result of one tool call, and whether the turn was cancelled before
/// the tool could start (while its pre-hook or permission prompt ran).
#[derive(Debug)]
struct ExecutedToolCall {
    message: ConversationMessage,
    interrupted: bool,
}

#[derive(Debug)]
struct BuiltAssistantMessage {
    /// Empty only for a refusal before any output.
    message: ConversationMessage,
    usage: Option<TokenUsage>,
    prompt_cache_events: Vec<PromptCacheEvent>,
    stop_reason: Option<String>,
    refusal: Option<String>,
}

fn build_assistant_message(
    events: Vec<AssistantEvent>,
) -> Result<BuiltAssistantMessage, RuntimeError> {
    let mut text = String::new();
    let mut blocks = Vec::new();
    let mut prompt_cache_events = Vec::new();
    let mut finished = false;
    let mut usage = None;
    let mut stop_reason = None;
    let mut refusal: Option<String> = None;

    for event in events {
        match event {
            AssistantEvent::TextDelta(delta) => text.push_str(&delta),
            AssistantEvent::ToolUse { id, name, input } => {
                flush_text_block(&mut text, &mut blocks);
                blocks.push(ContentBlock::ToolUse { id, name, input });
            }
            AssistantEvent::Usage(value) => usage = Some(value),
            AssistantEvent::PromptCache(event) => prompt_cache_events.push(event),
            AssistantEvent::StopReason(reason) => stop_reason = Some(reason),
            AssistantEvent::Refusal(note) => {
                refusal.get_or_insert_with(String::new).push_str(&note);
            }
            AssistantEvent::MessageStop => {
                finished = true;
            }
        }
    }

    flush_text_block(&mut text, &mut blocks);

    if !finished {
        return Err(RuntimeError::with_kind(
            RuntimeErrorKind::StreamInterrupted,
            "assistant stream ended without a message stop event",
        ));
    }
    // A refusal before any output is an answer of its own, not an empty reply.
    if blocks.is_empty() && !is_refusal_stop_reason(stop_reason.as_deref()) {
        return Err(RuntimeError::new("assistant stream produced no content"));
    }

    Ok(BuiltAssistantMessage {
        message: ConversationMessage::assistant_with_usage(blocks, usage),
        usage,
        prompt_cache_events,
        stop_reason,
        refusal: refusal
            .map(|note| note.trim().to_string())
            .filter(|note| !note.is_empty()),
    })
}

fn flush_text_block(text: &mut String, blocks: &mut Vec<ContentBlock>) {
    if !text.is_empty() {
        blocks.push(ContentBlock::Text {
            text: std::mem::take(text),
        });
    }
}

fn format_hook_message(result: &HookRunResult, fallback: &str) -> String {
    if result.messages().is_empty() {
        fallback.to_string()
    } else {
        result.messages().join("\n")
    }
}

fn merge_hook_feedback(messages: &[String], output: String, is_error: bool) -> String {
    if messages.is_empty() {
        return output;
    }

    let mut sections = Vec::new();
    if !output.trim().is_empty() {
        sections.push(output);
    }
    let label = if is_error {
        "Hook feedback (error)"
    } else {
        "Hook feedback"
    };
    sections.push(format!("{label}:\n{}", messages.join("\n")));
    sections.join("\n\n")
}

type ToolHandler = Box<dyn FnMut(&str) -> Result<String, ToolError>>;

/// Simple in-memory tool executor for tests and lightweight integrations.
#[derive(Default)]
pub struct StaticToolExecutor {
    handlers: BTreeMap<String, ToolHandler>,
}

impl StaticToolExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn register(
        mut self,
        tool_name: impl Into<String>,
        handler: impl FnMut(&str) -> Result<String, ToolError> + 'static,
    ) -> Self {
        self.handlers.insert(tool_name.into(), Box::new(handler));
        self
    }
}

impl ToolExecutor for StaticToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        self.handlers
            .get_mut(tool_name)
            .ok_or_else(|| ToolError::new(format!("unknown tool: {tool_name}")))?(input)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_assistant_message, parse_auto_compaction_threshold, ApiClient, ApiRequest,
        AssistantEvent, AutoCompactionEvent, ConversationRuntime, PromptCacheEvent, RuntimeError,
        StaticToolExecutor, ToolExecutor, DEFAULT_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD,
    };
    use crate::compact::CompactionConfig;
    use crate::config::{RuntimeFeatureConfig, RuntimeHookConfig};
    use crate::permissions::{
        PermissionMode, PermissionPolicy, PermissionPromptDecision, PermissionPrompter,
        PermissionRequest,
    };
    use crate::prompt::{ProjectContext, SystemPromptBuilder};
    use crate::session::{ContentBlock, MessageRole, Session};
    use crate::usage::TokenUsage;
    use crate::ToolError;
    use serde_json::Value;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use telemetry::{MemoryTelemetrySink, SessionTracer, TelemetryEvent};

    struct ScriptedApiClient {
        call_count: usize,
    }

    impl ApiClient for ScriptedApiClient {
        fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            self.call_count += 1;
            match self.call_count {
                1 => {
                    assert!(request
                        .messages
                        .iter()
                        .any(|message| message.role == MessageRole::User));
                    Ok(vec![
                        AssistantEvent::TextDelta("Let me calculate that.".to_string()),
                        AssistantEvent::ToolUse {
                            id: "tool-1".to_string(),
                            name: "add".to_string(),
                            input: r#"{"numbers":"2,2"}"#.to_string(),
                        },
                        AssistantEvent::Usage(TokenUsage {
                            input_tokens: 20,
                            output_tokens: 6,
                            cache_creation_input_tokens: 1,
                            cache_read_input_tokens: 2,
                        }),
                        AssistantEvent::MessageStop,
                    ])
                }
                2 => {
                    let last_message = request
                        .messages
                        .last()
                        .expect("tool result should be present");
                    assert_eq!(last_message.role, MessageRole::Tool);
                    Ok(vec![
                        AssistantEvent::TextDelta("The answer is 4.".to_string()),
                        AssistantEvent::Usage(TokenUsage {
                            input_tokens: 24,
                            output_tokens: 4,
                            cache_creation_input_tokens: 1,
                            cache_read_input_tokens: 3,
                        }),
                        AssistantEvent::PromptCache(PromptCacheEvent {
                            unexpected: true,
                            reason:
                                "cache read tokens dropped while prompt fingerprint remained stable"
                                    .to_string(),
                            previous_cache_read_input_tokens: 6_000,
                            current_cache_read_input_tokens: 1_000,
                            token_drop: 5_000,
                        }),
                        AssistantEvent::MessageStop,
                    ])
                }
                _ => unreachable!("extra API call"),
            }
        }
    }

    struct PromptAllowOnce;

    impl PermissionPrompter for PromptAllowOnce {
        fn decide(&mut self, request: &PermissionRequest) -> PermissionPromptDecision {
            assert_eq!(request.tool_name, "add");
            PermissionPromptDecision::Allow
        }
    }

    #[test]
    fn runs_user_to_tool_to_result_loop_end_to_end_and_tracks_usage() {
        let api_client = ScriptedApiClient { call_count: 0 };
        let tool_executor = StaticToolExecutor::new().register("add", |input| {
            let input: Value = serde_json::from_str(input).expect("input must be JSON");
            let total = input["numbers"]
                .as_str()
                .expect("numbers")
                .split(',')
                .map(|part| part.parse::<i32>().expect("input must be valid integer"))
                .sum::<i32>();
            Ok(total.to_string())
        });
        let permission_policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite);
        let system_prompt = SystemPromptBuilder::new()
            .with_project_context(ProjectContext {
                cwd: PathBuf::from("/tmp/project"),
                current_date: "2026-03-31".to_string(),
                git_status: None,
                git_diff: None,
                git_context: None,
                instruction_files: Vec::new(),
            })
            .with_os("linux", "6.8")
            .build();
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            api_client,
            tool_executor,
            permission_policy,
            system_prompt,
        );

        let summary = runtime
            .run_turn("what is 2 + 2?", Some(&mut PromptAllowOnce))
            .expect("conversation loop should succeed");

        assert_eq!(summary.iterations, 2);
        assert_eq!(summary.assistant_messages.len(), 2);
        assert_eq!(summary.tool_results.len(), 1);
        assert_eq!(summary.prompt_cache_events.len(), 1);
        assert_eq!(runtime.session().messages.len(), 4);
        assert_eq!(summary.usage.output_tokens, 10);
        assert_eq!(summary.auto_compaction, None);
        assert!(matches!(
            runtime.session().messages[1].blocks[1],
            ContentBlock::ToolUse { .. }
        ));
        assert!(matches!(
            runtime.session().messages[2].blocks[0],
            ContentBlock::ToolResult {
                is_error: false,
                ..
            }
        ));
    }

    #[test]
    fn records_runtime_session_trace_events() {
        let sink = Arc::new(MemoryTelemetrySink::default());
        let tracer = SessionTracer::new("session-runtime", sink.clone());
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            ScriptedApiClient { call_count: 0 },
            StaticToolExecutor::new().register("add", |_input| Ok("4".to_string())),
            PermissionPolicy::new(PermissionMode::WorkspaceWrite),
            vec!["system".to_string()],
        )
        .with_session_tracer(tracer);

        runtime
            .run_turn("what is 2 + 2?", Some(&mut PromptAllowOnce))
            .expect("conversation loop should succeed");

        let events = sink.events();
        let trace_names = events
            .iter()
            .filter_map(|event| match event {
                TelemetryEvent::SessionTrace(trace) => Some(trace.name.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert!(trace_names.contains(&"turn_started"));
        assert!(trace_names.contains(&"assistant_iteration_completed"));
        assert!(trace_names.contains(&"tool_execution_started"));
        assert!(trace_names.contains(&"tool_execution_finished"));
        assert!(trace_names.contains(&"turn_completed"));
    }

    #[test]
    fn records_denied_tool_results_when_prompt_rejects() {
        struct RejectPrompter;
        impl PermissionPrompter for RejectPrompter {
            fn decide(&mut self, _request: &PermissionRequest) -> PermissionPromptDecision {
                PermissionPromptDecision::Deny {
                    reason: "not now".to_string(),
                }
            }
        }

        struct SingleCallApiClient;
        impl ApiClient for SingleCallApiClient {
            fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                if request
                    .messages
                    .iter()
                    .any(|message| message.role == MessageRole::Tool)
                {
                    return Ok(vec![
                        AssistantEvent::TextDelta("I could not use the tool.".to_string()),
                        AssistantEvent::MessageStop,
                    ]);
                }
                Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "tool-1".to_string(),
                        name: "blocked".to_string(),
                        input: r#"{"secret":true}"#.to_string(),
                    },
                    AssistantEvent::MessageStop,
                ])
            }
        }

        let mut runtime = ConversationRuntime::new(
            Session::new(),
            SingleCallApiClient,
            StaticToolExecutor::new(),
            PermissionPolicy::new(PermissionMode::WorkspaceWrite),
            vec!["system".to_string()],
        );

        let summary = runtime
            .run_turn("use the tool", Some(&mut RejectPrompter))
            .expect("conversation should continue after denied tool");

        assert_eq!(summary.tool_results.len(), 1);
        assert!(matches!(
            &summary.tool_results[0].blocks[0],
            ContentBlock::ToolResult { is_error: true, output, .. } if output == "not now"
        ));
    }

    #[test]
    fn denies_tool_use_when_pre_tool_hook_blocks() {
        struct SingleCallApiClient;
        impl ApiClient for SingleCallApiClient {
            fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                if request
                    .messages
                    .iter()
                    .any(|message| message.role == MessageRole::Tool)
                {
                    return Ok(vec![
                        AssistantEvent::TextDelta("blocked".to_string()),
                        AssistantEvent::MessageStop,
                    ]);
                }
                Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "tool-1".to_string(),
                        name: "blocked".to_string(),
                        input: r#"{"path":"secret.txt"}"#.to_string(),
                    },
                    AssistantEvent::MessageStop,
                ])
            }
        }

        let mut runtime = ConversationRuntime::new_with_features(
            Session::new(),
            SingleCallApiClient,
            StaticToolExecutor::new().register("blocked", |_input| {
                panic!("tool should not execute when hook denies")
            }),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
            &RuntimeFeatureConfig::default().with_hooks(RuntimeHookConfig::new(
                vec![shell_snippet("printf 'blocked by hook'; exit 2")],
                Vec::new(),
                Vec::new(),
            )),
        );

        let summary = runtime
            .run_turn("use the tool", None)
            .expect("conversation should continue after hook denial");

        assert_eq!(summary.tool_results.len(), 1);
        let ContentBlock::ToolResult {
            is_error, output, ..
        } = &summary.tool_results[0].blocks[0]
        else {
            panic!("expected tool result block");
        };
        assert!(
            *is_error,
            "hook denial should produce an error result: {output}"
        );
        assert!(
            output.contains("denied tool") || output.contains("blocked by hook"),
            "unexpected hook denial output: {output:?}"
        );
    }

    #[test]
    fn denies_tool_use_when_pre_tool_hook_fails() {
        struct SingleCallApiClient;
        impl ApiClient for SingleCallApiClient {
            fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                if request
                    .messages
                    .iter()
                    .any(|message| message.role == MessageRole::Tool)
                {
                    return Ok(vec![
                        AssistantEvent::TextDelta("failed".to_string()),
                        AssistantEvent::MessageStop,
                    ]);
                }
                Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "tool-1".to_string(),
                        name: "blocked".to_string(),
                        input: r#"{"path":"secret.txt"}"#.to_string(),
                    },
                    AssistantEvent::MessageStop,
                ])
            }
        }

        // given
        let mut runtime = ConversationRuntime::new_with_features(
            Session::new(),
            SingleCallApiClient,
            StaticToolExecutor::new().register("blocked", |_input| {
                panic!("tool should not execute when hook fails")
            }),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
            &RuntimeFeatureConfig::default().with_hooks(RuntimeHookConfig::new(
                vec![shell_snippet("printf 'broken hook'; exit 1")],
                Vec::new(),
                Vec::new(),
            )),
        );

        // when
        let summary = runtime
            .run_turn("use the tool", None)
            .expect("conversation should continue after hook failure");

        // then
        assert_eq!(summary.tool_results.len(), 1);
        let ContentBlock::ToolResult {
            is_error, output, ..
        } = &summary.tool_results[0].blocks[0]
        else {
            panic!("expected tool result block");
        };
        assert!(
            *is_error,
            "hook failure should produce an error result: {output}"
        );
        assert!(
            output.contains("exited with status 1") || output.contains("broken hook"),
            "unexpected hook failure output: {output:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn appends_post_tool_hook_feedback_to_tool_result() {
        struct TwoCallApiClient {
            calls: usize,
        }

        impl ApiClient for TwoCallApiClient {
            fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                self.calls += 1;
                match self.calls {
                    1 => Ok(vec![
                        AssistantEvent::ToolUse {
                            id: "tool-1".to_string(),
                            name: "add".to_string(),
                            input: r#"{"lhs":2,"rhs":2}"#.to_string(),
                        },
                        AssistantEvent::MessageStop,
                    ]),
                    2 => {
                        assert!(request
                            .messages
                            .iter()
                            .any(|message| message.role == MessageRole::Tool));
                        Ok(vec![
                            AssistantEvent::TextDelta("done".to_string()),
                            AssistantEvent::MessageStop,
                        ])
                    }
                    _ => unreachable!("extra API call"),
                }
            }
        }

        let mut runtime = ConversationRuntime::new_with_features(
            Session::new(),
            TwoCallApiClient { calls: 0 },
            StaticToolExecutor::new().register("add", |_input| Ok("4".to_string())),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
            &RuntimeFeatureConfig::default().with_hooks(RuntimeHookConfig::new(
                vec![shell_snippet("printf 'pre hook ran'")],
                vec![shell_snippet("printf 'post hook ran'")],
                Vec::new(),
            )),
        );

        let summary = runtime
            .run_turn("use add", None)
            .expect("tool loop succeeds");

        assert_eq!(summary.tool_results.len(), 1);
        let ContentBlock::ToolResult {
            is_error, output, ..
        } = &summary.tool_results[0].blocks[0]
        else {
            panic!("expected tool result block");
        };
        assert!(
            !*is_error,
            "post hook should preserve non-error result: {output:?}"
        );
        assert!(
            output.contains('4'),
            "tool output missing value: {output:?}"
        );
        assert!(
            output.contains("pre hook ran"),
            "tool output missing pre hook feedback: {output:?}"
        );
        assert!(
            output.contains("post hook ran"),
            "tool output missing post hook feedback: {output:?}"
        );
    }

    #[test]
    fn appends_post_tool_use_failure_hook_feedback_to_tool_result() {
        struct TwoCallApiClient {
            calls: usize,
        }

        impl ApiClient for TwoCallApiClient {
            fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                self.calls += 1;
                match self.calls {
                    1 => Ok(vec![
                        AssistantEvent::ToolUse {
                            id: "tool-1".to_string(),
                            name: "fail".to_string(),
                            input: r#"{"path":"README.md"}"#.to_string(),
                        },
                        AssistantEvent::MessageStop,
                    ]),
                    2 => {
                        assert!(request
                            .messages
                            .iter()
                            .any(|message| message.role == MessageRole::Tool));
                        Ok(vec![
                            AssistantEvent::TextDelta("done".to_string()),
                            AssistantEvent::MessageStop,
                        ])
                    }
                    _ => unreachable!("extra API call"),
                }
            }
        }

        // given
        let mut runtime = ConversationRuntime::new_with_features(
            Session::new(),
            TwoCallApiClient { calls: 0 },
            StaticToolExecutor::new()
                .register("fail", |_input| Err(ToolError::new("tool exploded"))),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
            &RuntimeFeatureConfig::default().with_hooks(RuntimeHookConfig::new(
                Vec::new(),
                vec![shell_snippet("printf 'post hook should not run'")],
                vec![shell_snippet("printf 'failure hook ran'")],
            )),
        );

        // when
        let summary = runtime
            .run_turn("use fail", None)
            .expect("tool loop succeeds");

        // then
        assert_eq!(summary.tool_results.len(), 1);
        let ContentBlock::ToolResult {
            is_error, output, ..
        } = &summary.tool_results[0].blocks[0]
        else {
            panic!("expected tool result block");
        };
        assert!(
            *is_error,
            "failure hook path should preserve error result: {output:?}"
        );
        assert!(
            output.contains("tool exploded"),
            "tool output missing failure reason: {output:?}"
        );
        assert!(
            output.contains("failure hook ran"),
            "tool output missing failure hook feedback: {output:?}"
        );
        assert!(
            !output.contains("post hook should not run"),
            "normal post hook should not run on tool failure: {output:?}"
        );
    }

    #[test]
    fn reconstructs_usage_tracker_from_restored_session() {
        struct SimpleApi;
        impl ApiClient for SimpleApi {
            fn stream(
                &mut self,
                _request: ApiRequest,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ])
            }
        }

        let mut session = Session::new();
        session
            .messages
            .push(crate::session::ConversationMessage::assistant_with_usage(
                vec![ContentBlock::Text {
                    text: "earlier".to_string(),
                }],
                Some(TokenUsage {
                    input_tokens: 11,
                    output_tokens: 7,
                    cache_creation_input_tokens: 2,
                    cache_read_input_tokens: 1,
                }),
            ));

        let runtime = ConversationRuntime::new(
            session,
            SimpleApi,
            StaticToolExecutor::new(),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        );

        assert_eq!(runtime.usage().turns(), 1);
        assert_eq!(runtime.usage().cumulative_usage().total_tokens(), 21);
    }

    #[test]
    fn compacts_session_after_turns() {
        struct SimpleApi;
        impl ApiClient for SimpleApi {
            fn stream(
                &mut self,
                _request: ApiRequest,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ])
            }
        }

        let mut runtime = ConversationRuntime::new(
            Session::new(),
            SimpleApi,
            StaticToolExecutor::new(),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        );
        runtime.run_turn("a", None).expect("turn a");
        runtime.run_turn("b", None).expect("turn b");
        runtime.run_turn("c", None).expect("turn c");

        let result = runtime.compact(CompactionConfig {
            preserve_recent_messages: 2,
            max_estimated_tokens: 1,
        });
        assert!(result.summary.contains("Conversation summary"));
        assert_eq!(
            result.compacted_session.messages[0].role,
            MessageRole::System
        );
        assert_eq!(
            result.compacted_session.session_id,
            runtime.session().session_id
        );
        assert!(result.compacted_session.compaction.is_some());
    }

    #[test]
    fn persists_conversation_turn_messages_to_jsonl_session() {
        struct SimpleApi;
        impl ApiClient for SimpleApi {
            fn stream(
                &mut self,
                _request: ApiRequest,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ])
            }
        }

        let path = temp_session_path("persisted-turn");
        let session = Session::new().with_persistence_path(path.clone());
        let mut runtime = ConversationRuntime::new(
            session,
            SimpleApi,
            StaticToolExecutor::new(),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        );

        runtime
            .run_turn("persist this turn", None)
            .expect("turn should succeed");

        let restored = Session::load_from_path(&path).expect("persisted session should reload");
        fs::remove_file(&path).expect("temp session file should be removable");

        assert_eq!(restored.messages.len(), 2);
        assert_eq!(restored.messages[0].role, MessageRole::User);
        assert_eq!(restored.messages[1].role, MessageRole::Assistant);
        assert_eq!(restored.session_id, runtime.session().session_id);
    }

    #[test]
    fn forks_runtime_session_without_mutating_original() {
        let mut session = Session::new();
        session
            .push_user_text("branch me")
            .expect("message should append");

        let runtime = ConversationRuntime::new(
            session.clone(),
            ScriptedApiClient { call_count: 0 },
            StaticToolExecutor::new(),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        );

        let forked = runtime.fork_session(Some("alt-path".to_string()));

        assert_eq!(forked.messages, session.messages);
        assert_ne!(forked.session_id, session.session_id);
        assert_eq!(
            forked
                .fork
                .as_ref()
                .map(|fork| (fork.parent_session_id.as_str(), fork.branch_name.as_deref())),
            Some((session.session_id.as_str(), Some("alt-path")))
        );
        assert!(runtime.session().fork.is_none());
    }

    fn temp_session_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("runtime-conversation-{label}-{nanos}.json"))
    }

    #[cfg(windows)]
    fn shell_snippet(script: &str) -> String {
        let command = script.strip_prefix("printf '").expect("printf fixture");
        let (message, remainder) = command.split_once('\'').expect("quoted fixture message");
        match remainder.strip_prefix("; exit ") {
            Some(code) => format!("echo {message}& exit /b {code}"),
            None => format!("echo {message}"),
        }
    }

    #[cfg(not(windows))]
    fn shell_snippet(script: &str) -> String {
        script.to_string()
    }

    #[test]
    fn does_not_auto_compact_on_cumulative_billed_tokens() {
        // Billed input tokens accumulate across calls; they say nothing about
        // the size of the current context, so they must not trigger compaction.
        struct SimpleApi;
        impl ApiClient for SimpleApi {
            fn stream(
                &mut self,
                _request: ApiRequest,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::Usage(TokenUsage {
                        input_tokens: 120_000,
                        output_tokens: 4,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                    }),
                    AssistantEvent::MessageStop,
                ])
            }
        }

        let mut session = Session::new();
        session.messages = vec![
            crate::session::ConversationMessage::user_text("one"),
            crate::session::ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "two".to_string(),
            }]),
            crate::session::ConversationMessage::user_text("three"),
            crate::session::ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "four".to_string(),
            }]),
        ];

        let mut runtime = ConversationRuntime::new(
            session,
            SimpleApi,
            StaticToolExecutor::new(),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        )
        .with_auto_compaction_input_tokens_threshold(100_000);

        runtime
            .run_turn("first", None)
            .expect("turn should succeed");
        let summary = runtime
            .run_turn("second", None)
            .expect("turn should succeed");

        assert!(runtime.usage().cumulative_usage().input_tokens >= 240_000);
        assert_eq!(summary.auto_compaction, None);
        assert_eq!(runtime.session().messages[0].role, MessageRole::User);
    }

    #[test]
    fn auto_compacts_before_the_model_call_when_current_context_exceeds_threshold() {
        struct RecordingApi {
            first_request_roles: Option<Vec<MessageRole>>,
        }
        impl ApiClient for RecordingApi {
            fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                self.first_request_roles
                    .get_or_insert_with(|| request.messages.iter().map(|m| m.role).collect());
                Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ])
            }
        }

        let mut session = Session::new();
        for index in 0..10 {
            session
                .messages
                .push(crate::session::ConversationMessage::user_text(format!(
                    "request {index} {}",
                    "context ".repeat(50)
                )));
            session
                .messages
                .push(crate::session::ConversationMessage::assistant(vec![
                    ContentBlock::Text {
                        text: format!("answer {index}"),
                    },
                ]));
        }

        let mut runtime = ConversationRuntime::new(
            session,
            RecordingApi {
                first_request_roles: None,
            },
            StaticToolExecutor::new(),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        )
        .with_auto_compaction_input_tokens_threshold(500);

        let summary = runtime
            .run_turn("trigger", None)
            .expect("turn should succeed");

        let removed = summary
            .auto_compaction
            .expect("compaction should run")
            .removed_message_count;
        assert!(removed > 0);
        // The compacted transcript is what the model saw on its first call.
        let roles = runtime
            .api_client()
            .first_request_roles
            .clone()
            .expect("request recorded");
        assert_eq!(roles[0], MessageRole::System);
        assert_eq!(runtime.session().messages[0].role, MessageRole::System);
        assert!(runtime.estimated_tokens() < 21 * 110);
    }

    #[test]
    fn skips_auto_compaction_below_threshold() {
        struct SimpleApi;
        impl ApiClient for SimpleApi {
            fn stream(
                &mut self,
                _request: ApiRequest,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::Usage(TokenUsage {
                        input_tokens: 99_999,
                        output_tokens: 4,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                    }),
                    AssistantEvent::MessageStop,
                ])
            }
        }

        let mut runtime = ConversationRuntime::new(
            Session::new(),
            SimpleApi,
            StaticToolExecutor::new(),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        )
        .with_auto_compaction_input_tokens_threshold(100_000);

        let summary = runtime
            .run_turn("trigger", None)
            .expect("turn should succeed");
        assert_eq!(summary.auto_compaction, None);
        assert_eq!(runtime.session().messages.len(), 2);
    }

    #[test]
    fn auto_compaction_threshold_defaults_and_parses_values() {
        assert_eq!(
            parse_auto_compaction_threshold(None),
            DEFAULT_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD
        );
        assert_eq!(parse_auto_compaction_threshold(Some("4321")), 4321);
        assert_eq!(
            parse_auto_compaction_threshold(Some("0")),
            DEFAULT_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD
        );
        assert_eq!(
            parse_auto_compaction_threshold(Some("not-a-number")),
            DEFAULT_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD
        );
    }

    #[test]
    fn compacted_session_runs_without_a_health_probe_tool_call() {
        struct SimpleApi;
        impl ApiClient for SimpleApi {
            fn stream(
                &mut self,
                _request: ApiRequest,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ])
            }
        }

        let mut session = Session::new();
        session.record_compaction("summarized earlier work", 4);
        session
            .push_user_text("previous message")
            .expect("message should append");

        // A tool executor where glob_search is unavailable (for example not in
        // --allowedTools) must not brick compacted sessions.
        let tool_executor = StaticToolExecutor::new().register("glob_search", |_input| {
            panic!("the runtime must not call tools on its own")
        });
        let mut runtime = ConversationRuntime::new(
            session,
            SimpleApi,
            tool_executor,
            PermissionPolicy::new(PermissionMode::ReadOnly),
            vec!["system".to_string()],
        );

        for prompt in ["first", "second"] {
            let summary = runtime.run_turn(prompt, None).expect("turn should succeed");
            assert_eq!(summary.outcome, super::TurnOutcome::Completed);
        }
        assert_eq!(runtime.session().messages.len(), 5);
    }

    #[test]
    fn compaction_health_probe_skips_empty_compacted_session() {
        struct SimpleApi;
        impl ApiClient for SimpleApi {
            fn stream(
                &mut self,
                _request: ApiRequest,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ])
            }
        }

        let mut session = Session::new();
        session.record_compaction("fresh summary", 2);

        let tool_executor = StaticToolExecutor::new().register("glob_search", |_input| {
            Err(ToolError::new(
                "glob_search should not run for an empty compacted session",
            ))
        });
        let mut runtime = ConversationRuntime::new(
            session,
            SimpleApi,
            tool_executor,
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        );

        let summary = runtime
            .run_turn("trigger", None)
            .expect("empty compacted session should not fail health probe");
        assert_eq!(summary.auto_compaction, None);
        assert_eq!(runtime.session().messages.len(), 2);
    }

    #[test]
    fn build_assistant_message_requires_message_stop_event() {
        // given
        let events = vec![AssistantEvent::TextDelta("hello".to_string())];

        // when
        let error = build_assistant_message(events)
            .expect_err("assistant messages should require a stop event");

        // then
        assert!(error
            .to_string()
            .contains("assistant stream ended without a message stop event"));
    }

    #[test]
    fn build_assistant_message_requires_content() {
        // given
        let events = vec![AssistantEvent::MessageStop];

        // when
        let error =
            build_assistant_message(events).expect_err("assistant messages should require content");

        // then
        assert!(error
            .to_string()
            .contains("assistant stream produced no content"));
    }

    #[test]
    fn static_tool_executor_rejects_unknown_tools() {
        // given
        let mut executor = StaticToolExecutor::new();

        // when
        let error = executor
            .execute("missing", "{}")
            .expect_err("unregistered tools should fail");

        // then
        assert_eq!(error.to_string(), "unknown tool: missing");
    }

    #[test]
    fn run_turn_errors_when_max_iterations_is_exceeded() {
        struct LoopingApi;

        impl ApiClient for LoopingApi {
            fn stream(
                &mut self,
                _request: ApiRequest,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "tool-1".to_string(),
                        name: "echo".to_string(),
                        input: r#"{"payload":1}"#.to_string(),
                    },
                    AssistantEvent::MessageStop,
                ])
            }
        }

        // given
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            LoopingApi,
            StaticToolExecutor::new().register("echo", |input| Ok(input.to_string())),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        )
        .with_max_iterations(1);

        // when
        let error = runtime
            .run_turn("loop", None)
            .expect_err("conversation loop should stop after the configured limit");

        // then
        assert!(error
            .to_string()
            .contains("conversation loop exceeded the maximum number of iterations"));
    }

    #[test]
    fn run_turn_propagates_api_errors() {
        struct FailingApi;

        impl ApiClient for FailingApi {
            fn stream(
                &mut self,
                _request: ApiRequest,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Err(RuntimeError::new("upstream failed"))
            }
        }

        // given
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            FailingApi,
            StaticToolExecutor::new(),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        );

        // when
        let error = runtime
            .run_turn("hello", None)
            .expect_err("API failures should propagate");

        // then
        assert_eq!(error.to_string(), "upstream failed");
    }
}

#[cfg(test)]
mod resilience_tests {
    use super::{
        is_turn_end_note, parse_max_iterations, ApiClient, ApiRequest, AssistantEvent,
        AutoCompactionEvent, ConversationRuntime, RuntimeError, RuntimeErrorKind,
        StaticToolExecutor, ToolExecutor, TurnOutcome, DEFAULT_MAX_ITERATIONS,
        FAILED_RESPONSE_NOTE_PREFIX, INTERRUPTED_RESPONSE_NOTE, INTERRUPTED_TOOL_RESULT,
        MAX_ITERATIONS_ENV_VAR, REFUSAL_STOP_REASON,
    };
    use crate::cancel::CancellationToken;
    use crate::compact::AUTO_CONTINUE_PREFIX;
    use crate::context_manager::{ContextBudget, ContextManager, ContextPolicy};
    use crate::permissions::{
        PermissionMode, PermissionPolicy, PermissionPromptDecision, PermissionPrompter,
        PermissionRequest,
    };
    use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};
    use crate::transcript::unanswered_tool_use_ids;
    use crate::usage::TokenUsage;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{SystemTime, UNIX_EPOCH};

    type Response = Result<Vec<AssistantEvent>, RuntimeError>;

    /// Replays scripted responses (panicking on unexpected extra calls) and
    /// records every request it receives.
    struct ScriptedApi {
        responses: Vec<Response>,
        requests: Vec<ApiRequest>,
        on_call: Option<Box<dyn FnMut(usize)>>,
    }

    impl ScriptedApi {
        fn new(responses: Vec<Response>) -> Self {
            Self {
                responses,
                requests: Vec::new(),
                on_call: None,
            }
        }
    }

    impl ApiClient for ScriptedApi {
        fn stream(&mut self, request: ApiRequest) -> Response {
            self.requests.push(request);
            if let Some(on_call) = self.on_call.as_mut() {
                on_call(self.requests.len());
            }
            assert!(!self.responses.is_empty(), "unexpected extra model call");
            self.responses.remove(0)
        }
    }

    fn tool_call(id: &str, name: &str, input: &str) -> AssistantEvent {
        AssistantEvent::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: input.to_string(),
        }
    }

    fn text(value: &str) -> AssistantEvent {
        AssistantEvent::TextDelta(value.to_string())
    }

    fn runtime_with<C: ApiClient, T: ToolExecutor>(
        session: Session,
        api: C,
        tools: T,
    ) -> ConversationRuntime<C, T> {
        ConversationRuntime::new(
            session,
            api,
            tools,
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        )
    }

    fn tool_results(session: &Session) -> Vec<(String, String, bool)> {
        session
            .messages
            .iter()
            .flat_map(|message| message.blocks.iter())
            .filter_map(|block| match block {
                ContentBlock::ToolResult {
                    tool_use_id,
                    output,
                    is_error,
                    ..
                } => Some((tool_use_id.clone(), output.clone(), *is_error)),
                _ => None,
            })
            .collect()
    }

    fn temp_session_path(label: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("runtime-resilience-{label}-{nanos}.jsonl"))
    }

    #[test]
    fn cancellation_mid_tool_chain_synthesizes_interrupted_results_and_persists() {
        let token = CancellationToken::new();
        let cancel_from_tool = token.clone();
        let api = ScriptedApi::new(vec![Ok(vec![
            tool_call("a", "first", "{}"),
            tool_call("b", "second", "{}"),
            AssistantEvent::MessageStop,
        ])]);
        let tools = StaticToolExecutor::new()
            .register("first", move |_| {
                // Ctrl-C arrives while the first tool is running.
                cancel_from_tool.cancel();
                Ok("first ran".to_string())
            })
            .register("second", |_| panic!("cancelled tools must not run"));
        let path = temp_session_path("cancel");
        let session = Session::new().with_persistence_path(path.clone());
        let mut runtime = runtime_with(session, api, tools).with_cancellation_token(token);

        let summary = runtime
            .run_turn("run both", None)
            .expect("cancellation is not an error");

        assert_eq!(
            summary.outcome,
            TurnOutcome::Cancelled {
                interrupted_tool_calls: 1
            }
        );
        let results = tool_results(runtime.session());
        assert_eq!(
            results[0],
            ("a".to_string(), "first ran".to_string(), false)
        );
        assert_eq!(
            results[1],
            ("b".to_string(), INTERRUPTED_TOOL_RESULT.to_string(), true)
        );
        let restored = Session::load_from_path(&path).expect("persisted session reloads");
        std::fs::remove_file(&path).expect("cleanup");
        assert_eq!(restored.messages, runtime.session().messages);
        assert_eq!(
            unanswered_tool_use_ids(&restored.messages),
            Vec::<String>::new()
        );
    }

    /// A UI closes each tool call's row from `tool_call_finished`: it hears calls that ran,
    /// were denied, or never started.
    #[test]
    fn the_executor_hears_every_tool_calls_final_result() {
        type Finished = Rc<RefCell<Vec<(String, String, String, bool)>>>;
        struct Recording {
            inner: StaticToolExecutor,
            finished: Finished,
        }
        impl ToolExecutor for Recording {
            fn execute(
                &mut self,
                tool_name: &str,
                input: &str,
            ) -> Result<String, super::ToolError> {
                self.inner.execute(tool_name, input)
            }
            fn tool_call_finished(&mut self, id: &str, name: &str, output: &str, is_error: bool) {
                self.finished.borrow_mut().push((
                    id.to_string(),
                    name.to_string(),
                    output.to_string(),
                    is_error,
                ));
            }
        }
        struct DenyOne;
        impl PermissionPrompter for DenyOne {
            fn decide(&mut self, request: &PermissionRequest) -> PermissionPromptDecision {
                if request.tool_name == "denied" {
                    PermissionPromptDecision::Deny {
                        reason: "not this one".to_string(),
                    }
                } else {
                    PermissionPromptDecision::Allow
                }
            }
        }

        let token = CancellationToken::new();
        let cancel_from_tool = token.clone();
        let api = ScriptedApi::new(vec![Ok(vec![
            tool_call("1", "ran", "{}"),
            tool_call("2", "denied", "{}"),
            tool_call("3", "stops", "{}"),
            tool_call("4", "never", "{}"),
            AssistantEvent::MessageStop,
        ])]);
        let finished = Finished::default();
        let tools = Recording {
            inner: StaticToolExecutor::new()
                .register("ran", |_| Ok("ok".to_string()))
                .register("stops", move |_| {
                    cancel_from_tool.cancel();
                    Ok("stopped".to_string())
                })
                .register("never", |_| panic!("cancelled tools must not run")),
            finished: Rc::clone(&finished),
        };
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            api,
            tools,
            PermissionPolicy::new(PermissionMode::WorkspaceWrite),
            vec!["system".to_string()],
        )
        .with_cancellation_token(token);

        runtime
            .run_turn("go", Some(&mut DenyOne))
            .expect("cancellation is not an error");

        let row = |id: &str, name: &str, output: &str, is_error| {
            (
                id.to_string(),
                name.to_string(),
                output.to_string(),
                is_error,
            )
        };
        assert_eq!(
            *finished.borrow(),
            vec![
                row("1", "ran", "ok", false),
                row("2", "denied", "not this one", true),
                row("3", "stops", "stopped", false),
                row("4", "never", INTERRUPTED_TOOL_RESULT, true),
            ]
        );
    }

    #[test]
    fn cancellation_during_model_call_returns_to_prompt_and_next_turn_works() {
        let token = CancellationToken::new();
        let cancel_from_api = token.clone();
        let mut api = ScriptedApi::new(vec![
            Err(RuntimeError::with_kind(
                RuntimeErrorKind::Cancelled,
                "request cancelled",
            )),
            Ok(vec![text("fresh answer"), AssistantEvent::MessageStop]),
        ]);
        api.on_call = Some(Box::new(move |call| {
            if call == 1 {
                cancel_from_api.cancel();
            }
        }));
        let mut runtime = runtime_with(Session::new(), api, StaticToolExecutor::new())
            .with_cancellation_token(token);

        let cancelled = runtime
            .run_turn("slow question", None)
            .expect("not an error");
        assert_eq!(
            cancelled.outcome,
            TurnOutcome::Cancelled {
                interrupted_tool_calls: 0
            }
        );
        // Nothing was streamed: the note alone answers the interrupted prompt.
        assert_eq!(
            roles_and_texts(runtime.session()),
            vec![
                (MessageRole::User, "slow question".to_string()),
                (
                    MessageRole::Assistant,
                    INTERRUPTED_RESPONSE_NOTE.to_string()
                ),
            ]
        );
        assert!(cancelled.assistant_messages.is_empty(), "no model output");

        // The REPL installs a fresh token for the next turn.
        runtime.set_cancellation_token(CancellationToken::new());
        let next = runtime.run_turn("try again", None).expect("next turn runs");
        assert_eq!(next.outcome, TurnOutcome::Completed);
        assert_eq!(runtime.session().messages.len(), 4);
        // The model never sees the lone note (it repeated it as its next answer), but it
        // still sees the interrupted prompt as a turn of its own, answered by a
        // placeholder; the new prompt is a message of its own.
        let next_request = &runtime.api_client().requests[1].messages;
        assert_eq!(
            next_request,
            &vec![
                ConversationMessage::user_text("slow question"),
                assistant_text(super::STOPPED_PROMPT_REPLY_FOR_MODEL),
                ConversationMessage::user_text("try again"),
            ]
        );
        assert_alternates(next_request);
        assert_no_turn_end_markers(next_request);
    }

    fn assistant_text(text: &str) -> ConversationMessage {
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: text.to_string(),
        }])
    }

    /// A refinement after an interrupt ("write it in Rust instead") must still see the
    /// request it refines, as the user's earlier turn.
    #[test]
    fn a_refinement_after_an_interrupt_keeps_the_first_prompt_as_a_prior_user_turn() {
        let token = CancellationToken::new();
        let cancel_from_api = token.clone();
        let mut api = ScriptedApi::new(vec![
            Err(RuntimeError::with_kind(
                RuntimeErrorKind::Cancelled,
                "request cancelled",
            )),
            Ok(vec![
                text("fn frobnicate_widget(s: &str) -> String { s.chars().rev().collect() }"),
                AssistantEvent::MessageStop,
            ]),
        ]);
        api.on_call = Some(Box::new(move |call| {
            if call == 1 {
                cancel_from_api.cancel();
            }
        }));
        let mut runtime = runtime_with(Session::new(), api, StaticToolExecutor::new())
            .with_cancellation_token(token);
        let first = "Write a Python function named frobnicate_widget that returns its string argument reversed.";
        let refinement = "Actually, write it in Rust instead.";

        runtime.run_turn(first, None).expect("not an error");
        runtime.set_cancellation_token(CancellationToken::new());
        runtime.run_turn(refinement, None).expect("next turn runs");

        let next_request = &runtime.api_client().requests[1].messages;
        assert_eq!(
            next_request,
            &vec![
                ConversationMessage::user_text(first),
                assistant_text(super::STOPPED_PROMPT_REPLY_FOR_MODEL),
                ConversationMessage::user_text(refinement),
            ]
        );
        assert_alternates(next_request);
        assert_no_turn_end_markers(next_request);
        // The session file keeps the runtime's own note.
        assert_eq!(
            roles_and_texts(runtime.session())[1],
            (
                MessageRole::Assistant,
                INTERRUPTED_RESPONSE_NOTE.to_string()
            )
        );
    }

    #[test]
    fn requests_keep_each_stopped_prompt_as_its_own_turn() {
        let note = || assistant_text(INTERRUPTED_RESPONSE_NOTE);
        let stopped = super::STOPPED_PROMPT_REPLY_FOR_MODEL;
        let partial = format!("Step one\n\n{INTERRUPTED_RESPONSE_NOTE}");
        let messages = vec![
            ConversationMessage::user_text("first, interrupted"),
            note(),
            ConversationMessage::user_text("second, interrupted too"),
            note(),
            ConversationMessage::user_text("third"),
            assistant_text("three"),
            ConversationMessage::user_text("fourth, cut short"),
            assistant_text(&partial),
            ConversationMessage::user_text("fifth"),
        ];

        let request = super::request_messages(&messages);

        assert_eq!(
            request,
            vec![
                ConversationMessage::user_text("first, interrupted"),
                assistant_text(stopped),
                ConversationMessage::user_text("second, interrupted too"),
                assistant_text(stopped),
                ConversationMessage::user_text("third"),
                assistant_text("three"),
                ConversationMessage::user_text("fourth, cut short"),
                assistant_text(&format!(
                    "Step one\n\n{}",
                    super::STOPPED_ANSWER_NOTE_FOR_MODEL
                )),
                ConversationMessage::user_text("fifth"),
            ]
        );
        assert_alternates(&request);
        assert_no_turn_end_markers(&request);
        // After a compaction summary too.
        let summary = ConversationMessage {
            role: MessageRole::System,
            blocks: vec![ContentBlock::Text {
                text: "summary".to_string(),
            }],
            usage: None,
        };
        let compacted = vec![
            summary.clone(),
            ConversationMessage::user_text("interrupted"),
            note(),
            ConversationMessage::user_text("next"),
        ];
        assert_eq!(
            super::request_messages(&compacted),
            vec![
                summary,
                ConversationMessage::user_text("interrupted"),
                assistant_text(stopped),
                ConversationMessage::user_text("next"),
            ]
        );
    }

    /// Every shape a session can close a turn with, then a new prompt: no request puts
    /// a note in a user message or shows the session's own markers.
    #[test]
    fn no_request_puts_a_note_in_a_user_message() {
        let tool_use = |id: &str| {
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: "bash".to_string(),
                input: "{}".to_string(),
            }])
        };
        let failed = format!(
            "{FAILED_RESPONSE_NOTE_PREFIX}api failed after 3 attempts: Shared free capacity is exhausted right now. (capacity_exhausted); retry in 3h 44m; request req_1]"
        );
        let messages = vec![
            ConversationMessage::user_text("Create cancelled.txt containing hello."),
            assistant_text(INTERRUPTED_RESPONSE_NOTE),
            ConversationMessage::user_text("What is 17 multiplied by 3?"),
            assistant_text(&failed),
            ConversationMessage::user_text("explain"),
            assistant_text(&format!("Partly\n\n{INTERRUPTED_RESPONSE_NOTE}")),
            ConversationMessage::user_text("run it"),
            tool_use("a"),
            ConversationMessage::tool_result("a", "bash", INTERRUPTED_TOOL_RESULT, true),
            assistant_text(INTERRUPTED_RESPONSE_NOTE),
            ConversationMessage::user_text("run it again"),
            tool_use("b"),
            ConversationMessage::tool_result("b", "bash", "ok", false),
            assistant_text(&failed),
            ConversationMessage::user_text("Unrelated: what is the capital of Japan?"),
        ];

        let request = super::request_messages(&messages);

        assert_alternates(&request);
        assert_no_turn_end_markers(&request);
        assert_eq!(
            request.last(),
            Some(&ConversationMessage::user_text(
                "Unrelated: what is the capital of Japan?"
            ))
        );
        assert_eq!(
            request[3],
            assistant_text(
                "(no answer: this request failed: api failed after 3 attempts: Shared free capacity is exhausted right now. (capacity_exhausted))"
            )
        );
        // Every prompt is a user message of its own, as typed.
        let prompts = request
            .iter()
            .filter(|message| super::is_user_prompt(message))
            .cloned()
            .collect::<Vec<_>>();
        let typed = messages
            .iter()
            .filter(|message| super::is_user_prompt(message))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(prompts, typed);
    }

    #[test]
    fn a_failed_turns_error_is_shortened_for_the_model() {
        assert_eq!(
            super::short_error(
                "Shared free capacity is exhausted right now. (capacity_exhausted); retry in 3h 44m; upgrade: https://zero-x.live/pricing"
            ),
            "Shared free capacity is exhausted right now. (capacity_exhausted)"
        );
        let long = "word ".repeat(100);
        let short = super::short_error(&long);
        assert!(short.ends_with("word\u{2026}"), "{short}");
        assert!(
            short.chars().count() <= super::FAILED_PROMPT_REPLY_ERROR_MAX_CHARS + 1,
            "{short}"
        );
        // The session's own excerpt mark is not doubled.
        assert_eq!(
            super::short_error("upstream failed\u{2026}"),
            "upstream failed"
        );
    }

    /// No message in `request` carries a note the runtime wrote to close a turn (models
    /// repeat those as their own answer), and no user message carries a note from
    /// Neuron: the user's messages hold what the user typed.
    fn assert_no_turn_end_markers(request: &[ConversationMessage]) {
        for message in request {
            for block in &message.blocks {
                if let ContentBlock::Text { text } = block {
                    assert!(
                        !text.contains(INTERRUPTED_RESPONSE_NOTE)
                            && !text.contains(FAILED_RESPONSE_NOTE_PREFIX),
                        "{request:?}"
                    );
                    assert!(
                        message.role != MessageRole::User || !text.contains("Note from Neuron"),
                        "{request:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn an_interrupted_turn_that_ran_tools_ends_with_an_out_of_band_note() {
        let token = CancellationToken::new();
        let cancel_from_tool = token.clone();
        let api = ScriptedApi::new(vec![
            Ok(vec![
                tool_call("a", "slow", "{}"),
                AssistantEvent::MessageStop,
            ]),
            Ok(vec![text("4"), AssistantEvent::MessageStop]),
        ]);
        let tools = StaticToolExecutor::new().register("slow", move |_| {
            cancel_from_tool.cancel();
            Ok("stopped early".to_string())
        });
        let mut runtime = runtime_with(Session::new(), api, tools).with_cancellation_token(token);

        let cancelled = runtime.run_turn("run it", None).expect("not an error");
        assert!(matches!(cancelled.outcome, TurnOutcome::Cancelled { .. }));
        // The session keeps the runtime's note; the summary has no answer text.
        let last = runtime.session().messages.last().expect("messages");
        assert!(matches!(
            last.blocks.as_slice(),
            [ContentBlock::Text { text }] if text == INTERRUPTED_RESPONSE_NOTE
        ));
        assert!(cancelled.assistant_messages.iter().all(|message| message
            .blocks
            .iter()
            .all(|block| !matches!(block, ContentBlock::Text { .. }))));

        runtime.set_cancellation_token(CancellationToken::new());
        runtime
            .run_turn("what is 2 plus 2?", None)
            .expect("next turn runs");
        let next_request = &runtime.api_client().requests[1].messages;
        // The tool call ran, so the turn stays; its close reads as a note, not an answer.
        assert_eq!(next_request.len(), 5, "{next_request:?}");
        assert_eq!(next_request[0], ConversationMessage::user_text("run it"));
        assert!(matches!(
            next_request[3].blocks.as_slice(),
            [ContentBlock::Text { text }] if text == super::INTERRUPTED_TURN_NOTE_FOR_MODEL
        ));
        assert!(!next_request.iter().any(|message| message
            .blocks
            .iter()
            .any(|block| matches!(block, ContentBlock::Text { text } if text.contains(INTERRUPTED_RESPONSE_NOTE)))));
        assert_eq!(next_request[4].role, MessageRole::User);
    }

    /// (role, text) of every message with text.
    fn roles_and_texts(session: &Session) -> Vec<(MessageRole, String)> {
        session
            .messages
            .iter()
            .filter_map(|message| {
                let text = message
                    .blocks
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>();
                (!text.is_empty()).then_some((message.role, text))
            })
            .collect()
    }

    /// No two user prompts follow each other without an assistant reply between them
    /// (tool results are on the user's side).
    fn assert_alternates(messages: &[ConversationMessage]) {
        let prompts = messages
            .iter()
            .filter_map(|message| match message.role {
                MessageRole::User | MessageRole::Tool => Some(MessageRole::User),
                MessageRole::Assistant => Some(MessageRole::Assistant),
                MessageRole::System => None,
            })
            .collect::<Vec<_>>();
        for pair in prompts.windows(2) {
            assert_ne!(pair[0], pair[1], "{messages:?}");
        }
    }

    #[test]
    fn an_interrupted_stream_keeps_the_partial_answer_for_the_next_turn() {
        let token = CancellationToken::new();
        let cancel_from_api = token.clone();
        let mut api = ScriptedApi::new(vec![
            Err(
                RuntimeError::with_kind(RuntimeErrorKind::Cancelled, "request cancelled")
                    .with_partial_text("Step one: open the file. Step"),
            ),
            Ok(vec![text("done"), AssistantEvent::MessageStop]),
        ]);
        api.on_call = Some(Box::new(move |call| {
            if call == 1 {
                cancel_from_api.cancel();
            }
        }));
        let path = temp_session_path("partial");
        let session = Session::new().with_persistence_path(path.clone());
        let mut runtime =
            runtime_with(session, api, StaticToolExecutor::new()).with_cancellation_token(token);

        let cancelled = runtime.run_turn("explain", None).expect("not an error");

        let kept = format!("Step one: open the file. Step\n\n{INTERRUPTED_RESPONSE_NOTE}");
        assert_eq!(
            roles_and_texts(runtime.session()),
            vec![
                (MessageRole::User, "explain".to_string()),
                (MessageRole::Assistant, kept.clone()),
            ]
        );
        assert_eq!(cancelled.assistant_messages.len(), 1);
        let restored = Session::load_from_path(&path).expect("persisted session reloads");
        std::fs::remove_file(&path).expect("cleanup");
        assert_eq!(restored.messages, runtime.session().messages);

        runtime.set_cancellation_token(CancellationToken::new());
        runtime.run_turn("go on", None).expect("next turn runs");
        // The partial answer reaches the model ending with a short past-tense note
        // instead of the session's marker; the next prompt is as typed.
        let next_request = &runtime.api_client().requests[1].messages;
        assert_eq!(
            next_request,
            &vec![
                ConversationMessage::user_text("explain"),
                assistant_text(&format!(
                    "Step one: open the file. Step\n\n{}",
                    super::STOPPED_ANSWER_NOTE_FOR_MODEL
                )),
                ConversationMessage::user_text("go on"),
            ]
        );
        assert_alternates(next_request);
        assert_no_turn_end_markers(next_request);
    }

    #[test]
    fn a_turn_that_fails_before_any_output_does_not_leave_its_prompt_unanswered() {
        let api = ScriptedApi::new(vec![
            Err(RuntimeError::new("gateway returned 503\nretry later")),
            Ok(vec![text("hello"), AssistantEvent::MessageStop]),
        ]);
        let mut runtime = runtime_with(Session::new(), api, StaticToolExecutor::new());

        runtime
            .run_turn("first request", None)
            .expect_err("the provider failed");

        let messages = roles_and_texts(runtime.session());
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert_eq!(messages[1].0, MessageRole::Assistant);
        assert_eq!(
            messages[1].1,
            format!("{FAILED_RESPONSE_NOTE_PREFIX}gateway returned 503]")
        );
        assert!(super::is_turn_end_note(&messages[1].1));

        runtime
            .run_turn("second request", None)
            .expect("next turn runs");
        let next_request = &runtime.api_client().requests[1].messages;
        assert_alternates(next_request);
        assert_no_turn_end_markers(next_request);
    }

    /// After a failure (the gateway's capacity ran out, say) and a switch to another
    /// model, the next model must not repeat the failure note as its answer, must
    /// answer the new, unrelated question rather than the failed one, and "try again"
    /// must still know what to try.
    #[test]
    fn an_unrelated_question_after_a_failure_is_its_own_last_user_message() {
        let api = ScriptedApi::new(vec![
            Err(RuntimeError::new(
                "Shared free capacity is exhausted right now. (capacity_exhausted); retry in 3h 44m; upgrade: https://zero-x.live/pricing",
            )),
            Ok(vec![text("Tokyo"), AssistantEvent::MessageStop]),
        ]);
        let mut runtime = runtime_with(Session::new(), api, StaticToolExecutor::new());
        runtime
            .run_turn("Write an essay on the Byzantine Empire.", None)
            .expect_err("the provider failed");

        runtime
            .run_turn("What is the capital of Japan?", None)
            .expect("next turn runs");

        let next_request = &runtime.api_client().requests[1].messages;
        assert_eq!(
            next_request,
            &vec![
                ConversationMessage::user_text("Write an essay on the Byzantine Empire."),
                assistant_text(
                    "(no answer: this request failed: Shared free capacity is exhausted right now. (capacity_exhausted))"
                ),
                ConversationMessage::user_text("What is the capital of Japan?"),
            ]
        );
        assert_alternates(next_request);
        assert_no_turn_end_markers(next_request);
        // The session keeps the note, for the transcript and resume.
        assert!(runtime.session().messages.iter().any(|message| {
            message.role == MessageRole::Assistant
                && matches!(message.blocks.as_slice(), [ContentBlock::Text { text }] if text.starts_with(FAILED_RESPONSE_NOTE_PREFIX))
        }));
    }

    #[test]
    fn a_failure_after_tool_results_reaches_the_model_as_an_out_of_band_note() {
        let messages = vec![
            ConversationMessage::user_text("run it"),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "a".to_string(),
                name: "echo".to_string(),
                input: "{}".to_string(),
            }]),
            ConversationMessage::tool_result("a", "echo", "ok", false),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: format!("{FAILED_RESPONSE_NOTE_PREFIX}upstream failed]"),
            }]),
            ConversationMessage::user_text("try again"),
        ];

        let request = super::request_messages(&messages);

        assert_eq!(request.len(), 5, "{request:?}");
        assert_eq!(
            request[3],
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "(Note from Neuron, not a reply: this turn failed before an answer was written. The error: upstream failed)".to_string(),
            }])
        );
        assert_eq!(request[4], ConversationMessage::user_text("try again"));
        assert_no_turn_end_markers(&request);
    }

    #[test]
    fn a_turn_that_fails_after_tool_results_ends_with_the_failure_note() {
        let api = ScriptedApi::new(vec![
            Ok(vec![
                tool_call("a", "echo", "{}"),
                AssistantEvent::MessageStop,
            ]),
            Err(RuntimeError::new("upstream failed")),
        ]);
        let tools = StaticToolExecutor::new().register("echo", |_| Ok("ok".to_string()));
        let mut runtime = runtime_with(Session::new(), api, tools);

        runtime
            .run_turn("run it", None)
            .expect_err("second call fails");

        let last = runtime.session().messages.last().expect("messages");
        assert_eq!(last.role, MessageRole::Assistant);
        assert!(matches!(
            last.blocks.as_slice(),
            [ContentBlock::Text { text }] if text.starts_with(FAILED_RESPONSE_NOTE_PREFIX)
        ));
        assert_eq!(
            unanswered_tool_use_ids(&runtime.session().messages),
            Vec::<String>::new()
        );
    }

    #[test]
    fn cancel_noticed_after_response_skips_requested_tools() {
        let token = CancellationToken::new();
        let cancel_from_api = token.clone();
        let mut api = ScriptedApi::new(vec![Ok(vec![
            tool_call("a", "write_file", r#"{"path":"x"}"#),
            AssistantEvent::MessageStop,
        ])]);
        // A client that ignores the token: the response still arrives.
        api.on_call = Some(Box::new(move |_| cancel_from_api.cancel()));
        let tools = StaticToolExecutor::new().register("write_file", |_| panic!("must not run"));
        let mut runtime = runtime_with(Session::new(), api, tools).with_cancellation_token(token);

        let summary = runtime.run_turn("write", None).expect("not an error");

        assert_eq!(
            summary.outcome,
            TurnOutcome::Cancelled {
                interrupted_tool_calls: 1
            }
        );
        assert_eq!(
            unanswered_tool_use_ids(&runtime.session().messages),
            Vec::<String>::new()
        );
    }

    fn long_session(exchanges: usize) -> Session {
        let mut session = Session::new();
        for index in 0..exchanges {
            session
                .push_user_text(format!("question {index}: {}", "context ".repeat(400)))
                .expect("push");
            session
                .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: format!("answer {index}: {}", "detail ".repeat(400)),
                }]))
                .expect("push");
        }
        session
    }

    fn overflow_error() -> RuntimeError {
        RuntimeError::with_kind(
            RuntimeErrorKind::ContextWindowExceeded,
            "context_length_exceeded: prompt is too long",
        )
    }

    #[test]
    fn provider_context_overflow_shrinks_the_context_and_retries_once() {
        let api = ScriptedApi::new(vec![
            Err(overflow_error()),
            Ok(vec![text("fits now"), AssistantEvent::MessageStop]),
        ]);
        let mut runtime = runtime_with(long_session(12), api, StaticToolExecutor::new());

        let summary = runtime
            .run_turn("latest request", None)
            .expect("one overflow is recovered");

        assert_eq!(summary.outcome, TurnOutcome::Completed);
        let requests = &runtime.api_client().requests;
        assert_eq!(requests.len(), 2);
        let before = crate::context_manager::estimate_messages_tokens(&requests[0].messages);
        let after = crate::context_manager::estimate_messages_tokens(&requests[1].messages);
        assert!(after * 2 <= before, "before={before} after={after}");
        assert!(summary.auto_compaction.is_some());
        // The latest request survives compaction verbatim.
        assert!(requests[1].messages.iter().any(|message| message
            .blocks
            .iter()
            .any(|block| matches!(block, ContentBlock::Text { text } if text.contains("latest request")))));
    }

    #[test]
    fn repeated_context_overflow_is_reported_instead_of_looping() {
        let api = ScriptedApi::new(vec![Err(overflow_error()), Err(overflow_error())]);
        let mut runtime = runtime_with(long_session(12), api, StaticToolExecutor::new());

        let error = runtime
            .run_turn("latest request", None)
            .expect_err("a second overflow ends the turn");

        assert_eq!(error.kind(), RuntimeErrorKind::ContextWindowExceeded);
        assert_eq!(runtime.api_client().requests.len(), 2);
    }

    #[test]
    fn ctrl_c_during_the_permission_prompt_never_starts_the_tool() {
        struct CancelWhileAsking {
            cancel: CancellationToken,
        }
        impl PermissionPrompter for CancelWhileAsking {
            fn decide(&mut self, _request: &PermissionRequest) -> PermissionPromptDecision {
                // The user presses Ctrl-C while the prompt waits, then the
                // prompt returns "allow" anyway.
                self.cancel.cancel();
                PermissionPromptDecision::Allow
            }
        }
        let token = CancellationToken::new();
        let api = ScriptedApi::new(vec![Ok(vec![
            tool_call("a", "bash", r#"{"command":"rm -rf build"}"#),
            tool_call("b", "bash", r#"{"command":"ls"}"#),
            AssistantEvent::MessageStop,
        ])]);
        let tools = StaticToolExecutor::new().register("bash", |_| panic!("must not run"));
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            api,
            tools,
            PermissionPolicy::new(PermissionMode::WorkspaceWrite)
                .with_tool_requirement("bash", PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        )
        .with_cancellation_token(token.clone());
        let mut prompter = CancelWhileAsking { cancel: token };

        let summary = runtime
            .run_turn("clean up", Some(&mut prompter))
            .expect("cancellation is not an error");

        assert_eq!(
            summary.outcome,
            TurnOutcome::Cancelled {
                interrupted_tool_calls: 2
            }
        );
        assert_eq!(
            tool_results(runtime.session()),
            vec![
                ("a".to_string(), INTERRUPTED_TOOL_RESULT.to_string(), true),
                ("b".to_string(), INTERRUPTED_TOOL_RESULT.to_string(), true),
            ]
        );
    }

    #[test]
    fn doom_loop_guard_warns_once_then_stops_the_turn() {
        struct RepeatingApi {
            calls: usize,
        }
        impl ApiClient for RepeatingApi {
            fn stream(&mut self, _request: ApiRequest) -> Response {
                self.calls += 1;
                Ok(vec![
                    tool_call(&format!("call-{}", self.calls), "echo", r#"{"q": 1}"#),
                    AssistantEvent::MessageStop,
                ])
            }
        }
        let executions = Rc::new(RefCell::new(0_usize));
        let counter = Rc::clone(&executions);
        let tools = StaticToolExecutor::new().register("echo", move |_| {
            *counter.borrow_mut() += 1;
            Ok("same output".to_string())
        });
        let mut runtime = runtime_with(Session::new(), RepeatingApi { calls: 0 }, tools);

        let summary = runtime.run_turn("loop", None).expect("guard stops cleanly");

        assert_eq!(
            summary.outcome,
            TurnOutcome::LoopDetected {
                tool_name: "echo".to_string(),
                repeats: 4
            }
        );
        assert_eq!(
            *executions.borrow(),
            3,
            "the fourth identical call is not run"
        );
        assert_eq!(runtime.api_client().calls, 4);
        let results = tool_results(runtime.session());
        assert!(!results[1].1.contains("[loop guard]"));
        assert!(results[2].1.contains("[loop guard]") && !results[2].2);
        assert!(results[3].1.contains("Not executed") && results[3].2);
    }

    #[test]
    fn iteration_cap_defaults_to_fifty_and_stops_with_consistent_transcript() {
        struct EndlessApi {
            calls: usize,
        }
        impl ApiClient for EndlessApi {
            fn stream(&mut self, _request: ApiRequest) -> Response {
                self.calls += 1;
                // Distinct arguments so the loop guard does not fire.
                Ok(vec![
                    tool_call(
                        &format!("call-{}", self.calls),
                        "echo",
                        &format!(r#"{{"n":{}}}"#, self.calls),
                    ),
                    AssistantEvent::MessageStop,
                ])
            }
        }
        let tools = StaticToolExecutor::new().register("echo", |input| Ok(input.to_string()));
        let runtime = runtime_with(Session::new(), EndlessApi { calls: 0 }, tools);
        assert_eq!(runtime.max_iterations(), DEFAULT_MAX_ITERATIONS);
        assert_eq!(DEFAULT_MAX_ITERATIONS, 50);

        let mut runtime = runtime.with_max_iterations(5);
        let error = runtime
            .run_turn("never ends", None)
            .expect_err("iteration cap must stop the turn");

        assert_eq!(error.kind(), RuntimeErrorKind::MaxIterations);
        assert!(error.to_string().contains("(5)"));
        // Both knobs that change the cap are named.
        assert!(error.to_string().contains(MAX_ITERATIONS_ENV_VAR));
        assert!(error.to_string().contains("\"maxIterations\" setting"));
        assert_eq!(runtime.api_client().calls, 5);
        assert_eq!(
            unanswered_tool_use_ids(&runtime.session().messages),
            Vec::<String>::new()
        );
    }

    #[test]
    fn max_iterations_resolution_prefers_env_then_setting_then_default() {
        assert_eq!(parse_max_iterations(None, None), DEFAULT_MAX_ITERATIONS);
        assert_eq!(parse_max_iterations(Some("7"), Some(9)), 7);
        assert_eq!(parse_max_iterations(Some(" 0 "), Some(9)), 9);
        assert_eq!(
            parse_max_iterations(Some("many"), None),
            DEFAULT_MAX_ITERATIONS
        );
        assert_eq!(parse_max_iterations(None, Some(0)), DEFAULT_MAX_ITERATIONS);
        assert_eq!(parse_max_iterations(None, Some(120)), 120);
    }

    #[test]
    fn truncated_tool_call_is_not_executed_and_model_is_told_to_split() {
        let api = ScriptedApi::new(vec![
            Ok(vec![
                tool_call(
                    "w",
                    "write_file",
                    r#"{"path":"big.rs","content":"fn main() {"#,
                ),
                AssistantEvent::StopReason("max_tokens".to_string()),
                AssistantEvent::MessageStop,
            ]),
            Ok(vec![text("will split it"), AssistantEvent::MessageStop]),
        ]);
        let tools = StaticToolExecutor::new()
            .register("write_file", |_| panic!("partial JSON must not run"));
        let mut runtime = runtime_with(Session::new(), api, tools);

        let summary = runtime
            .run_turn("write a big file", None)
            .expect("turn completes");

        assert_eq!(summary.outcome, TurnOutcome::Completed);
        let results = tool_results(runtime.session());
        assert!(results[0].2);
        assert!(results[0].1.contains("NOT executed"));
        assert!(results[0].1.contains("smaller"));
        assert_eq!(runtime.api_client().requests.len(), 2);
    }

    #[test]
    fn truncated_text_gets_one_automatic_continue_request() {
        let api = ScriptedApi::new(vec![
            Ok(vec![
                text("part one"),
                AssistantEvent::StopReason("length".to_string()),
                AssistantEvent::MessageStop,
            ]),
            Ok(vec![
                text("part two"),
                AssistantEvent::StopReason("end_turn".to_string()),
                AssistantEvent::MessageStop,
            ]),
        ]);
        let mut runtime = runtime_with(Session::new(), api, StaticToolExecutor::new());

        let summary = runtime
            .run_turn("long answer", None)
            .expect("turn completes");

        assert_eq!(summary.outcome, TurnOutcome::Completed);
        assert_eq!(summary.assistant_messages.len(), 2);
        assert_eq!(summary.stop_reason.as_deref(), Some("end_turn"));
        let continue_prompt = &runtime.api_client().requests[1]
            .messages
            .last()
            .expect("continue prompt")
            .blocks[0];
        assert!(matches!(
            continue_prompt,
            ContentBlock::Text { text } if text.starts_with(AUTO_CONTINUE_PREFIX)
        ));

        let twice = ScriptedApi::new(vec![
            Ok(vec![
                text("a"),
                AssistantEvent::StopReason("max_tokens".to_string()),
                AssistantEvent::MessageStop,
            ]),
            Ok(vec![
                text("b"),
                AssistantEvent::StopReason("max_tokens".to_string()),
                AssistantEvent::MessageStop,
            ]),
        ]);
        let mut runtime = runtime_with(Session::new(), twice, StaticToolExecutor::new());
        let summary = runtime
            .run_turn("long answer", None)
            .expect("turn completes");
        assert_eq!(summary.outcome, TurnOutcome::OutputTruncated);
        assert_eq!(runtime.api_client().requests.len(), 2);
    }

    #[test]
    fn no_tool_call_of_a_cut_off_response_runs_even_with_complete_arguments() {
        let api = ScriptedApi::new(vec![
            Ok(vec![
                tool_call("r", "read_file", r#"{"path":"notes.txt"}"#),
                tool_call("w", "write_file", r#"{"path":"out.txt","content":"par"#),
                AssistantEvent::StopReason("length".to_string()),
                AssistantEvent::MessageStop,
            ]),
            Ok(vec![
                text("made the calls again"),
                AssistantEvent::MessageStop,
            ]),
        ]);
        let tools = StaticToolExecutor::new()
            .register("read_file", |_| {
                panic!("a cut-off response's call must not run")
            })
            .register("write_file", |_| panic!("partial JSON must not run"));
        let mut runtime = runtime_with(Session::new(), api, tools);

        let summary = runtime
            .run_turn("read then write", None)
            .expect("turn completes");

        assert_eq!(summary.outcome, TurnOutcome::Completed);
        let results = tool_results(runtime.session());
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|(_, _, is_error)| *is_error));
        assert!(
            results[0]
                .1
                .contains("no tool call of a response that was cut off is run"),
            "{}",
            results[0].1
        );
        assert!(results[1].1.contains("smaller"), "{}", results[1].1);
        assert_eq!(runtime.api_client().requests.len(), 2);
    }

    const REFUSAL_NOTE: &str =
        "Declined (safety category: cyber). Rephrase it or choose another model.";

    #[test]
    fn a_refusal_mid_text_keeps_the_text_and_ends_the_turn() {
        let api = ScriptedApi::new(vec![
            Ok(vec![
                text("Here is how it works. First,"),
                AssistantEvent::StopReason(REFUSAL_STOP_REASON.to_string()),
                AssistantEvent::Refusal(REFUSAL_NOTE.to_string()),
                AssistantEvent::MessageStop,
            ]),
            Ok(vec![text("Sure."), AssistantEvent::MessageStop]),
        ]);
        let mut runtime = runtime_with(Session::new(), api, StaticToolExecutor::new());

        let summary = runtime
            .run_turn("explain", None)
            .expect("a refusal is no error");

        assert_eq!(
            summary.outcome,
            TurnOutcome::Refused {
                note: Some(REFUSAL_NOTE.to_string()),
                skipped_tool_calls: 0,
            }
        );
        assert_eq!(summary.stop_reason.as_deref(), Some("refusal"));
        assert_eq!(summary.iterations, 1, "a refused turn is not continued");
        assert_eq!(summary.assistant_messages.len(), 1);
        let last = runtime.session().messages.last().expect("messages");
        assert_eq!(
            last.blocks,
            vec![ContentBlock::Text {
                text: "Here is how it works. First,".to_string()
            }],
            "the partial answer closes the turn; no note is added after it"
        );

        runtime.run_turn("something else", None).expect("next turn");
        let request = &runtime.api_client().requests[1];
        assert!(request
            .messages
            .iter()
            .any(|message| message.role == MessageRole::Assistant
                && message.blocks
                    == vec![ContentBlock::Text {
                        text: "Here is how it works. First,".to_string()
                    }]));
    }

    #[test]
    fn a_refusal_before_any_text_is_an_outcome_not_an_empty_reply() {
        let api = ScriptedApi::new(vec![
            Ok(vec![
                AssistantEvent::Usage(TokenUsage {
                    input_tokens: 120,
                    output_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                }),
                AssistantEvent::StopReason(REFUSAL_STOP_REASON.to_string()),
                AssistantEvent::Refusal(format!("  {REFUSAL_NOTE}\n")),
                AssistantEvent::MessageStop,
            ]),
            Ok(vec![text("Here you go."), AssistantEvent::MessageStop]),
        ]);
        let mut runtime = runtime_with(Session::new(), api, StaticToolExecutor::new());

        let summary = runtime
            .run_turn("do the thing", None)
            .expect("a refusal before any text is not 'no content'");

        assert_eq!(
            summary.outcome,
            TurnOutcome::Refused {
                note: Some(REFUSAL_NOTE.to_string()),
                skipped_tool_calls: 0,
            }
        );
        assert_eq!(
            summary.assistant_messages,
            Vec::<ConversationMessage>::new()
        );
        assert_eq!(summary.usage.input_tokens, 120);
        // The prompt is answered by a note, so the next prompt is not taken for it.
        let ContentBlock::Text { text: note } =
            &runtime.session().messages.last().expect("note").blocks[0]
        else {
            panic!("expected the closing note");
        };
        assert!(is_turn_end_note(note), "{note}");
        assert!(note.contains("declined"), "{note}");

        runtime.run_turn("something else", None).expect("next turn");
        let request = &runtime.api_client().requests[1];
        let replies = request
            .messages
            .iter()
            .filter(|message| message.role == MessageRole::Assistant)
            .flat_map(|message| message.blocks.iter())
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            replies,
            ["(no answer: this request failed: the model declined to continue this request)"]
        );
    }

    #[test]
    fn tool_calls_of_a_refused_response_are_reported_not_run() {
        let api = ScriptedApi::new(vec![Ok(vec![
            text("Let me look."),
            tool_call("r", "read_file", r#"{"path":"a.txt"}"#),
            // Un-normalized, as a provider without the client's mapping says it.
            AssistantEvent::StopReason("content_filter".to_string()),
            AssistantEvent::MessageStop,
        ])]);
        let tools = StaticToolExecutor::new().register("read_file", |_| {
            panic!("a refused response's call must not run")
        });
        let mut runtime = runtime_with(Session::new(), api, tools);

        let summary = runtime.run_turn("read a.txt", None).expect("turn ends");

        assert_eq!(
            summary.outcome,
            TurnOutcome::Refused {
                note: None,
                skipped_tool_calls: 1,
            }
        );
        let results = tool_results(runtime.session());
        assert_eq!(results.len(), 1);
        assert!(results[0].2);
        assert!(results[0].1.contains("declined"), "{}", results[0].1);
        assert_eq!(runtime.api_client().requests.len(), 1);
        // Tool results do not end the transcript: a note closes the turn.
        let last = runtime.session().messages.last().expect("messages");
        assert_eq!(last.role, MessageRole::Assistant);
        assert_eq!(
            unanswered_tool_use_ids(&runtime.session().messages),
            Vec::<String>::new()
        );
    }

    #[test]
    fn unparseable_tool_arguments_become_a_tool_error_for_the_model() {
        let api = ScriptedApi::new(vec![
            Ok(vec![
                tool_call("x", "read_file", r#"{"path":"src/main.rs","limit":"#),
                tool_call("s", "read_file", r#""{\"path\":\"a\"""#),
                tool_call("e", "read_file", ""),
                AssistantEvent::StopReason("tool_use".to_string()),
                AssistantEvent::MessageStop,
            ]),
            Ok(vec![text("retried"), AssistantEvent::MessageStop]),
        ]);
        let tools = StaticToolExecutor::new().register("read_file", |_| {
            panic!("bad arguments must never reach the tool")
        });
        let mut runtime = runtime_with(Session::new(), api, tools);

        let summary = runtime.run_turn("read", None).expect("no crash, no panic");

        assert_eq!(summary.outcome, TurnOutcome::Completed);
        let results = tool_results(runtime.session());
        assert_eq!(results.len(), 3);
        for (_, output, is_error) in &results {
            assert!(*is_error);
            assert!(output.contains("NOT executed"), "{output}");
            assert!(output.contains("not a valid JSON object"), "{output}");
        }
        assert!(results[0].1.contains("cut off"), "{}", results[0].1);
        assert!(results[1].1.contains("a JSON string"), "{}", results[1].1);
        assert!(results[2].1.contains("empty"), "{}", results[2].1);
        assert_eq!(
            runtime.api_client().requests.len(),
            2,
            "the model hears about it and goes on"
        );
    }

    #[test]
    fn huge_tool_output_is_capped_when_inserted() {
        let api = ScriptedApi::new(vec![
            Ok(vec![
                tool_call("r", "read_file", r#"{"path":"huge.log"}"#),
                AssistantEvent::MessageStop,
            ]),
            Ok(vec![text("read it"), AssistantEvent::MessageStop]),
        ]);
        let tools = StaticToolExecutor::new().register("read_file", |_| {
            Ok(format!("start{}end", "y".repeat(500_000)))
        });
        let mut runtime = runtime_with(Session::new(), api, tools);

        runtime.run_turn("read", None).expect("turn completes");

        let (_, output, _) = tool_results(runtime.session()).remove(0);
        assert!(output.chars().count() <= 40_000);
        assert!(output.starts_with("start") && output.ends_with("end"));
        assert!(output.contains("chars omitted"));
    }

    #[test]
    fn context_is_checked_before_every_model_call_inside_the_tool_loop() {
        let api = ScriptedApi::new(vec![
            Ok(vec![
                tool_call("a", "read_file", r#"{"path":"a"}"#),
                AssistantEvent::MessageStop,
            ]),
            Ok(vec![
                tool_call("b", "read_file", r#"{"path":"b"}"#),
                AssistantEvent::MessageStop,
            ]),
            Ok(vec![text("done"), AssistantEvent::MessageStop]),
        ]);
        let tools = StaticToolExecutor::new().register("read_file", |_| Ok("z".repeat(36_000)));
        let manager = ContextManager::new(
            ContextBudget {
                context_window: 20_000,
                output_reserve: 2_000,
                tool_schema_tokens: 0,
            },
            ContextPolicy {
                keep_recent_tool_results: 1,
                ..ContextPolicy::default()
            },
        );
        let mut runtime = runtime_with(Session::new(), api, tools).with_context_manager(manager);

        let summary = runtime.run_turn("read both", None).expect("turn completes");

        assert_eq!(summary.pruned_tool_results, 1);
        assert_eq!(summary.auto_compaction, None::<AutoCompactionEvent>);
        let third_request = &runtime.api_client().requests[2];
        let outputs = third_request
            .messages
            .iter()
            .flat_map(|message| message.blocks.iter())
            .filter_map(|block| match block {
                ContentBlock::ToolResult { output, .. } => Some(output.chars().count()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(outputs[0] <= 2_000, "older result pruned: {outputs:?}");
        assert_eq!(outputs[1], 36_000, "most recent result intact");
    }

    #[test]
    fn dangling_tool_use_from_a_previous_failure_is_repaired_before_the_next_request() {
        let mut session = Session::new();
        session.messages = vec![
            ConversationMessage::user_text("earlier"),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "dangling".to_string(),
                name: "bash".to_string(),
                input: "{}".to_string(),
            }]),
        ];
        let api = ScriptedApi::new(vec![Ok(vec![text("ok"), AssistantEvent::MessageStop])]);
        let mut runtime = runtime_with(session, api, StaticToolExecutor::new());

        runtime.run_turn("next", None).expect("turn completes");

        let roles = runtime.api_client().requests[0]
            .messages
            .iter()
            .map(|message| message.role)
            .collect::<Vec<_>>();
        assert_eq!(
            roles,
            vec![
                MessageRole::User,
                MessageRole::Assistant,
                MessageRole::Tool,
                MessageRole::User
            ]
        );
    }
}
