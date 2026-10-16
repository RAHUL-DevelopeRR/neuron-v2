//! Model-aware context budgeting for the conversation loop.
//!
//! The context manager answers three questions before every model request:
//!
//! 1. How many input tokens may this request use? The budget is the model's
//!    context window (from [`crate::model_registry`]) minus the output reserve
//!    (`max_tokens`), the estimated size of the tool schemas and the system
//!    prompt.
//! 2. Is the transcript getting close to that budget? At
//!    [`DEFAULT_COMPACTION_THRESHOLD_PERCENT`] the manager first prunes the
//!    bodies of older tool results (head + tail), keeping the most recent
//!    results intact, and compacts the session only if that was not enough.
//! 3. Is a single tool result too large to insert at all? Every tool result is
//!    capped at insertion time ([`DEFAULT_MAX_TOOL_RESULT_CHARS`]).
//!
//! Token counts are heuristic estimates. They are deliberately conservative
//! for non-ASCII text (one token per character) and never split a UTF-8
//! sequence when truncating.

use crate::compact::{compact_session_with_policy, TokenCompaction};
use crate::model_registry::{self, DEFAULT_CONTEXT_WINDOW, DEFAULT_MAX_OUTPUT_TOKENS};
use crate::session::{ContentBlock, ConversationMessage, Session};

/// Default cap applied to every tool result when it enters the transcript.
pub const DEFAULT_MAX_TOOL_RESULT_CHARS: usize = 40_000;
/// Size older tool results are pruned down to once the context fills up.
pub const DEFAULT_PRUNED_TOOL_RESULT_CHARS: usize = 2_000;
/// Number of most recent tool results that pruning leaves untouched.
pub const DEFAULT_KEEP_RECENT_TOOL_RESULTS: usize = 4;
/// Share of the input budget at which pruning (and then compaction) starts.
pub const DEFAULT_COMPACTION_THRESHOLD_PERCENT: usize = 80;
/// Share of the input budget kept verbatim as the recent tail after compaction.
pub const DEFAULT_RETAINED_TAIL_PERCENT: usize = 40;
/// Below this input budget compaction cannot create useful room, so the
/// manager only prunes and leaves the final decision to the provider.
pub const MIN_COMPACTION_BUDGET_TOKENS: usize = 2_048;
/// Bounds for the summary written by automatic compaction (20% of the budget).
const MIN_SUMMARY_TOKENS: usize = 800;
const MAX_SUMMARY_TOKENS: usize = 8_000;
/// Environment variable overriding the per-result insertion cap.
pub const MAX_TOOL_RESULT_CHARS_ENV_VAR: &str = "NEURON_MAX_TOOL_RESULT_CHARS";

const MESSAGE_OVERHEAD_TOKENS: usize = 4;
const TOOL_BLOCK_OVERHEAD_TOKENS: usize = 12;
/// Characters reserved for the omission marker inserted by
/// [`truncate_head_tail`]; the marker itself is never longer than this.
const OMISSION_MARKER_RESERVE: usize = 40;

/// Estimates the token footprint of `text`.
///
/// ASCII is counted at four characters per token. Every non-ASCII character
/// (CJK, emoji, accented letters) is counted as a full token, which
/// over-estimates most scripts slightly and therefore errs on the safe side.
#[must_use]
pub fn estimate_text_tokens(text: &str) -> usize {
    let (ascii, other) = text.chars().fold((0_usize, 0_usize), |(ascii, other), ch| {
        if ch.is_ascii() {
            (ascii + 1, other)
        } else {
            (ascii, other + 1)
        }
    });
    ascii.div_ceil(4) + other
}

/// Estimates the token footprint of one content block.
#[must_use]
pub fn estimate_block_tokens(block: &ContentBlock) -> usize {
    match block {
        ContentBlock::Text { text } => estimate_text_tokens(text),
        ContentBlock::ToolUse { id, name, input } => {
            TOOL_BLOCK_OVERHEAD_TOKENS
                + estimate_text_tokens(id)
                + estimate_text_tokens(name)
                + estimate_text_tokens(input)
        }
        ContentBlock::ToolResult {
            tool_use_id,
            tool_name,
            output,
            ..
        } => {
            TOOL_BLOCK_OVERHEAD_TOKENS
                + estimate_text_tokens(tool_use_id)
                + estimate_text_tokens(tool_name)
                + estimate_text_tokens(output)
        }
    }
}

/// Estimates the token footprint of one message including role overhead.
#[must_use]
pub fn estimate_message_tokens(message: &ConversationMessage) -> usize {
    MESSAGE_OVERHEAD_TOKENS
        + message
            .blocks
            .iter()
            .map(estimate_block_tokens)
            .sum::<usize>()
}

