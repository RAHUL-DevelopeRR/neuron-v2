//! Secret redaction for tool output (see the `redact` crate for the patterns).
//!
//! Tool results are redacted once, when the conversation records them
//! ([`redact_tool_result`] in `ConversationRuntime::record_tool_result`), so
//! the same redacted text reaches every consumer: the session file, the
//! model's next request, the UI's `ToolResult` event and the REPL renderer.
//! The model therefore never sees a credential a command printed; it sees
//! `[REDACTED:<kind>]` and can tell the user a secret was there. The session
//! file applies it again on write ([`crate::Session::push_message`]) for
//! callers that push tool results directly. Redaction is idempotent.
//!
//! Trade-off: a file read whose text holds a credential comes back redacted,
//! so an edit that has to match the credential text fails. `"redactSecrets":
//! false` in user settings turns redaction off (untrusted workspace settings
//! cannot, since only restricting keys survive the trust gate).

use std::borrow::Cow;

use crate::session::{ContentBlock, ConversationMessage};

pub use redact::{is_enabled, redact, redact_json, redact_with, register_secret, set_enabled};

/// Redacts the output of every `ToolResult` block in `message`.
#[must_use]
pub fn redact_tool_result(mut message: ConversationMessage) -> ConversationMessage {
    redact_tool_result_in_place(&mut message);
    message
}

/// In-place form of [`redact_tool_result`]; returns whether anything changed.
pub fn redact_tool_result_in_place(message: &mut ConversationMessage) -> bool {
    if !redact::is_enabled() {
        return false;
    }
    let mut changed = false;
    for block in &mut message.blocks {
        if let ContentBlock::ToolResult { output, .. } = block {
            if let Cow::Owned(redacted) = redact::redact(output) {
                *output = redacted;
                changed = true;
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::redact_tool_result;
    use crate::session::{ContentBlock, ConversationMessage};

    #[test]
    fn tool_result_output_is_redacted_and_other_fields_kept() {
        let message = ConversationMessage::tool_result(
            "tool-1",
            "bash",
            "OPENAI_API_KEY=sk-proj-Abc123def456GHI789jkl012MNO\nok",
            false,
        );
        let redacted = redact_tool_result(message);
        let ContentBlock::ToolResult {
            tool_use_id,
            tool_name,
            output,
            is_error,
        } = &redacted.blocks[0]
        else {
            panic!("tool result block");
        };
        assert_eq!(tool_use_id, "tool-1");
        assert_eq!(tool_name, "bash");
        assert!(!is_error);
        assert_eq!(output, "OPENAI_API_KEY=[REDACTED:api-key]\nok");
    }
}
