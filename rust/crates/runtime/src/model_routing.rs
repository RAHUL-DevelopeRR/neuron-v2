//! Model routing for one conversation: which model answers a request when the
//! session's model cannot do it.
//!
//! - A reply that answers a request to change files in chat, with no tool call in
//!   the whole turn, gets one nudge; when the next reply still makes no tool call,
//!   the request runs once more on a model known to call tools
//!   ([`ModelRouting::tool_retry_models`]).
//! - A provider that stays unavailable after the client's own retries (capacity,
//!   overload, 5xx, stalled streams) makes the turn fall over to the next model of
//!   the `providerFallbacks` chain ([`ModelRouting::fallbacks`]).
//! - Cheaper per-task models (`subagentModel`, `summaryModel`, `reviewerModel`) are
//!   read from settings ([`ModelRoutingSettings`]) and picked with [`task_model`].
//!
//! Switching models goes through [`crate::ApiClient::set_model_override`]; a
//! client that cannot switch keeps the session model and nothing is retried.

use crate::conversation::{RuntimeError, RuntimeErrorKind};

/// Sent once when a request to change files was answered in chat without any
/// tool call.
pub const CHAT_ONLY_NUDGE_MESSAGE: &str = "The user's request asks for files or code to be created or changed, but your reply made no tool call, so nothing was changed. Answering in chat does not do the work: use your tools now (for example write_file or edit_file) to make the requested changes in the workspace, then summarize what you changed. If you cannot make the change, say why.";

/// Settings that pick models for particular jobs. Every field is optional; an
/// unset one leaves the job on the session's model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelRoutingSettings {
    /// `toolCapableModels`: models proven to call tools, best first. A request
    /// answered in chat without tools is retried once on the first of them.
    pub tool_capable_models: Vec<String>,
    /// `autoToolModel`: the model used in place of `auto` when tools are attached.
    pub auto_tool_model: Option<String>,
    /// `subagentModel`: the `Agent` tool's model when the call names none.
    pub subagent_model: Option<String>,
    /// `summaryModel`: short drafting requests (commit messages).
    pub summary_model: Option<String>,
    /// `reviewerModel`: the completion check's reviewer.
    pub reviewer_model: Option<String>,
}

impl ModelRoutingSettings {
    #[must_use]
    pub fn tool_capable_models(&self) -> &[String] {
        &self.tool_capable_models
    }

    #[must_use]
    pub fn auto_tool_model(&self) -> Option<&str> {
        self.auto_tool_model.as_deref()
    }

    #[must_use]
    pub fn subagent_model(&self) -> Option<&str> {
        self.subagent_model.as_deref()
    }

    #[must_use]
    pub fn summary_model(&self) -> Option<&str> {
        self.summary_model.as_deref()
    }

    #[must_use]
    pub fn reviewer_model(&self) -> Option<&str> {
        self.reviewer_model.as_deref()
    }
}

/// The model for a job: the configured one when set (and not blank), otherwise
/// the session's.
#[must_use]
pub fn task_model<'a>(configured: Option<&'a str>, session_model: &'a str) -> &'a str {
    configured
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or(session_model)
}

/// How a conversation may move between models within a turn. The default
/// switches nothing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelRouting {
    /// Tools that change files are attached and the permission mode allows them:
    /// a chat-only answer to a request to change files is nudged, then retried.
    pub agent_mode: bool,
    /// Models a chat-only answer is retried on, best first (as the client sends
    /// them).
    pub tool_retry_models: Vec<String>,
    /// The `providerFallbacks` chain after the session's model, in order.
    pub fallbacks: Vec<String>,
}

impl ModelRouting {
    /// The model to retry a chat-only answer on: the first one that is not
    /// `current` (the model that answered, or the one asked for).
    #[must_use]
    pub fn tool_retry_model(&self, current: &[&str]) -> Option<&str> {
        self.tool_retry_models
            .iter()
            .map(String::as_str)
            .find(|model| !current.iter().any(|seen| same_model(seen, model)))
    }

    /// The next model of the fallback chain that this turn has not tried yet.
    #[must_use]
    pub fn next_fallback(&self, tried: &[String]) -> Option<&str> {
        self.fallbacks
            .iter()
            .map(String::as_str)
            .find(|model| !tried.iter().any(|seen| same_model(seen, model)))
    }
}

