//! Hook progress for the full-screen UI: `hook_started` / `hook_finished` events, with
//! the command shortened and anything that looks like a secret hidden.

// Output goes through the UI channel or runtime::ui_hooks.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use std::sync::mpsc::Sender;
use std::sync::Arc;

use neuron_tui::protocol::UiEvent;
use runtime::{HookProgressEvent, HookProgressReporter};

use crate::turn_sink::UiCapabilities;

/// How much of a hook command the UI sees.
const COMMAND_PREVIEW_CHARS: usize = 80;
const HIDDEN: &str = "***";

/// Sends hook progress to a UI that declared `hook_progress`; others hear nothing, as
/// before (the line terminal shows them with `NEURON_DEBUG`).
pub(crate) struct ChannelHookReporter {
    events: Sender<UiEvent>,
    capabilities: Arc<UiCapabilities>,
}

impl ChannelHookReporter {
    pub(crate) fn new(events: Sender<UiEvent>, capabilities: Arc<UiCapabilities>) -> Self {
        Self {
            events,
            capabilities,
        }
    }
}

impl HookProgressReporter for ChannelHookReporter {
    fn on_event(&mut self, event: &HookProgressEvent) {
        if !self.capabilities.hook_progress() {
            return;
        }
        let _ = self.events.send(hook_ui_event(event));
    }
}

/// The UI event for a hook's progress.
pub(crate) fn hook_ui_event(event: &HookProgressEvent) -> UiEvent {
    match event {
        HookProgressEvent::Started {
            event,
            command,
            matcher,
            tool_use_id,
            ..
        } => UiEvent::HookStarted {
            event: event.as_str().to_string(),
            matcher: matcher.clone(),
            command: command_preview(command),
            tool_use_id: tool_use_id.clone(),
        },
        HookProgressEvent::Completed {
            event,
            command,
            matcher,
            tool_use_id,
            outcome,
            duration_ms,
            message,
            ..
        } => UiEvent::HookFinished {
            event: event.as_str().to_string(),
            matcher: matcher.clone(),
            command: command_preview(command),
            tool_use_id: tool_use_id.clone(),
            outcome: outcome.as_str().to_string(),
            duration_ms: *duration_ms,
            message: message.as_deref().map(hide_secrets),
        },
        HookProgressEvent::Cancelled {
            event,
            command,
            matcher,
            tool_use_id,
            duration_ms,
            ..
        } => UiEvent::HookFinished {
            event: event.as_str().to_string(),
            matcher: matcher.clone(),
            command: command_preview(command),
            tool_use_id: tool_use_id.clone(),
            outcome: "cancelled".to_string(),
            duration_ms: *duration_ms,
            message: None,
        },
    }
}

/// A hook command as people may see it: one line, secrets hidden, at most
/// [`COMMAND_PREVIEW_CHARS`] characters.
pub(crate) fn command_preview(command: &str) -> String {
    let one_line = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let hidden = hide_secrets(&one_line);
    if hidden.chars().count() <= COMMAND_PREVIEW_CHARS {
        return hidden;
    }
    let mut shortened: String = hidden.chars().take(COMMAND_PREVIEW_CHARS - 1).collect();
    shortened.push('\u{2026}');
    shortened
}

/// Hides what follows `Bearer`, values of secret-named flags and variables, credentials
/// in URLs, and long token-shaped words.
pub(crate) fn hide_secrets(text: &str) -> String {
    let words: Vec<&str> = text.split(' ').collect();
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    let mut hide_next = false;
    for word in words {
        if hide_next && !word.is_empty() {
            out.push(HIDDEN.to_string());
            hide_next = false;
            continue;
        }
        let lower = word.to_ascii_lowercase();
        if lower == "bearer" || (lower.starts_with("--") && names_a_secret(&lower)) {
            if let Some((flag, _)) = word.split_once('=') {
                out.push(format!("{flag}={HIDDEN}"));
            } else {
                out.push(word.to_string());
                hide_next = true;
            }
            continue;
        }
        if let Some((name, value)) = word.split_once('=') {
            if names_a_secret(&name.to_ascii_lowercase()) && !value.is_empty() {
                out.push(format!("{name}={HIDDEN}"));
                continue;
            }
        }
        if let Some(scheme_end) = word.find("://") {
            let rest = &word[scheme_end + 3..];
            if let Some(at) = rest.find('@') {
                if !rest[..at].contains('/') {
                    out.push(format!(
                        "{}{HIDDEN}{}",
                        &word[..scheme_end + 3],
                        &rest[at..]
                    ));
                    continue;
                }
            }
        }
        if looks_like_a_token(word) {
            out.push(HIDDEN.to_string());
            continue;
        }
        out.push(word.to_string());
    }
    out.join(" ")
}