/// Estimates the token footprint of a transcript.
#[must_use]
pub fn estimate_messages_tokens(messages: &[ConversationMessage]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

/// Estimates the token footprint of the system prompt sections.
#[must_use]
pub fn estimate_system_prompt_tokens(system_prompt: &[String]) -> usize {
    system_prompt
        .iter()
        .map(|section| estimate_text_tokens(section) + 2)
        .sum()
}

/// Truncates `text` to at most `max_chars` characters, keeping its head and
/// tail and replacing the middle with a `[… N chars omitted …]` marker.
///
/// Character counts are Unicode scalar values, so multi-byte sequences such as
/// emoji or CJK text are never split.
#[must_use]
pub fn truncate_head_tail(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let kept = max_chars.saturating_sub(OMISSION_MARKER_RESERVE);
    let head_chars = kept.div_ceil(2);
    let tail_chars = kept - head_chars;
    let omitted = total - head_chars - tail_chars;
    let head_end = byte_offset_of_char(text, head_chars);
    let tail_start = byte_offset_of_char(text, total - tail_chars);
    format!(
        "{}{}{}",
        &text[..head_end],
        omission_marker(omitted),
        &text[tail_start..]
    )
}

/// Marker inserted where [`truncate_head_tail`] removed content.
#[must_use]
pub fn omission_marker(omitted_chars: usize) -> String {
    format!("\n[… {omitted_chars} chars omitted …]\n")
}

fn byte_offset_of_char(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(offset, _)| offset)
}

/// Fixed token costs of a request that are not part of the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudget {
    /// Total tokens (input + output) the model accepts.
    pub context_window: usize,
    /// Output tokens requested per call (`max_tokens`).
    pub output_reserve: usize,
    /// Estimated tokens consumed by the tool definitions sent with each call.
    pub tool_schema_tokens: usize,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            context_window: DEFAULT_CONTEXT_WINDOW as usize,
            output_reserve: DEFAULT_MAX_OUTPUT_TOKENS as usize,
            tool_schema_tokens: 0,
        }
    }
}

impl ContextBudget {
    /// Builds a budget for `model` using the shared capability registry.
    #[must_use]
    pub fn for_model(model: &str, output_reserve: u32, tool_schema_tokens: usize) -> Self {
        Self {
            context_window: model_registry::context_window_or_default(model) as usize,
            output_reserve: output_reserve as usize,
            tool_schema_tokens,
        }
    }

    /// Input tokens available to the transcript:
    /// `context_window − output_reserve − tool_schema_tokens − system_prompt_tokens`.
    #[must_use]
    pub fn input_budget(&self, system_prompt_tokens: usize) -> usize {
        self.context_window
            .saturating_sub(self.output_reserve)
            .saturating_sub(self.tool_schema_tokens)
            .saturating_sub(system_prompt_tokens)
    }
}

/// Tunables for pruning, compaction and tool-result capping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextPolicy {
    pub threshold_percent: usize,
    pub retained_tail_percent: usize,
    pub max_tool_result_chars: usize,
    pub pruned_tool_result_chars: usize,
    pub keep_recent_tool_results: usize,
    /// Explicit compaction threshold in estimated transcript tokens. When set
    /// it replaces the percentage-based threshold.
    pub threshold_override_tokens: Option<usize>,
}

impl Default for ContextPolicy {
    fn default() -> Self {
        Self {
            threshold_percent: DEFAULT_COMPACTION_THRESHOLD_PERCENT,
            retained_tail_percent: DEFAULT_RETAINED_TAIL_PERCENT,
            max_tool_result_chars: DEFAULT_MAX_TOOL_RESULT_CHARS,
            pruned_tool_result_chars: DEFAULT_PRUNED_TOOL_RESULT_CHARS,
            keep_recent_tool_results: DEFAULT_KEEP_RECENT_TOOL_RESULTS,
            threshold_override_tokens: None,
        }
    }
}

impl ContextPolicy {
    /// Default policy with `NEURON_MAX_TOOL_RESULT_CHARS` applied when set.
    #[must_use]
    pub fn from_env() -> Self {
        let mut policy = Self::default();
        if let Some(limit) = std::env::var(MAX_TOOL_RESULT_CHARS_ENV_VAR)
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|value| *value > 0)
        {
            policy.max_tool_result_chars = limit;
        }
        policy
    }
}