/// Whether two model ids name the same model, ignoring case and the `openai/`
/// transport prefix.
#[must_use]
pub fn same_model(left: &str, right: &str) -> bool {
    let strip = |model: &str| {
        let model = model.trim();
        model
            .strip_prefix("openai/")
            .unwrap_or(model)
            .to_ascii_lowercase()
    };
    strip(left) == strip(right)
}

/// Whether `error` means the provider could not serve the model right now even
/// after the client's own retries (capacity, overload, 5xx, a stalled or broken
/// stream), so another model may.
#[must_use]
pub fn is_provider_unavailable(error: &RuntimeError) -> bool {
    matches!(
        error.kind(),
        RuntimeErrorKind::ProviderUnavailable
            | RuntimeErrorKind::Timeout
            | RuntimeErrorKind::StreamInterrupted
    )
}

/// The notice shown when a turn moves to the next model of the fallback chain.
#[must_use]
pub fn fallover_notice(from: &str, to: &str, error: &RuntimeError) -> String {
    let reason = error.to_string();
    let reason = reason.lines().next().unwrap_or_default().trim();
    let reason: String = reason.chars().take(160).collect();
    format!("\n\u{26a0} {from} is unavailable ({reason}) \u{2014} switching to {to} for this turn\u{2026}")
}

/// The notice shown when a chat-only answer is retried on another model.
#[must_use]
pub fn tool_retry_notice(from: &str, to: &str) -> String {
    format!("\n\u{21bb} {from} answered without using tools \u{2014} retrying with {to}\u{2026}")
}

/// What the model that takes over a chat-only answer is told.
#[must_use]
pub fn tool_retry_message(model: &str) -> String {
    format!(
        "(Note from Neuron, not from the user: the replies above made no tool call, so nothing was changed. This request now runs on {model}.) Carry out the user's request above with tool calls: create or edit the files it needs in the workspace, then summarize what you changed."
    )
}

/// Words that start a request to change files or code.
const ACTION_VERBS: &[&str] = &[
    "create",
    "build",
    "write",
    "edit",
    "fix",
    "implement",
    "refactor",
    "add",
    "update",
    "modify",
    "change",
    "rename",
    "generate",
    "make",
    "scaffold",
    "rewrite",
    "port",
    "convert",
    "setup",
    "set up",
    "bootstrap",
    "patch",
    "remove",
    "delete",
    "replace",
    "move",
];

/// Verbs that are about code by themselves: no object needs to say so.
const CODE_ONLY_VERBS: &[&str] = &["implement", "refactor", "scaffold", "patch"];

/// Openers before the verb of a request ("please", "can you", ...).
const REQUEST_OPENERS: &[&str] = &[
    "please",
    "pls",
    "can you",
    "could you",
    "would you",
    "will you",
    "i want you to",
    "i need you to",
    "i'd like you to",
    "go ahead and",
    "now",
    "then",
    "also",
    "ok",
    "okay",
    "so",
    "and",
    "just",
    "let's",
    "lets",
    "help me",
    "you should",
];

/// Things that are code or files: the object of a request that needs tools.
const CODE_OBJECTS: &[&str] = &[
    "file",
    "code",
    "function",
    "method",
    "class",
    "struct",
    "module",
    "component",
    "script",
    "app",
    "application",
    "website",
    "web site",
    "site",
    "page",
    "landing",
    "api",
    "endpoint",
    "server",
    "cli",
    "tool",
    "program",
    "project",
    "repo",
    "library",
    "package",
    "crate",
    "game",
    "test",
    "bug",
    "error",
    "feature",
    "readme",
    "config",
    "dockerfile",
    "makefile",
    "html",
    "css",
    "javascript",
    "typescript",
    "python",
    "rust",
    "react",
    "database",
    "schema",
    "migration",
    "handler",
    "route",
    "button",
    "form",
    "ui",
    "frontend",
    "backend",
    "dashboard",
    "bot",
    "extension",
    "plugin",
    "workflow",
    "pipeline",
    "flag",
    "option",
    "command",
    "variable",
    "type",
    "interface",
    "implementation",
    "src",
];

