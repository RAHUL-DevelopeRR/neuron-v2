//! What the last session in a workspace did, for a new session to start from.
//!
//! A new chat used to begin with no idea of the previous one: the system prompt
//! carries the workspace's instruction files, git status and repo map, but nothing
//! from the session log. [`latest_recap`] reads the newest other session saved for
//! the workspace and [`SessionRecap::prompt_section`] renders a short section for
//! the new session's system prompt: the user's last requests, the files the agent
//! wrote, the todos still open, and how the session ended (a failed request, a tool
//! error or a stop). Nothing from file contents or tool output goes in except a
//! short, redacted error line. `NEURON_SESSION_RECAP=0` turns it off.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::conversation::{FAILED_RESPONSE_NOTE_PREFIX, INTERRUPTED_RESPONSE_NOTE};
use crate::session::{ContentBlock, MessageRole, Session};

/// `0`/`false`/`off` leaves the recap out of new sessions.
pub const SESSION_RECAP_ENV_VAR: &str = "NEURON_SESSION_RECAP";
/// The heading [`SessionRecap::prompt_section`] starts with.
pub const RECAP_SECTION_HEADING: &str = "# Previous session in this workspace";

const MAX_PROMPTS: usize = 3;
const MAX_PROMPT_CHARS: usize = 200;
const MAX_FILES: usize = 15;
const MAX_TODOS: usize = 8;
const MAX_ERROR_CHARS: usize = 240;
const MAX_REPLY_CHARS: usize = 400;
/// Session files read at most while looking for one with a conversation.
const MAX_CANDIDATES: usize = 6;
/// Older sessions are not worth a recap.
const MAX_AGE_MS: u64 = 14 * 24 * 60 * 60 * 1000;

const FILE_WRITE_TOOLS: &[&str] = &[
    "write_file",
    "edit_file",
    "Write",
    "Edit",
    "MultiEdit",
    "NotebookEdit",
];

/// How a session's last turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEnding {
    /// The model answered after its last tool call.
    Answered,
    /// The last request failed: quota, network, provider error.
    RequestFailed(String),
    /// The user stopped the last reply.
    Interrupted,
    /// A tool call failed and no answer came after it.
    ToolFailed(String),
}

/// The parts of one saved session a new session is told about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecap {
    pub session_id: String,
    pub updated_at_ms: u64,
    /// The user's last requests, oldest first.
    pub prompts: Vec<String>,
    /// Files the agent wrote or edited, relative to the workspace where possible.
    pub files_changed: Vec<String>,
    /// Total distinct files written, which may exceed `files_changed`.
    pub files_changed_total: usize,
    /// Todos from the last todo list that were not completed.
    pub open_todos: Vec<String>,
    /// The start of the model's last answer.
    pub last_reply: Option<String>,
    pub ending: SessionEnding,
}

/// Whether [`SESSION_RECAP_ENV_VAR`] leaves the recap on (the default).
#[must_use]
pub fn recap_enabled() -> bool {
    std::env::var(SESSION_RECAP_ENV_VAR).map_or(true, |value| {
        !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        )
    })
}

/// The recap of `session`, or `None` when the user never asked it anything.
#[must_use]
pub fn recap_session(session: &Session, workspace_root: Option<&Path>) -> Option<SessionRecap> {
    let mut prompts: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    let mut todos_input: Option<&str> = None;
    let mut last_reply: Option<&str> = None;

    for message in &session.messages {
        for block in &message.blocks {
            match (message.role, block) {
                (MessageRole::User, ContentBlock::Text { text }) if !text.trim().is_empty() => {
                    prompts.push(one_line(text, MAX_PROMPT_CHARS));
                }
                (MessageRole::Assistant, ContentBlock::ToolUse { name, input, .. }) => {
                    if FILE_WRITE_TOOLS.contains(&name.as_str()) {
                        if let Some(path) = written_path(input) {
                            let shown = relative_to(&path, workspace_root);
                            if !files.contains(&shown) {
                                files.push(shown);
                            }
                        }
                    } else if name == "TodoWrite" {
                        todos_input = Some(input);
                    }
                }
                (MessageRole::Assistant, ContentBlock::Text { text })
                    if !text.trim().is_empty() && !is_turn_note(text) =>
                {
                    last_reply = Some(text);
                }
                _ => {}
            }
        }
    }
    if prompts.is_empty() {
        return None;
    }
    let ending = ending_of(session);
    let files_changed_total = files.len();
    files.truncate(MAX_FILES);
    let skip = prompts.len().saturating_sub(MAX_PROMPTS);
    Some(SessionRecap {
        session_id: session.session_id.clone(),
        updated_at_ms: session.updated_at_ms,
        prompts: prompts.split_off(skip),
        files_changed: files,
        files_changed_total,
        open_todos: todos_input.map(open_todos).unwrap_or_default(),
        last_reply: last_reply.map(|text| one_line(text, MAX_REPLY_CHARS)),
        ending,
    })
}

