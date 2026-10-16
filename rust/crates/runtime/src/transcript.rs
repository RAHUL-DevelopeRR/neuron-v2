//! Transcript normalization.
//!
//! Providers reject transcripts where an assistant `tool_use` has no matching
//! `tool_result`, where a `tool_result` refers to an unknown call, or where the
//! conversation ends in an empty assistant message. Such transcripts appear
//! after crashes, interrupted turns, partial JSONL writes or hand-edited
//! session files. [`normalize_messages`] repairs them deterministically and is
//! run when a session is loaded and before every model request.

use std::collections::BTreeSet;

use crate::session::{ContentBlock, ConversationMessage, MessageRole};

/// Output recorded for a tool call whose result is missing from the transcript.
pub const MISSING_TOOL_RESULT_OUTPUT: &str = "Tool call was interrupted before it produced a result; it may or may not have run. Verify the workspace state before retrying.";

/// Counts of the repairs applied by [`normalize_messages`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TranscriptRepair {
    pub synthesized_tool_results: usize,
    pub dropped_orphan_tool_results: usize,
    pub dropped_empty_assistant_messages: usize,
}

impl TranscriptRepair {
    #[must_use]
    pub fn changed(&self) -> bool {
        self.total() > 0
    }

    #[must_use]
    pub fn total(&self) -> usize {
        self.synthesized_tool_results
            + self.dropped_orphan_tool_results
            + self.dropped_empty_assistant_messages
    }
}

/// Repairs `messages` in place:
///
/// * every assistant `tool_use` without a result gets a synthetic `is_error`
///   result, inserted before the next user/assistant turn;
/// * `tool_result` blocks that do not answer a pending call of the preceding
///   assistant message (orphans and duplicates) are dropped;
/// * assistant messages without any blocks, and trailing assistant messages
///   that contain only blank text, are dropped.
pub fn normalize_messages(messages: &mut Vec<ConversationMessage>) -> TranscriptRepair {
    let mut repair = TranscriptRepair::default();
    let mut normalized = Vec::with_capacity(messages.len());
    let mut pending = PendingToolUses::default();

    for mut message in messages.drain(..) {
        if message.role == MessageRole::Assistant {
            if message.blocks.is_empty() {
                repair.dropped_empty_assistant_messages += 1;
                continue;
            }
            repair.synthesized_tool_results += pending.flush_into(&mut normalized);
            pending.track(&message);
            normalized.push(message);
            continue;
        }

        let had_tool_results = message.blocks.iter().any(is_tool_result);
        let before = message.blocks.len();
        message.blocks.retain(|block| match block {
            ContentBlock::ToolResult { tool_use_id, .. } => pending.answer(tool_use_id),
            _ => true,
        });
        repair.dropped_orphan_tool_results += before - message.blocks.len();
        if message.blocks.is_empty() {
            continue;
        }
        if !had_tool_results || !message.blocks.iter().any(is_tool_result) {
            // A new user/system turn: any still-unanswered calls were interrupted.
            repair.synthesized_tool_results += pending.flush_into(&mut normalized);
        }
        normalized.push(message);
    }
    repair.synthesized_tool_results += pending.flush_into(&mut normalized);

    while normalized
        .last()
        .is_some_and(|message| message.role == MessageRole::Assistant && is_blank(message))
    {
        normalized.pop();
        repair.dropped_empty_assistant_messages += 1;
    }

    *messages = normalized;
    repair
}

/// Returns a normalized copy of `messages`, leaving the input untouched.
#[must_use]
pub fn normalized_messages(messages: &[ConversationMessage]) -> Vec<ConversationMessage> {
    let mut copy = messages.to_vec();
    normalize_messages(&mut copy);
    copy
}

/// Returns the ids of assistant tool calls that have no result yet.
#[must_use]
pub fn unanswered_tool_use_ids(messages: &[ConversationMessage]) -> Vec<String> {
    let mut answered = BTreeSet::new();
    let mut ids = Vec::new();
    for block in messages.iter().flat_map(|message| message.blocks.iter()) {
        match block {
            ContentBlock::ToolUse { id, .. } => ids.push(id.clone()),
            ContentBlock::ToolResult { tool_use_id, .. } => {
                answered.insert(tool_use_id.clone());
            }
            ContentBlock::Text { .. } => {}
        }
    }
    ids.retain(|id| !answered.contains(id));
    ids
}

fn is_tool_result(block: &ContentBlock) -> bool {
    matches!(block, ContentBlock::ToolResult { .. })
}

fn is_blank(message: &ConversationMessage) -> bool {
    message.blocks.iter().all(|block| match block {
        ContentBlock::Text { text } => text.trim().is_empty(),
        ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. } => false,
    })
}