fn names_a_secret(name: &str) -> bool {
    [
        "token", "secret", "password", "passwd", "api_key", "apikey", "api-key", "auth",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

/// A long run of letters and digits mixing both, like an API key.
fn looks_like_a_token(word: &str) -> bool {
    let core = word.trim_matches(|ch: char| matches!(ch, '"' | '\'' | ',' | ';'));
    core.len() >= 24
        && core
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        && core.chars().any(|ch| ch.is_ascii_digit())
        && core.chars().any(|ch| ch.is_ascii_alphabetic())
        && !core.contains('/')
}

#[cfg(test)]
mod tests {
    use super::{command_preview, hide_secrets, hook_ui_event};
    use neuron_tui::protocol::UiEvent;
    use runtime::{HookEvent, HookOutcome, HookProgressEvent};

    #[test]
    fn secrets_in_hook_commands_are_hidden() {
        assert_eq!(
            hide_secrets("curl -H Authorization: Bearer abc.def"),
            "curl -H Authorization: Bearer ***"
        );
        assert_eq!(
            hide_secrets("notify --token s3cr3t --channel ops"),
            "notify --token *** --channel ops"
        );
        assert_eq!(hide_secrets("notify --api-key=xyz"), "notify --api-key=***");
        assert_eq!(hide_secrets("GH_TOKEN=ghp_x run.sh"), "GH_TOKEN=*** run.sh");
        assert_eq!(
            hide_secrets("curl https://user:pw@example.com/hook"),
            "curl https://***@example.com/hook"
        );
        // Built at run time so the source holds no literal live-key string.
        let live_key = format!("sk_{}_0123456789abcdefghijklmn", "live");
        assert_eq!(hide_secrets(&format!("send {live_key}")), "send ***");
        // Ordinary commands and paths stay as written.
        assert_eq!(
            hide_secrets("node ./hooks/pre-tool-use.mjs --verbose"),
            "node ./hooks/pre-tool-use.mjs --verbose"
        );
    }

    #[test]
    fn previews_are_one_short_line() {
        assert_eq!(command_preview("a\n  b\tc"), "a b c");
        let long = format!("echo {}", "x ".repeat(100));
        let preview = command_preview(&long);
        assert_eq!(preview.chars().count(), 80);
        assert!(preview.ends_with('\u{2026}'));
    }

    #[test]
    fn progress_becomes_hook_events() {
        let finished = hook_ui_event(&HookProgressEvent::Completed {
            event: HookEvent::PreToolUse,
            tool_name: "bash".into(),
            command: "guard.sh --token abc".into(),
            matcher: Some("bash".into()),
            tool_use_id: Some("call-1".into()),
            outcome: HookOutcome::Blocked,
            duration_ms: 7,
            message: Some("no rm -rf".into()),
        });
        assert_eq!(
            finished,
            UiEvent::HookFinished {
                event: "PreToolUse".into(),
                matcher: Some("bash".into()),
                command: "guard.sh --token ***".into(),
                tool_use_id: Some("call-1".into()),
                outcome: "blocked".into(),
                duration_ms: 7,
                message: Some("no rm -rf".into()),
            }
        );
        let cancelled = hook_ui_event(&HookProgressEvent::Cancelled {
            event: HookEvent::Stop,
            tool_name: String::new(),
            command: "check.sh".into(),
            matcher: None,
            tool_use_id: None,
            duration_ms: 3,
        });
        assert!(matches!(
            cancelled,
            UiEvent::HookFinished { ref outcome, .. } if outcome == "cancelled"
        ));
    }
}