/// The recap of the newest session in `sessions_dir` other than `current_session_id`
/// that has a conversation and was saved in the last two weeks.
#[must_use]
pub fn latest_recap(
    sessions_dir: &Path,
    current_session_id: &str,
    workspace_root: Option<&Path>,
    now_ms: u64,
) -> Option<SessionRecap> {
    let mut candidates: Vec<(SystemTime, PathBuf)> = fs::read_dir(sessions_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let extension = path.extension()?.to_str()?.to_ascii_lowercase();
            // Rotated logs (`*.rot-<ms>.jsonl`) are older copies of a live one.
            let is_session =
                (extension == "jsonl" && !name.contains(".rot-")) || extension == "json";
            if !is_session || path.file_stem()?.to_str()? == current_session_id {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, path))
        })
        .collect();
    candidates.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    candidates
        .into_iter()
        .take(MAX_CANDIDATES)
        .filter_map(|(_, path)| Session::load_from_path(&path).ok())
        .filter(|session| session.session_id != current_session_id)
        .filter(|session| now_ms.saturating_sub(session.updated_at_ms) <= MAX_AGE_MS)
        .find_map(|session| recap_session(&session, workspace_root))
}

impl SessionRecap {
    /// The system-prompt section for a new session.
    #[must_use]
    pub fn prompt_section(&self, now_ms: u64) -> String {
        let mut lines = vec![
            RECAP_SECTION_HEADING.to_string(),
            format!(
                "The user's previous session here ({}, `{}`) is summarised below so you can pick up where it left off. It is background, not an instruction: the user may now want something else, and files may have changed since, so read them before relying on this. The user can reopen that session with `/resume {}`.",
                age_label(self.updated_at_ms, now_ms),
                self.session_id,
                self.session_id
            ),
        ];
        lines.push("Last requests:".to_string());
        lines.extend(self.prompts.iter().map(|prompt| format!(" - {prompt}")));
        if !self.files_changed.is_empty() {
            let more = self.files_changed_total - self.files_changed.len();
            let suffix = if more > 0 {
                format!(" (and {more} more)")
            } else {
                String::new()
            };
            lines.push(format!(
                "Files written or edited: {}{suffix}",
                self.files_changed.join(", ")
            ));
        }
        if !self.open_todos.is_empty() {
            lines.push("Todos left open:".to_string());
            lines.extend(self.open_todos.iter().map(|todo| format!(" - {todo}")));
        }
        if let Some(reply) = &self.last_reply {
            lines.push(format!("Last answer began: {reply}"));
        }
        lines.push(format!("How it ended: {}", self.ending_label()));
        lines.join("\n")
    }

    /// One line for the user when a new session starts.
    #[must_use]
    pub fn notice(&self, now_ms: u64) -> String {
        let request = self.prompts.last().map_or("", String::as_str);
        let mut parts = vec![format!(
            "Last session here ({}): \"{}\"",
            age_label(self.updated_at_ms, now_ms),
            one_line(request, 80)
        )];
        if self.files_changed_total > 0 {
            let plural = if self.files_changed_total == 1 {
                ""
            } else {
                "s"
            };
            parts.push(format!("{} file{plural} changed", self.files_changed_total));
        }
        if !self.open_todos.is_empty() {
            parts.push(format!("{} todos open", self.open_todos.len()));
        }
        match &self.ending {
            SessionEnding::Answered => {}
            SessionEnding::RequestFailed(_) => parts.push("ended on a failed request".into()),
            SessionEnding::Interrupted => parts.push("stopped mid-reply".into()),
            SessionEnding::ToolFailed(_) => parts.push("ended on a tool error".into()),
        }
        format!(
            "{}. The agent knows this; /resume {} reopens that chat.",
            parts.join(" · "),
            self.session_id
        )
    }