/// Phrases that say the user wants no change.
const NO_CHANGE_PHRASES: &[&str] = &[
    "don't change",
    "do not change",
    "don't edit",
    "do not edit",
    "don't modify",
    "do not modify",
    "don't write",
    "do not write",
    "don't create",
    "do not create",
    "without changing",
    "without editing",
    "without modifying",
    "no code changes",
    "no changes",
    "just explain",
    "only explain",
    "just tell me",
    "just describe",
    "read-only",
    "read only",
];

/// Whether `request` (the user's message that started a turn) asks for files or
/// code to be created or changed: a sentence that opens with an action verb
/// ("build me a todo app", "please fix the failing test", "can you add a --json
/// flag?") about code or files. Conservative: questions about how or why,
/// requests that rule changes out and verbs without a code or file object do not
/// count.
#[must_use]
pub fn asks_for_file_changes(request: &str) -> bool {
    let lower = request.to_ascii_lowercase().replace('\u{2019}', "'");
    if NO_CHANGE_PHRASES
        .iter()
        .any(|phrase| lower.contains(phrase))
    {
        return false;
    }
    sentences(&lower)
        .iter()
        .any(|sentence| sentence_asks_for_change(sentence))
}

/// Sentences of `text`: split at line breaks, `!`, `?`, `;` and at a `.` that ends
/// a sentence (not the one in `main.rs`).
fn sentences(text: &str) -> Vec<&str> {
    let mut sentences = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        let ends = match ch {
            '\n' | '!' | '?' | ';' => true,
            '.' => chars.peek().is_none_or(|(_, next)| next.is_whitespace()),
            _ => false,
        };
        if ends {
            sentences.push(&text[start..index]);
            start = index + ch.len_utf8();
        }
    }
    sentences.push(&text[start..]);
    sentences
        .into_iter()
        .map(str::trim)
        .filter(|sentence| !sentence.is_empty())
        .collect()
}

fn sentence_asks_for_change(sentence: &str) -> bool {
    let mut rest = sentence.trim_start_matches(|ch: char| !ch.is_alphanumeric());
    // Strip any openers, in any order ("ok so can you please ...").
    loop {
        let before = rest;
        for opener in REQUEST_OPENERS {
            if let Some(after) = strip_word(rest, opener) {
                rest = after.trim_start_matches(|ch: char| ch == ',' || ch.is_whitespace());
            }
        }
        if rest == before {
            break;
        }
    }
    let Some(verb) = ACTION_VERBS
        .iter()
        .find(|verb| strip_word(rest, verb).is_some())
    else {
        return false;
    };
    let object = strip_word(rest, verb).unwrap_or_default();
    CODE_ONLY_VERBS.contains(verb) || names_code_or_file(object)
}

/// `text` after `word` when it starts with it as a whole word.
fn strip_word<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let after = text.strip_prefix(word)?;
    match after.chars().next() {
        Some(ch) if ch.is_alphanumeric() || ch == '_' => None,
        _ => Some(after),
    }
}

fn names_code_or_file(object: &str) -> bool {
    let words = object
        .split(|ch: char| !(ch.is_alphanumeric() || ch == '\'' || ch == '-'))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let has_word = |wanted: &str| {
        if wanted.contains(' ') {
            object.contains(wanted)
        } else {
            words.iter().any(|word| {
                *word == wanted
                    || word.strip_suffix('s') == Some(wanted)
                    || word.strip_suffix("es") == Some(wanted)
            })
        }
    };
    CODE_OBJECTS.iter().any(|noun| has_word(noun))
        // A path, a file name with an extension or code in backticks.
        || object.contains('`')
        || object.contains('/')
        || object.contains('\\')
        || object
            .split_whitespace()
            .any(|word| looks_like_file_name(word.trim_matches(|ch: char| ",:;()\"'".contains(ch))))
}