/// Point-in-time view of the context budget, used for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextSnapshot {
    pub estimated_tokens: usize,
    pub system_prompt_tokens: usize,
    pub input_budget: usize,
    pub threshold_tokens: usize,
    pub budget: ContextBudget,
}

impl ContextSnapshot {
    /// Percentage of the input budget currently used (saturating at 999).
    #[must_use]
    pub fn used_percent(&self) -> usize {
        if self.input_budget == 0 {
            return 999;
        }
        (self.estimated_tokens.saturating_mul(100) / self.input_budget).min(999)
    }
}

/// What [`ContextManager::prepare_session`] did to make the next request fit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContextPreparation {
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub pruned_tool_results: usize,
    pub compacted_messages: usize,
    pub truncated_recent_tool_results: usize,
}

impl ContextPreparation {
    #[must_use]
    pub fn changed(&self) -> bool {
        self.pruned_tool_results > 0
            || self.compacted_messages > 0
            || self.truncated_recent_tool_results > 0
    }
}

/// Applies the budget and policy to a session before each model request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextManager {
    budget: ContextBudget,
    policy: ContextPolicy,
}

impl ContextManager {
    #[must_use]
    pub fn new(budget: ContextBudget, policy: ContextPolicy) -> Self {
        Self { budget, policy }
    }

    #[must_use]
    pub fn budget(&self) -> ContextBudget {
        self.budget
    }

    #[must_use]
    pub fn policy(&self) -> &ContextPolicy {
        &self.policy
    }

    pub fn set_threshold_override(&mut self, threshold_tokens: Option<usize>) {
        self.policy.threshold_override_tokens = threshold_tokens;
    }

    /// Token count at which pruning/compaction starts for `input_budget`.
    #[must_use]
    pub fn threshold_tokens(&self, input_budget: usize) -> usize {
        self.policy
            .threshold_override_tokens
            .unwrap_or_else(|| input_budget.saturating_mul(self.policy.threshold_percent) / 100)
    }

    #[must_use]
    pub fn snapshot(
        &self,
        messages: &[ConversationMessage],
        system_prompt: &[String],
    ) -> ContextSnapshot {
        let system_prompt_tokens = estimate_system_prompt_tokens(system_prompt);
        let input_budget = self.budget.input_budget(system_prompt_tokens);
        ContextSnapshot {
            estimated_tokens: estimate_messages_tokens(messages),
            system_prompt_tokens,
            input_budget,
            threshold_tokens: self.threshold_tokens(input_budget),
            budget: self.budget,
        }
    }

    /// Caps a tool result at insertion time.
    #[must_use]
    pub fn cap_tool_result(&self, output: String) -> String {
        if output.len() <= self.policy.max_tool_result_chars {
            // Byte length bounds the char count, so this is a cheap fast path.
            return output;
        }
        truncate_head_tail(&output, self.policy.max_tool_result_chars)
    }

    /// Head+tail-truncates the bodies of tool results older than the most
    /// recent [`ContextPolicy::keep_recent_tool_results`]. Returns how many
    /// results were shortened.
    pub fn prune_old_tool_results(&self, messages: &mut [ConversationMessage]) -> usize {
        prune_tool_results(
            messages,
            self.policy.keep_recent_tool_results,
            self.policy.pruned_tool_result_chars,
        )
    }

    /// Makes `session` fit the budget for the next request: prune old tool
    /// results at the threshold, compact if that is not enough, and as a last
    /// resort shorten even the most recent tool results when the transcript
    /// still exceeds the hard input budget.
    pub fn prepare_session(
        &self,
        session: &mut Session,
        system_prompt: &[String],
    ) -> ContextPreparation {
        let snapshot = self.snapshot(&session.messages, system_prompt);
        let mut preparation = ContextPreparation {
            tokens_before: snapshot.estimated_tokens,
            tokens_after: snapshot.estimated_tokens,
            ..ContextPreparation::default()
        };
        if snapshot.estimated_tokens < snapshot.threshold_tokens {
            return preparation;
        }

        preparation.pruned_tool_results = self.prune_old_tool_results(&mut session.messages);
        let mut tokens = estimate_messages_tokens(&session.messages);

        let can_compact = self.policy.threshold_override_tokens.is_some()
            || snapshot.input_budget >= MIN_COMPACTION_BUDGET_TOKENS;
        if tokens >= snapshot.threshold_tokens && can_compact {
            let reference = snapshot.input_budget.min(snapshot.threshold_tokens.max(1));
            let result = compact_session_with_policy(
                session,
                TokenCompaction {
                    min_recent_messages: 2,
                    recent_tokens_budget: reference
                        .saturating_mul(self.policy.retained_tail_percent)
                        / 100,
                    max_summary_tokens: (reference / 5)
                        .clamp(MIN_SUMMARY_TOKENS, MAX_SUMMARY_TOKENS),
                },
            );
            if result.removed_message_count > 0 {
                preparation.compacted_messages = result.removed_message_count;
                *session = result.compacted_session;
                tokens = estimate_messages_tokens(&session.messages);
            }
        }

        if tokens > snapshot.input_budget {
            preparation.truncated_recent_tool_results = prune_tool_results(
                &mut session.messages,
                0,
                self.policy.pruned_tool_result_chars,
            );
            tokens = estimate_messages_tokens(&session.messages);
        }

        preparation.tokens_after = tokens;
        preparation
    }
}

