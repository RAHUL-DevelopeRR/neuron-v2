//! How the agent loop uses what lifecycle hooks return: the context they add to a
//! prompt, the reason a blocked prompt gives, and the note that keeps a turn going
//! when a `Stop` hook asks for more.

/// How many times `Stop` hooks may keep one turn going. After that the turn ends
/// whatever they say, so a hook that always blocks cannot loop forever (hooks also
/// get `stop_hook_active` to stop asking themselves).
pub const MAX_STOP_HOOK_CONTINUATIONS: usize = 3;

/// Marks hook-provided context inside the user's message.
pub const HOOK_CONTEXT_HEADING: &str = "[Context from hooks]";

/// The user's prompt with what `SessionStart` and `UserPromptSubmit` hooks printed
/// after it.
#[must_use]
pub fn with_hook_context(prompt: String, context: &[String]) -> String {
    let context: Vec<&str> = context
        .iter()
        .map(|text| text.trim())
        .filter(|text| !text.is_empty())
        .collect();
    if context.is_empty() {
        return prompt;
    }
    format!("{prompt}\n\n{HOOK_CONTEXT_HEADING}\n{}", context.join("\n"))
}

/// The error a blocked prompt gives, with the hook's reason when it gave one.
#[must_use]
pub fn blocked_prompt_message(messages: &[String]) -> String {
    let reason = join_reason(messages);
    if reason.is_empty() {
        "A UserPromptSubmit hook blocked this prompt; it was not sent.".to_string()
    } else {
        format!("A UserPromptSubmit hook blocked this prompt; it was not sent: {reason}")
    }
}

/// What the model is told when a `Stop` hook asks it to keep going.
#[must_use]
pub fn stop_continuation_message(messages: &[String]) -> String {
    let reason = join_reason(messages);
    if reason.is_empty() {
        "A Stop hook asked you to continue: the task is not finished yet.".to_string()
    } else {
        format!("A Stop hook asked you to continue: {reason}")
    }
}

fn join_reason(messages: &[String]) -> String {
    messages
        .iter()
        .map(|message| message.trim())
        .filter(|message| !message.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{
        blocked_prompt_message, stop_continuation_message, with_hook_context, HOOK_CONTEXT_HEADING,
    };

    #[test]
    fn context_follows_the_prompt_under_a_heading() {
        assert_eq!(with_hook_context("fix it".into(), &[]), "fix it");
        assert_eq!(
            with_hook_context("fix it".into(), &[" ".into()]),
            "fix it",
            "blank output adds nothing"
        );
        assert_eq!(
            with_hook_context("fix it".into(), &["branch: main".into(), "CI red".into()]),
            format!("fix it\n\n{HOOK_CONTEXT_HEADING}\nbranch: main\nCI red")
        );
    }

    #[test]
    fn block_and_stop_messages_carry_the_reason() {
        assert!(blocked_prompt_message(&[]).ends_with("it was not sent."));
        assert!(blocked_prompt_message(&["no secrets".into()]).ends_with(": no secrets"));
        assert_eq!(
            stop_continuation_message(&["tests fail".into()]),
            "A Stop hook asked you to continue: tests fail"
        );
        assert!(stop_continuation_message(&[]).contains("not finished"));
    }
}