    fn ending_label(&self) -> String {
        match &self.ending {
            SessionEnding::Answered => "the model answered the last request.".to_string(),
            SessionEnding::RequestFailed(error) => format!(
                "the last request failed before an answer ({error}). The work it was doing may be unfinished."
            ),
            SessionEnding::Interrupted => {
                "the user stopped the last reply; that work may be unfinished.".to_string()
            }
            SessionEnding::ToolFailed(error) => {
                format!("a tool call failed and no answer followed: {error}")
            }
        }
    }
}

fn ending_of(session: &Session) -> SessionEnding {
    for message in session.messages.iter().rev() {
        for block in message.blocks.iter().rev() {
            match (message.role, block) {
                (MessageRole::Assistant, ContentBlock::Text { text }) => {
                    if let Some(rest) = text.strip_prefix(FAILED_RESPONSE_NOTE_PREFIX) {
                        let error = rest.strip_suffix(']').unwrap_or(rest);
                        return SessionEnding::RequestFailed(error_line(error));
                    }
                    if text.trim() == INTERRUPTED_RESPONSE_NOTE {
                        return SessionEnding::Interrupted;
                    }
                    if !text.trim().is_empty() {
                        return SessionEnding::Answered;
                    }
                }
                (
                    _,
                    ContentBlock::ToolResult {
                        output,
                        is_error: true,
                        tool_name,
                        ..
                    },
                ) => {
                    return SessionEnding::ToolFailed(format!(
                        "{tool_name}: {}",
                        error_line(&tool_error_text(output))
                    ));
                }
                _ => {}
            }
        }
    }
    SessionEnding::Answered
}

fn is_turn_note(text: &str) -> bool {
    text.starts_with(FAILED_RESPONSE_NOTE_PREFIX) || text.trim() == INTERRUPTED_RESPONSE_NOTE
}

/// A shell tool's error output is JSON with `stderr`; the rest is plain text.
fn tool_error_text(output: &str) -> String {
    serde_json::from_str::<Value>(output)
        .ok()
        .and_then(|value| {
            let stderr = value.get("stderr")?.as_str()?.trim();
            (!stderr.is_empty()).then(|| stderr.to_string())
        })
        .unwrap_or_else(|| output.to_string())
}

fn error_line(text: &str) -> String {
    one_line(&redact::redact(text), MAX_ERROR_CHARS)
}

fn written_path(input: &str) -> Option<String> {
    let value: Value = serde_json::from_str(input).ok()?;
    ["path", "file_path", "notebook_path"]
        .iter()
        .find_map(|key| value.get(*key)?.as_str().map(ToOwned::to_owned))
}