/// Shortens every tool result except the `keep_recent` most recent ones to at
/// most `max_chars` characters. Results already within the limit are left
/// alone, so pruning is idempotent.
pub fn prune_tool_results(
    messages: &mut [ConversationMessage],
    keep_recent: usize,
    max_chars: usize,
) -> usize {
    let total_results = messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter(|block| matches!(block, ContentBlock::ToolResult { .. }))
        .count();
    let prunable = total_results.saturating_sub(keep_recent);
    let mut seen = 0;
    let mut pruned = 0;
    for message in messages.iter_mut() {
        for block in &mut message.blocks {
            let ContentBlock::ToolResult { output, .. } = block else {
                continue;
            };
            if seen >= prunable {
                return pruned;
            }
            seen += 1;
            if output.chars().count() > max_chars {
                *output = truncate_head_tail(output, max_chars);
                pruned += 1;
            }
        }
    }
    pruned
}

#[cfg(test)]
mod tests {
    use super::{
        estimate_messages_tokens, estimate_text_tokens, omission_marker, prune_tool_results,
        truncate_head_tail, ContextBudget, ContextManager, ContextPolicy,
        DEFAULT_MAX_TOOL_RESULT_CHARS,
    };
    use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};

    #[test]
    fn estimates_ascii_and_unicode_conservatively() {
        assert_eq!(estimate_text_tokens(""), 0);
        assert_eq!(estimate_text_tokens("abcd"), 1);
        assert_eq!(estimate_text_tokens("abcde"), 2);
        // Each non-ASCII scalar counts as a full token.
        assert_eq!(estimate_text_tokens("日本語"), 3);
        assert_eq!(estimate_text_tokens("🙂🙂"), 2);
        assert_eq!(estimate_text_tokens("ab🙂"), 2);
    }

    #[test]
    fn head_tail_truncation_never_splits_multibyte_characters() {
        // Every character is multi-byte so any byte-based cut would panic or
        // produce invalid UTF-8.
        let text = "🙂日本語é".repeat(500);
        for max in [0, 1, 39, 40, 41, 57, 100, 777, 1_999] {
            let truncated = truncate_head_tail(&text, max);
            assert!(truncated.chars().count() <= max.max(40) + 1, "max={max}");
            assert!(truncated.contains("chars omitted"), "max={max}");
        }
        let truncated = truncate_head_tail(&text, 100);
        let omitted = 2_500 - 60;
        assert!(truncated.contains(&omission_marker(omitted)));
        assert!(truncated.starts_with("🙂日本語é"));
        assert!(truncated.ends_with("🙂日本語é"));
    }

    #[test]
    fn head_tail_truncation_keeps_short_text_unchanged() {
        assert_eq!(truncate_head_tail("short 🙂", 8), "short 🙂");
        assert_eq!(truncate_head_tail("", 0), "");
    }

    #[test]
    fn caps_huge_tool_output_at_insertion() {
        let manager = ContextManager::default();
        let huge = format!("HEAD{}TAIL", "x".repeat(5_000_000));
        let capped = manager.cap_tool_result(huge);
        assert!(capped.chars().count() <= DEFAULT_MAX_TOOL_RESULT_CHARS);
        assert!(capped.starts_with("HEAD"));
        assert!(capped.ends_with("TAIL"));
        assert!(capped.contains("chars omitted"));

        let small = "fits".to_string();
        assert_eq!(manager.cap_tool_result(small.clone()), small);
    }

    #[test]
    fn input_budget_subtracts_reserve_tools_and_system_prompt() {
        let budget = ContextBudget {
            context_window: 131_072,
            output_reserve: 32_000,
            tool_schema_tokens: 6_000,
        };
        assert_eq!(budget.input_budget(3_072), 90_000);
        // Saturates instead of underflowing.
        assert_eq!(budget.input_budget(200_000), 0);
        let manager = ContextManager::new(budget, ContextPolicy::default());
        assert_eq!(manager.threshold_tokens(90_000), 72_000);
        assert_eq!(manager.threshold_tokens(0), 0);
    }

    #[test]
    fn budget_uses_registry_window_and_defaults_for_unknown_models() {
        let known = ContextBudget::for_model("openai/gemini/gemini-2.5-flash", 32_000, 100);
        assert_eq!(known.context_window, 1_048_576);
        let unknown = ContextBudget::for_model("vendor/never-heard-of-it", 8_192, 0);
        assert_eq!(
            unknown.context_window,
            crate::model_registry::DEFAULT_CONTEXT_WINDOW as usize
        );
    }

    #[test]
    fn threshold_boundary_is_exclusive_below_and_inclusive_at_limit() {
        let budget = ContextBudget {
            context_window: 10_000,
            output_reserve: 1_000,
            tool_schema_tokens: 0,
        };
        let manager = ContextManager::new(budget, ContextPolicy::default());
        let mut session = Session::new();
        // Each message: 4 overhead + 1 token per 4 ASCII chars.
        let threshold = manager.threshold_tokens(budget.input_budget(0));
        assert_eq!(threshold, 7_200);
        let below = (threshold - 5) * 4;
        session.messages = vec![ConversationMessage::user_text("a".repeat(below))];
        assert_eq!(estimate_messages_tokens(&session.messages), threshold - 1);
        let untouched = manager.prepare_session(&mut session, &[]);
        assert!(!untouched.changed());
        assert_eq!(untouched.tokens_before, threshold - 1);
    }

    #[test]
    fn prunes_old_tool_results_before_compacting_and_keeps_recent_ones() {
        let budget = ContextBudget {
            context_window: 40_000,
            output_reserve: 4_000,
            tool_schema_tokens: 0,
        };
        let manager = ContextManager::new(budget, ContextPolicy::default());
        let mut session = Session::new();
        session
            .messages
            .push(ConversationMessage::user_text("inspect"));
        for index in 0..6 {
            session.messages.push(ConversationMessage::assistant(vec![
                ContentBlock::ToolUse {
                    id: format!("t{index}"),
                    name: "read_file".to_string(),
                    input: format!(r#"{{"path":"f{index}.rs"}}"#),
                },
            ]));
            session.messages.push(ConversationMessage::tool_result(
                format!("t{index}"),
                "read_file",
                format!("{index}").repeat(24_000),
                false,
            ));
        }

        let preparation = manager.prepare_session(&mut session, &[]);
        assert_eq!(preparation.pruned_tool_results, 2);
        assert_eq!(preparation.compacted_messages, 0, "pruning was enough");
        assert!(preparation.tokens_after < preparation.tokens_before);
        let outputs = session
            .messages
            .iter()
            .flat_map(|message| message.blocks.iter())
            .filter_map(|block| match block {
                ContentBlock::ToolResult { output, .. } => Some(output.chars().count()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(outputs[0] <= 2_000 && outputs[1] <= 2_000);
        assert!(outputs[2..].iter().all(|len| *len == 24_000));
    }

    #[test]
    fn compacts_when_pruning_is_not_enough() {
        let budget = ContextBudget {
            context_window: 20_000,
            output_reserve: 2_000,
            tool_schema_tokens: 0,
        };
        let manager = ContextManager::new(budget, ContextPolicy::default());
        let mut session = Session::new();
        for index in 0..40 {
            session
                .messages
                .push(ConversationMessage::user_text(format!(
                    "request {index}: {}",
                    "word ".repeat(400)
                )));
            session
                .messages
                .push(ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: format!("answer {index}"),
                }]));
        }

        let preparation = manager.prepare_session(&mut session, &[]);
        assert!(preparation.compacted_messages > 0);
        assert_eq!(session.messages[0].role, MessageRole::System);
        assert!(preparation.tokens_after < manager.threshold_tokens(18_000));
        assert!(session.compaction.is_some());
    }

    #[test]
    fn prune_helper_respects_keep_recent_and_is_idempotent() {
        let mut messages = vec![
            ConversationMessage::tool_result("a", "bash", "1".repeat(50), false),
            ConversationMessage::tool_result("b", "bash", "2".repeat(50), false),
        ];
        assert_eq!(prune_tool_results(&mut messages, 1, 45), 1);
        assert_eq!(prune_tool_results(&mut messages, 1, 45), 0);
        assert!(matches!(
            &messages[1].blocks[0],
            ContentBlock::ToolResult { output, .. } if output.len() == 50
        ));
    }
}
