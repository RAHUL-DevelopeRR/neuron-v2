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

    mod in_the_agent_loop {
        use std::sync::{Arc, Mutex};

        use crate::config::{RuntimeFeatureConfig, RuntimeHookConfig};
        use crate::conversation::{
            ApiClient, ApiRequest, AssistantEvent, ConversationRuntime, RuntimeError,
            RuntimeErrorKind, StaticToolExecutor,
        };
        use crate::hook_matcher::{HookCommand, HookSource};
        use crate::hooks::HookEvent;
        use crate::lifecycle_hooks::{HOOK_CONTEXT_HEADING, MAX_STOP_HOOK_CONTINUATIONS};
        use crate::permissions::{PermissionMode, PermissionPolicy};
        use crate::session::{ContentBlock, MessageRole, Session};

        /// A hook that prints `text` and exits with `code`, in the platform's shell.
        fn hook(text: &str, code: i32) -> String {
            if cfg!(windows) {
                format!("echo {text}& exit /b {code}")
            } else {
                format!("echo '{text}'; exit {code}")
            }
        }

        /// Answers every request with text and keeps the last user message of each.
        #[derive(Clone, Default)]
        struct Recording {
            prompts: Arc<Mutex<Vec<String>>>,
        }

        impl ApiClient for Recording {
            fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                let last_user = request
                    .messages
                    .iter()
                    .rev()
                    .find(|message| message.role == MessageRole::User)
                    .map(|message| {
                        message
                            .blocks
                            .iter()
                            .filter_map(|block| match block {
                                ContentBlock::Text { text } => Some(text.as_str()),
                                _ => None,
                            })
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                self.prompts.lock().expect("prompts").push(last_user);
                Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ])
            }
        }

        fn runtime(
            hooks: RuntimeHookConfig,
            client: Recording,
        ) -> ConversationRuntime<Recording, StaticToolExecutor> {
            ConversationRuntime::new_with_features(
                Session::new(),
                client,
                StaticToolExecutor::new(),
                PermissionPolicy::new(PermissionMode::DangerFullAccess),
                vec!["system".to_string()],
                &RuntimeFeatureConfig::default().with_hooks(hooks),
            )
        }

        fn user_hook(event: HookEvent, command: String) -> RuntimeHookConfig {
            RuntimeHookConfig::default().with(event, HookCommand::new(command, HookSource::User))
        }

        #[test]
        fn a_user_prompt_submit_hook_can_block_the_prompt() {
            let client = Recording::default();
            let mut runtime = runtime(
                user_hook(HookEvent::UserPromptSubmit, hook("no deploys on friday", 2)),
                client.clone(),
            );

            let error = runtime
                .run_turn("deploy now", None)
                .expect_err("the hook blocks the prompt");

            assert_eq!(error.kind(), RuntimeErrorKind::BlockedByHook);
            assert!(
                error.to_string().contains("no deploys on friday"),
                "{error}"
            );
            assert!(client.prompts.lock().unwrap().is_empty(), "no model call");
            assert!(runtime.session().messages.is_empty(), "nothing recorded");
        }

        #[test]
        fn session_start_and_prompt_hooks_add_context_for_the_model() {
            let client = Recording::default();
            let hooks = user_hook(HookEvent::SessionStart, hook("branch is main", 0)).with(
                HookEvent::UserPromptSubmit,
                HookCommand::new(hook("ticket ABC-1", 0), HookSource::Project),
            );
            let mut runtime = runtime(hooks, client.clone());

            let started = runtime.run_session_start_hooks("startup");
            assert!(!started.is_denied() && !started.is_failed());
            runtime.run_turn("fix the bug", None).expect("first turn");
            runtime.run_turn("and the next", None).expect("second turn");

            let prompts = client.prompts.lock().unwrap().clone();
            assert!(prompts[0].starts_with("fix the bug\n\n"), "{}", prompts[0]);
            assert!(prompts[0].contains(HOOK_CONTEXT_HEADING));
            assert!(prompts[0].contains("branch is main"), "{}", prompts[0]);
            assert!(prompts[0].contains("ticket ABC-1"), "{}", prompts[0]);
            // Session context is added once; prompt context every time.
            assert!(!prompts[1].contains("branch is main"), "{}", prompts[1]);
            assert!(prompts[1].contains("ticket ABC-1"), "{}", prompts[1]);
        }

        #[test]
        fn session_start_matchers_compare_the_source() {
            let client = Recording::default();
            let hooks = RuntimeHookConfig::default().with(
                HookEvent::SessionStart,
                HookCommand::new(hook("resumed", 0), HookSource::User)
                    .with_matcher(Some("resume".into())),
            );
            let mut runtime = runtime(hooks, client);
            assert_eq!(
                runtime.run_session_start_hooks("startup").messages(),
                [] as [String; 0]
            );
            assert_eq!(
                runtime.run_session_start_hooks("resume").messages(),
                ["resumed"]
            );
        }

        #[test]
        fn a_stop_hook_keeps_the_turn_going_up_to_the_guard() {
            let client = Recording::default();
            let mut runtime = runtime(
                user_hook(HookEvent::Stop, hook("tests still fail", 2)),
                client.clone(),
            );

            let summary = runtime.run_turn("finish it", None).expect("turn ends");

            let prompts = client.prompts.lock().unwrap().clone();
            assert_eq!(prompts.len(), 1 + MAX_STOP_HOOK_CONTINUATIONS);
            assert!(prompts[1].contains("A Stop hook asked you to continue: tests still fail"));
            assert_eq!(summary.iterations, 1 + MAX_STOP_HOOK_CONTINUATIONS);
        }

        #[test]
        fn a_stop_hook_that_allows_ends_the_turn() {
            let client = Recording::default();
            let mut runtime = runtime(user_hook(HookEvent::Stop, hook("fine", 0)), client.clone());
            runtime.run_turn("finish it", None).expect("turn ends");
            assert_eq!(client.prompts.lock().unwrap().len(), 1);
        }

        #[test]
        fn manual_pre_compact_hooks_run_with_the_trigger() {
            let mut runtime = runtime(
                RuntimeHookConfig::default().with(
                    HookEvent::PreCompact,
                    HookCommand::new(hook("saving notes", 0), HookSource::User)
                        .with_matcher(Some("manual".into())),
                ),
                Recording::default(),
            );
            assert_eq!(
                runtime.run_pre_compact_hooks("manual").messages(),
                ["saving notes"]
            );
            assert_eq!(
                runtime.run_pre_compact_hooks("auto").messages(),
                [] as [String; 0]
            );
        }
    }
}