fn looks_like_file_name(word: &str) -> bool {
    let Some((stem, extension)) = word.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && (1..=5).contains(&extension.len())
        && extension.chars().all(|ch| ch.is_ascii_alphanumeric())
        && extension.chars().any(|ch| ch.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_and_edit_requests_ask_for_file_changes() {
        for request in [
            "build me a todo app in react",
            "Build me X: a snake game in HTML",
            "please fix the failing test in parser.rs",
            "Can you add a --json flag to the CLI?",
            "create a file hello.py that prints hi",
            "Implement retry with backoff",
            "refactor this",
            "write a python script that renames photos",
            "I have a small repo. Add a README with setup steps.",
            "ok so can you please update `config.toml` to use port 8080",
            "make a landing page for my bakery",
            "set up a rust project with clap",
            "edit src/main.rs to print the version",
        ] {
            assert!(asks_for_file_changes(request), "{request}");
        }
    }

    #[test]
    fn questions_chat_and_no_change_requests_do_not() {
        for request in [
            "how do I build a todo app in react?",
            "what does this function do?",
            "explain the build pipeline",
            "why is the test failing?",
            "write me a poem about autumn",
            "make it shorter",
            "fix my grammar: their going home",
            "review the auth module but don't change anything",
            "Add a section on pricing to your answer, just explain",
            "hello",
            "",
            "thanks, that works",
            "create an account for me on github",
        ] {
            assert!(!asks_for_file_changes(request), "{request}");
        }
    }

    #[test]
    fn file_names_and_paths_count_as_code_objects() {
        assert!(asks_for_file_changes("update notes.md with today's date"));
        assert!(asks_for_file_changes("delete docs/old"));
        assert!(!asks_for_file_changes("update me on the plan"));
    }

    #[test]
    fn task_model_prefers_the_configured_one() {
        assert_eq!(task_model(Some("cheap-model"), "auto"), "cheap-model");
        assert_eq!(task_model(Some("  "), "auto"), "auto");
        assert_eq!(task_model(None, "big-model"), "big-model");
    }

    #[test]
    fn tool_retry_skips_the_model_that_already_answered() {
        let routing = ModelRouting {
            agent_mode: true,
            tool_retry_models: vec!["openai/nemotron-3-ultra".into(), "openai/kimi-k2".into()],
            fallbacks: Vec::new(),
        };
        assert_eq!(
            routing.tool_retry_model(&["auto", "groq/openai/gpt-oss-120b"]),
            Some("openai/nemotron-3-ultra")
        );
        assert_eq!(
            routing.tool_retry_model(&["NEMOTRON-3-ultra"]),
            Some("openai/kimi-k2")
        );
        assert_eq!(ModelRouting::default().tool_retry_model(&["auto"]), None);
    }

    #[test]
    fn fallbacks_are_tried_in_order_once_each() {
        let routing = ModelRouting {
            fallbacks: vec!["a".into(), "b".into()],
            ..ModelRouting::default()
        };
        assert_eq!(routing.next_fallback(&[]), Some("a"));
        assert_eq!(routing.next_fallback(&["a".into()]), Some("b"));
        assert_eq!(
            routing.next_fallback(&["a".into(), "openai/b".into()]),
            None
        );
    }

    #[test]
    fn unavailable_providers_are_told_apart_from_final_errors() {
        assert!(is_provider_unavailable(&RuntimeError::with_kind(
            RuntimeErrorKind::ProviderUnavailable,
            "503 capacity_exhausted"
        )));
        assert!(is_provider_unavailable(&RuntimeError::with_kind(
            RuntimeErrorKind::Timeout,
            "no first token"
        )));
        assert!(!is_provider_unavailable(&RuntimeError::new("402 credits")));
        assert!(!is_provider_unavailable(&RuntimeError::with_kind(
            RuntimeErrorKind::ContextWindowExceeded,
            "too long"
        )));
        assert!(!is_provider_unavailable(&RuntimeError::with_kind(
            RuntimeErrorKind::Cancelled,
            "stop"
        )));
    }

    #[test]
    fn notices_name_both_models_and_never_the_vendor() {
        let notice = tool_retry_notice("auto", "nemotron-3-ultra");
        assert!(
            notice.contains("retrying with nemotron-3-ultra"),
            "{notice}"
        );
        let fallover = fallover_notice(
            "a",
            "b",
            &RuntimeError::with_kind(RuntimeErrorKind::ProviderUnavailable, "503\nmore"),
        );
        assert!(fallover.contains("a is unavailable (503)"), "{fallover}");
        assert!(fallover.contains("switching to b"), "{fallover}");
        for text in [CHAT_ONLY_NUDGE_MESSAGE, &tool_retry_message("m"), &notice] {
            assert!(!text.contains("Claude"), "{text}");
        }
    }
}

/// Whole turns through [`crate::ConversationRuntime`] with a mock client.
#[cfg(test)]
#[path = "model_routing_tests.rs"]
mod turn_tests;