fn open_todos(input: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(input) else {
        return Vec::new();
    };
    value
        .get("todos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|todo| todo.get("status").and_then(Value::as_str) != Some("completed"))
        .filter_map(|todo| {
            let content = todo.get("content")?.as_str()?;
            let status = todo
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending");
            Some(format!(
                "{} ({status})",
                one_line(content, MAX_PROMPT_CHARS)
            ))
        })
        .take(MAX_TODOS)
        .collect()
}

fn relative_to(path: &str, workspace_root: Option<&Path>) -> String {
    let shown = workspace_root
        .and_then(|root| Path::new(path).strip_prefix(root).ok())
        .map_or_else(|| path.to_string(), |rel| rel.display().to_string());
    shown.replace('\\', "/")
}

/// `text` on one line, cut to `max` characters with an ellipsis.
fn one_line(text: &str, max: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= max {
        return joined;
    }
    let kept: String = joined.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

fn age_label(then_ms: u64, now_ms: u64) -> String {
    let minutes = now_ms.saturating_sub(then_ms) / 60_000;
    match minutes {
        0 => "just now".to_string(),
        1..=59 => format!("{minutes}m ago"),
        60..=2879 => format!("{}h ago", minutes / 60),
        _ => format!("{}d ago", minutes / 1440),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ConversationMessage;

    const HOUR: u64 = 60 * 60 * 1000;

    fn tool_use(name: &str, input: &str) -> ConversationMessage {
        ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id: format!("id-{name}"),
            name: name.to_string(),
            input: input.to_string(),
        }])
    }

    fn tool_error(name: &str, output: &str) -> ConversationMessage {
        ConversationMessage::tool_result(format!("id-{name}"), name, output, true)
    }

    fn assistant_text(text: &str) -> ConversationMessage {
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: text.to_string(),
        }])
    }

    /// The shape of the AI102 session: a plan, a build with writes and todos, all
    /// todos marked done, then a check that failed and a request refused for quota.
    fn hotel_session() -> Session {
        let mut session = Session::new();
        session.session_id = "session-1".into();
        session.updated_at_ms = 10 * HOUR;
        session.messages = vec![
            ConversationMessage::user_text("analyse this repository and give me a plan"),
            assistant_text("## Quick-look audit\n| File | Lines |"),
            ConversationMessage::user_text("Do a full stack application for the hotel management system and run it"),
            tool_use("TodoWrite", r#"{"todos":[{"content":"Build backend","status":"completed"},{"content":"Run and test the application","status":"in_progress"},{"content":"Docker setup","status":"pending"}]}"#),
            tool_use("write_file", r#"{"path":"C:\\Users\\rahul\\AI102\\backend\\api\\__init__.py","content":"SECRET"}"#),
            tool_use("write_file", r#"{"path":"C:\\Users\\rahul\\AI102\\.env","content":"API_KEY=sk-live-abc"}"#),
            tool_use("edit_file", r#"{"path":"C:\\Users\\rahul\\AI102\\backend\\api\\__init__.py","old_string":"a","new_string":"b"}"#),
            tool_use("read_file", r#"{"path":"C:\\Users\\rahul\\AI102\\README.md"}"#),
            assistant_text("## Summary\nI've created a full-stack Hotel Management System."),
            tool_use("bash", r#"{"command":"python -c \"import backend.api\""}"#),
            tool_error("bash", r#"{"stdout":"","stderr":"Traceback (most recent call last):\r\n  File \"backend/api/__init__.py\", line 53\r\nNameError: RoomStatus"}"#),
            assistant_text("[response failed: This request needs 84,937 tokens but only 74,329 of your 1,000,000 tokens for this 5-hour window are left (insufficient_quota); resets at 2026-11-04T18:48:59.950Z]"),
        ];
        session
    }

    #[test]
    fn a_session_recap_keeps_requests_files_open_todos_and_the_failure() {
        let root = PathBuf::from(r"C:\Users\rahul\AI102");
        let recap = recap_session(&hotel_session(), Some(&root)).expect("recap");

        assert_eq!(recap.prompts.len(), 2);
        assert!(recap.prompts[1].starts_with("Do a full stack application"));
        if cfg!(windows) {
            assert_eq!(recap.files_changed, vec!["backend/api/__init__.py", ".env"]);
        }
        assert_eq!(
            recap.files_changed_total, 2,
            "a file edited twice counts once"
        );
        assert_eq!(
            recap.open_todos,
            vec![
                "Run and test the application (in_progress)",
                "Docker setup (pending)"
            ]
        );
        assert_eq!(
            recap.last_reply.as_deref(),
            Some("## Summary I've created a full-stack Hotel Management System."),
            "the failure note is not the last answer"
        );
        match &recap.ending {
            SessionEnding::RequestFailed(error) => {
                assert!(error.contains("insufficient_quota"), "{error}");
            }
            other => panic!("expected a failed request, got {other:?}"),
        }
    }

    #[test]
    fn the_prompt_section_never_carries_file_contents() {
        let recap = recap_session(&hotel_session(), None).expect("recap");
        let section = recap.prompt_section(26 * HOUR);
        assert!(section.starts_with(RECAP_SECTION_HEADING));
        assert!(section.contains("16h ago"), "{section}");
        assert!(section.contains("/resume session-1"));
        assert!(section.contains("Run and test the application (in_progress)"));
        assert!(section.contains("the last request failed"));
        assert!(!section.contains("SECRET"));
        assert!(!section.contains("sk-live-abc"));
        assert!(
            !section.contains("Quick-look"),
            "only the last answer is quoted"
        );
    }

    #[test]
    fn the_notice_names_the_request_changes_and_ending() {
        let recap = recap_session(&hotel_session(), None).expect("recap");
        let notice = recap.notice(10 * HOUR + 5 * 60_000);
        assert!(
            notice.starts_with("Last session here (5m ago): \"Do a full stack"),
            "{notice}"
        );
        assert!(notice.contains("2 files changed"));
        assert!(notice.contains("2 todos open"));
        assert!(notice.contains("ended on a failed request"));
        assert!(notice.ends_with("/resume session-1 reopens that chat."));
    }

    #[test]
    fn a_tool_error_after_the_last_answer_is_the_ending() {
        let mut session = hotel_session();
        session.messages.pop();
        let recap = recap_session(&session, None).expect("recap");
        match recap.ending {
            SessionEnding::ToolFailed(error) => {
                assert!(error.starts_with("bash: Traceback"), "{error}");
                assert!(!error.contains('\n'));
            }
            other => panic!("expected a tool failure, got {other:?}"),
        }

        session
            .messages
            .push(assistant_text("Fixed it; the import works now."));
        let answered = recap_session(&session, None).expect("recap");
        assert_eq!(answered.ending, SessionEnding::Answered);
        assert!(answered.prompt_section(0).contains("the model answered"));
    }

    #[test]
    fn a_session_without_a_request_has_no_recap() {
        let mut session = Session::new();
        assert_eq!(recap_session(&session, None), None);
        session.messages.push(ConversationMessage::user_text("   "));
        assert_eq!(recap_session(&session, None), None);
    }

    #[test]
    fn only_the_last_three_requests_are_kept_and_long_ones_are_cut() {
        let mut session = Session::new();
        for index in 0..5 {
            session
                .messages
                .push(ConversationMessage::user_text(format!("request {index}")));
        }
        session
            .messages
            .push(ConversationMessage::user_text("x".repeat(500)));
        let recap = recap_session(&session, None).expect("recap");
        assert_eq!(recap.prompts.len(), 3);
        assert_eq!(recap.prompts[0], "request 3");
        assert_eq!(recap.prompts[2].chars().count(), MAX_PROMPT_CHARS);
        assert!(recap.prompts[2].ends_with('…'));
    }

    #[test]
    fn latest_recap_skips_the_current_empty_rotated_and_stale_sessions() {
        let dir = std::env::temp_dir().join(format!(
            "neuron-recap-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let now = 20 * 24 * HOUR;

        let mut old = hotel_session();
        old.session_id = "session-old".into();
        old.updated_at_ms = now - 15 * 24 * HOUR;
        old.save_to_path(dir.join("session-old.jsonl")).unwrap();

        let mut previous = hotel_session();
        previous.session_id = "session-prev".into();
        previous.updated_at_ms = now - 2 * HOUR;
        previous
            .save_to_path(dir.join("session-prev.jsonl"))
            .unwrap();
        fs::write(dir.join("session-prev.rot-1.jsonl"), "not json").unwrap();

        let mut empty = Session::new();
        empty.session_id = "session-empty".into();
        empty.save_to_path(dir.join("session-empty.jsonl")).unwrap();

        let mut current = hotel_session();
        current.session_id = "session-now".into();
        current.updated_at_ms = now;
        current.save_to_path(dir.join("session-now.jsonl")).unwrap();

        let recap = latest_recap(&dir, "session-now", None, now).expect("recap");
        assert_eq!(recap.session_id, "session-prev");

        fs::remove_file(dir.join("session-prev.jsonl")).unwrap();
        assert_eq!(
            latest_recap(&dir, "session-now", None, now),
            None,
            "a session older than two weeks is left out"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ages_read_in_minutes_hours_and_days() {
        assert_eq!(age_label(0, 30_000), "just now");
        assert_eq!(age_label(0, 59 * 60_000), "59m ago");
        assert_eq!(age_label(0, 16 * HOUR), "16h ago");
        assert_eq!(age_label(0, 72 * HOUR), "3d ago");
    }
}