#[derive(Default)]
struct PendingToolUses {
    calls: Vec<(String, String)>,
    answered: BTreeSet<String>,
}

impl PendingToolUses {
    fn track(&mut self, assistant: &ConversationMessage) {
        for block in &assistant.blocks {
            if let ContentBlock::ToolUse { id, name, .. } = block {
                if !self.calls.iter().any(|(existing, _)| existing == id) {
                    self.calls.push((id.clone(), name.clone()));
                }
            }
        }
    }

    /// Marks `id` answered; returns `false` for orphans and duplicates.
    fn answer(&mut self, id: &str) -> bool {
        self.calls.iter().any(|(pending, _)| pending == id) && self.answered.insert(id.to_string())
    }

    fn flush_into(&mut self, out: &mut Vec<ConversationMessage>) -> usize {
        let mut synthesized = 0;
        for (id, name) in self.calls.drain(..) {
            if self.answered.contains(&id) {
                continue;
            }
            out.push(ConversationMessage::tool_result(
                id,
                name,
                MISSING_TOOL_RESULT_OUTPUT,
                true,
            ));
            synthesized += 1;
        }
        self.answered.clear();
        synthesized
    }
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_messages, normalized_messages, unanswered_tool_use_ids, TranscriptRepair,
        MISSING_TOOL_RESULT_OUTPUT,
    };
    use crate::session::{ContentBlock, ConversationMessage, MessageRole};

    fn tool_use(id: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.to_string(),
            name: "bash".to_string(),
            input: "{}".to_string(),
        }
    }

    #[test]
    fn interrupted_tool_chain_gets_synthetic_results_before_next_turn() {
        let mut messages = vec![
            ConversationMessage::user_text("run two commands"),
            ConversationMessage::assistant(vec![tool_use("a"), tool_use("b")]),
            ConversationMessage::tool_result("a", "bash", "ok", false),
            ConversationMessage::user_text("what happened?"),
            ConversationMessage::assistant(vec![tool_use("c")]),
        ];

        let repair = normalize_messages(&mut messages);

        assert_eq!(repair.synthesized_tool_results, 2);
        assert_eq!(messages.len(), 7);
        assert!(matches!(
            &messages[3].blocks[0],
            ContentBlock::ToolResult { tool_use_id, is_error: true, output, .. }
                if tool_use_id == "b" && output == MISSING_TOOL_RESULT_OUTPUT
        ));
        assert_eq!(messages[4].role, MessageRole::User);
        assert!(matches!(
            &messages[6].blocks[0],
            ContentBlock::ToolResult { tool_use_id, is_error: true, .. } if tool_use_id == "c"
        ));
        assert_eq!(unanswered_tool_use_ids(&messages), Vec::<String>::new());
    }

    #[test]
    fn drops_orphan_and_duplicate_tool_results() {
        let mut messages = vec![
            ConversationMessage::tool_result("ghost", "bash", "orphan", false),
            ConversationMessage::user_text("hi"),
            ConversationMessage::assistant(vec![tool_use("a")]),
            ConversationMessage::tool_result("a", "bash", "first", false),
            ConversationMessage::tool_result("a", "bash", "duplicate", false),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "done".to_string(),
            }]),
            ConversationMessage::tool_result("a", "bash", "late orphan", false),
        ];

        let repair = normalize_messages(&mut messages);

        assert_eq!(
            repair,
            TranscriptRepair {
                synthesized_tool_results: 0,
                dropped_orphan_tool_results: 3,
                dropped_empty_assistant_messages: 0,
            }
        );
        assert_eq!(messages.len(), 4);
        assert!(matches!(
            &messages[2].blocks[0],
            ContentBlock::ToolResult { output, .. } if output == "first"
        ));
    }

    #[test]
    fn drops_empty_and_trailing_blank_assistant_messages() {
        let mut messages = vec![
            ConversationMessage::user_text("hi"),
            ConversationMessage::assistant(Vec::new()),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "hello".to_string(),
            }]),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "   ".to_string(),
            }]),
        ];

        let repair = normalize_messages(&mut messages);

        assert_eq!(repair.dropped_empty_assistant_messages, 2);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn normalization_is_idempotent_and_leaves_valid_transcripts_alone() {
        let messages = vec![
            ConversationMessage::user_text("hi"),
            ConversationMessage::assistant(vec![tool_use("a")]),
            ConversationMessage::tool_result("a", "bash", "ok", false),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "done".to_string(),
            }]),
        ];
        let normalized = normalized_messages(&messages);
        assert_eq!(normalized, messages);

        let mut broken = vec![
            ConversationMessage::user_text("hi"),
            ConversationMessage::assistant(vec![tool_use("a")]),
        ];
        assert!(normalize_messages(&mut broken).changed());
        assert!(!normalize_messages(&mut broken).changed());
    }
}
