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
use crate::session::{ContentBlock, ConversationMessage, Session};
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
/// Identical consecutive tool calls tolerated before the loop guard warns.
const LOOP_GUARD_THRESHOLD: usize = 3;
const MIN_MANUAL_SUMMARY_TOKENS: usize = 800;
const TRUNCATED_STOP_REASONS: &[&str] = &["max_tokens", "length", "model_context_window_exceeded"];
const TRUNCATED_TOOL_CALL_MESSAGE: &str = "This tool call was NOT executed: your response hit the output token limit (max_tokens) while writing it, so its arguments are incomplete JSON. Split the work into smaller steps — for example write or edit the file in several smaller edit_file calls instead of one large write — and try again.";
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
    /// Provider stop reason (`end_turn`, `tool_use`, `max_tokens`, `length`, ...).
    StopReason(String),
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
        }
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
    /// failures and the iteration cap return `Err`.
    #[allow(clippy::too_many_lines)]
    pub fn run_turn(
        &mut self,
        user_input: impl Into<String>,
        mut prompter: Option<&mut dyn PermissionPrompter>,
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
                        "conversation loop exceeded the maximum number of iterations ({}); stopping this turn. Raise the limit with the {MAX_ITERATIONS_ENV_VAR} environment variable (for example {MAX_ITERATIONS_ENV_VAR}=100).",
                        self.max_iterations
                    ),
                );
                self.record_turn_failed(progress.iterations, &error);
                return Err(error);
            }

            self.prepare_context(&mut progress)?;

            let request = ApiRequest {
                system_prompt: self.system_prompt.clone(),
                messages: normalized_messages(&self.session.messages),
            };
            let events = match self
                .api_client
                .stream_with_cancellation(request, &self.cancel)
            {
                Ok(events) => events,
                Err(error) if error.is_cancelled() || self.cancel.is_cancelled() => {
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

                if truncated && !is_complete_json(input) {
                    let result = ConversationMessage::tool_result(
                        tool_use_id.clone(),
                        tool_name.clone(),
                        TRUNCATED_TOOL_CALL_MESSAGE,
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
        progress.tool_results.push(result_message);
        Ok(())
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
/// The CLI passes the `maxIterations` settings key as `configured`. Settings
/// validation does not accept that key yet, so until it does the environment
/// variable is the only documented way to change the cap.
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

fn is_complete_json(input: &str) -> bool {
    serde_json::from_str::<Value>(input).is_ok()
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
    message: ConversationMessage,
    usage: Option<TokenUsage>,
    prompt_cache_events: Vec<PromptCacheEvent>,
    stop_reason: Option<String>,
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
    if blocks.is_empty() {
        return Err(RuntimeError::new("assistant stream produced no content"));
    }

    Ok(BuiltAssistantMessage {
        message: ConversationMessage::assistant_with_usage(blocks, usage),
        usage,
        prompt_cache_events,
        stop_reason,
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
                            input: "2,2".to_string(),
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
            let total = input
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
                        input: "secret".to_string(),
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
                        input: "payload".to_string(),
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
        parse_max_iterations, ApiClient, ApiRequest, AssistantEvent, AutoCompactionEvent,
        ConversationRuntime, RuntimeError, RuntimeErrorKind, StaticToolExecutor, ToolExecutor,
        TurnOutcome, DEFAULT_MAX_ITERATIONS, INTERRUPTED_TOOL_RESULT, MAX_ITERATIONS_ENV_VAR,
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
        assert_eq!(runtime.session().messages.len(), 1);

        // The REPL installs a fresh token for the next turn.
        runtime.set_cancellation_token(CancellationToken::new());
        let next = runtime.run_turn("try again", None).expect("next turn runs");
        assert_eq!(next.outcome, TurnOutcome::Completed);
        assert_eq!(runtime.session().messages.len(), 3);
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
        // Only advertise knobs that work: settings validation does not
        // accept `maxIterations` yet.
        assert!(error.to_string().contains(MAX_ITERATIONS_ENV_VAR));
        assert!(!error.to_string().contains("maxIterations"));
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
