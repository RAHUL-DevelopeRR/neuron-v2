use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::context_manager::{estimate_message_tokens, estimate_text_tokens, truncate_head_tail};
use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};

const COMPACT_CONTINUATION_PREAMBLE: &str =
    "This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.\n\n";
const COMPACT_RECENT_MESSAGES_NOTE: &str = "Recent messages are preserved verbatim.";
const COMPACT_DIRECT_RESUME_INSTRUCTION: &str = "Continue the conversation from where it left off without asking the user any further questions. Resume directly — do not acknowledge the summary, do not recap what was happening, and do not preface with continuation text.";
/// Label placed at the top of every generated summary.
pub const DERIVED_CONTEXT_NOTE: &str = "Note: this summary is derived context generated automatically from earlier messages. It can be incomplete or stale; verify file contents, paths and decisions against the workspace before relying on them.";
/// Prefix of the synthetic user message the runtime sends when a response was
/// cut off by the output limit. Such messages are not real user requests.
pub const AUTO_CONTINUE_PREFIX: &str = "[auto-continue]";

/// Maximum characters of the latest user request kept in the summary.
const LATEST_REQUEST_MAX_CHARS: usize = 20_000;
const EARLIER_REQUEST_MAX_CHARS: usize = 600;
const EARLIER_REQUEST_LIMIT: usize = 10;
const CONCLUSION_MAX_CHARS: usize = 400;
const CONCLUSION_LIMIT: usize = 8;
const ERROR_MAX_CHARS: usize = 240;
const ERROR_LIMIT: usize = 10;
const OUTCOME_LIMIT: usize = 12;
const OUTCOME_DETAIL_CHARS: usize = 160;
const PATH_LIMIT: usize = 60;
const TIMELINE_LIMIT: usize = 16;
/// Characters kept per block in the summary timeline.
pub(crate) const TIMELINE_SNIPPET_CHARS: usize = 320;
/// Characters of a previous summary carried into a merged summary.
const PREVIOUS_SUMMARY_MAX_CHARS: usize = 8_000;
/// Default cap on the estimated size of a generated summary.
pub const DEFAULT_MAX_SUMMARY_TOKENS: usize = 6_000;

/// Thresholds controlling when and how a session is compacted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionConfig {
    pub preserve_recent_messages: usize,
    pub max_estimated_tokens: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            preserve_recent_messages: 4,
            max_estimated_tokens: 10_000,
        }
    }
}

/// Result of compacting a session into a summary plus preserved tail messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionResult {
    pub summary: String,
    pub formatted_summary: String,
    pub compacted_session: Session,
    pub removed_message_count: usize,
}

impl CompactionResult {
    pub(crate) fn unchanged(session: &Session) -> Self {
        Self {
            summary: String::new(),
            formatted_summary: String::new(),
            compacted_session: session.clone(),
            removed_message_count: 0,
        }
    }
}

/// Estimates the token footprint of the current session transcript.
#[must_use]
pub fn estimate_session_tokens(session: &Session) -> usize {
    session.messages.iter().map(estimate_message_tokens).sum()
}

/// Returns `true` when the session exceeds the configured compaction budget.
#[must_use]
pub fn should_compact(session: &Session, config: CompactionConfig) -> bool {
    let start = compacted_summary_prefix_len(session);
    let compactable = &session.messages[start..];

    compactable.len() > config.preserve_recent_messages
        && compactable
            .iter()
            .map(estimate_message_tokens)
            .sum::<usize>()
            >= config.max_estimated_tokens
}

/// Normalizes a compaction summary into user-facing continuation text.
#[must_use]
pub fn format_compact_summary(summary: &str) -> String {
    let without_analysis = strip_tag_block(summary, "analysis");
    let formatted = if let Some(content) = extract_tag_block(&without_analysis, "summary") {
        without_analysis.replace(
            &format!("<summary>{content}</summary>"),
            &format!("Summary:\n{}", content.trim()),
        )
    } else {
        without_analysis
    };

    collapse_blank_lines(&formatted).trim().to_string()
}

/// Builds the synthetic system message used after session compaction.
#[must_use]
pub fn get_compact_continuation_message(
    summary: &str,
    suppress_follow_up_questions: bool,
    recent_messages_preserved: bool,
) -> String {
    let mut base = format!(
        "{COMPACT_CONTINUATION_PREAMBLE}{}",
        format_compact_summary(summary)
    );

    if recent_messages_preserved {
        base.push_str("\n\n");
        base.push_str(COMPACT_RECENT_MESSAGES_NOTE);
    }

    if suppress_follow_up_questions {
        base.push('\n');
        base.push_str(COMPACT_DIRECT_RESUME_INSTRUCTION);
    }

    base
}

/// Compacts a session by summarizing older messages and preserving the most
/// recent `config.preserve_recent_messages` messages (extended backwards so a
/// tool-use / tool-result pair is never split).
#[must_use]
pub fn compact_session(session: &Session, config: CompactionConfig) -> CompactionResult {
    if !should_compact(session, config) {
        return CompactionResult::unchanged(session);
    }
    let prefix_len = compacted_summary_prefix_len(session);
    let raw_keep_from = session
        .messages
        .len()
        .saturating_sub(config.preserve_recent_messages)
        .max(prefix_len);
    let keep_from = pair_safe_boundary(&session.messages, raw_keep_from, prefix_len);
    compact_at(session, prefix_len, keep_from, DEFAULT_MAX_SUMMARY_TOKENS)
}

/// Token-budgeted compaction policy (see [`compact_session_with_policy`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenCompaction {
    /// Minimum number of recent messages kept verbatim.
    pub min_recent_messages: usize,
    /// Estimated tokens of recent messages kept verbatim.
    pub recent_tokens_budget: usize,
    /// Upper bound for the generated summary; the timeline is dropped first,
    /// then the summary body is head+tail truncated.
    pub max_summary_tokens: usize,
}

/// Compacts a session keeping the most recent messages whose estimated size
/// fits `recent_tokens_budget` (but at least `min_recent_messages`), without
/// splitting tool-use / tool-result pairs. This is the policy used by the
/// automatic context manager and by interactive `/compact`.
#[must_use]
pub fn compact_session_to_token_budget(
    session: &Session,
    min_recent_messages: usize,
    recent_tokens_budget: usize,
) -> CompactionResult {
    compact_session_with_policy(
        session,
        TokenCompaction {
            min_recent_messages,
            recent_tokens_budget,
            max_summary_tokens: DEFAULT_MAX_SUMMARY_TOKENS,
        },
    )
}

/// Token-budgeted compaction with an explicit summary size cap.
#[must_use]
pub fn compact_session_with_policy(session: &Session, policy: TokenCompaction) -> CompactionResult {
    let prefix_len = compacted_summary_prefix_len(session);
    let messages = &session.messages;
    let mut keep_from = messages.len();
    let mut kept_tokens = 0_usize;
    while keep_from > prefix_len {
        let candidate = &messages[keep_from - 1];
        let candidate_tokens = estimate_message_tokens(candidate);
        let kept_count = messages.len() - keep_from;
        if kept_count >= policy.min_recent_messages
            && kept_tokens + candidate_tokens > policy.recent_tokens_budget
        {
            break;
        }
        kept_tokens += candidate_tokens;
        keep_from -= 1;
    }
    let keep_from = pair_safe_boundary(messages, keep_from, prefix_len);
    if keep_from <= prefix_len {
        return CompactionResult::unchanged(session);
    }
    compact_at(session, prefix_len, keep_from, policy.max_summary_tokens)
}
/// Moves `keep_from` backwards until the preserved tail does not start with a
/// tool result whose tool use would be summarized away. Never moves below
/// `prefix_len` and never indexes out of bounds (including `keep_from == len`).
fn pair_safe_boundary(
    messages: &[ConversationMessage],
    keep_from: usize,
    prefix_len: usize,
) -> usize {
    let mut boundary = keep_from.min(messages.len());
    while boundary > prefix_len && boundary < messages.len() {
        if !is_tool_result_message(&messages[boundary]) {
            break;
        }
        boundary -= 1;
    }
    boundary
}

fn is_tool_result_message(message: &ConversationMessage) -> bool {
    message
        .blocks
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
}

fn compact_at(
    session: &Session,
    prefix_len: usize,
    keep_from: usize,
    max_summary_tokens: usize,
) -> CompactionResult {
    if keep_from <= prefix_len {
        return CompactionResult::unchanged(session);
    }
    let existing_summary = session
        .messages
        .first()
        .and_then(extract_existing_compacted_summary);
    let removed = &session.messages[prefix_len..keep_from];
    let preserved = session.messages[keep_from..].to_vec();
    let latest_request_preserved = preserved.iter().any(is_user_request);
    let summary = enforce_summary_budget(
        merge_compact_summaries(
            existing_summary.as_deref(),
            &summarize_messages_with_context(removed, latest_request_preserved),
        ),
        max_summary_tokens,
    );
    let formatted_summary = format_compact_summary(&summary);
    let continuation = get_compact_continuation_message(&summary, true, !preserved.is_empty());

    let mut compacted_messages = vec![ConversationMessage {
        role: MessageRole::System,
        blocks: vec![ContentBlock::Text { text: continuation }],
        usage: None,
    }];
    compacted_messages.extend(preserved);

    let mut compacted_session = session.clone();
    compacted_session.messages = compacted_messages;
    compacted_session.record_compaction(summary.clone(), removed.len());

    CompactionResult {
        summary,
        formatted_summary,
        compacted_session,
        removed_message_count: removed.len(),
    }
}

/// Keeps a generated summary within `max_tokens`: the timeline is dropped
/// first, then the body is head+tail truncated (the latest user request and
/// the derived-context note sit at the top and survive). The `<summary>` tags
/// are preserved.
fn enforce_summary_budget(summary: String, max_tokens: usize) -> String {
    if estimate_text_tokens(&summary) <= max_tokens {
        return summary;
    }
    let without_timeline = match summary.find("\n- Key timeline:") {
        Some(index) => format!("{}\n</summary>", &summary[..index]),
        None => summary,
    };
    if estimate_text_tokens(&without_timeline) <= max_tokens {
        return without_timeline;
    }
    let body = extract_tag_block(&without_timeline, "summary").unwrap_or(without_timeline);
    let mut truncated = truncate_head_tail(&body, max_tokens.saturating_mul(4));
    if estimate_text_tokens(&truncated) > max_tokens {
        truncated = truncate_head_tail(&body, max_tokens);
    }
    format!("<summary>{truncated}\n</summary>")
}

fn compacted_summary_prefix_len(session: &Session) -> usize {
    usize::from(
        session
            .messages
            .first()
            .and_then(extract_existing_compacted_summary)
            .is_some(),
    )
}

fn summarize_messages(messages: &[ConversationMessage]) -> String {
    summarize_messages_with_context(messages, false)
}

/// Builds the deterministic structured summary for `messages`.
/// `latest_request_preserved` tells whether a newer user request survives in
/// the verbatim tail (in which case it is not repeated here).
fn summarize_messages_with_context(
    messages: &[ConversationMessage],
    latest_request_preserved: bool,
) -> String {
    let count_role = |role: MessageRole| {
        messages
            .iter()
            .filter(|message| message.role == role)
            .count()
    };
    let mut lines = vec![
        "<summary>".to_string(),
        "Conversation summary:".to_string(),
        format!("- {DERIVED_CONTEXT_NOTE}"),
        format!(
            "- Scope: {} earlier messages compacted (user={}, assistant={}, tool={}).",
            messages.len(),
            count_role(MessageRole::User),
            count_role(MessageRole::Assistant),
            count_role(MessageRole::Tool),
        ),
    ];

    let tool_names = messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolUse { name, .. } => Some(name.as_str()),
            ContentBlock::ToolResult { tool_name, .. } => Some(tool_name.as_str()),
            ContentBlock::Text { .. } => None,
        })
        .collect::<BTreeSet<_>>();
    if !tool_names.is_empty() {
        lines.push(format!(
            "- Tools mentioned: {}.",
            tool_names.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }

    push_user_requests(&mut lines, messages, latest_request_preserved);
    push_touched_paths(&mut lines, messages);

    let key_files = collect_key_files(messages);
    if !key_files.is_empty() {
        lines.push(format!("- Key files referenced: {}.", key_files.join(", ")));
    }

    push_section(
        &mut lines,
        "Decisions and conclusions",
        &collect_conclusions(messages),
    );
    push_section(&mut lines, "Errors encountered", &collect_errors(messages));
    let todos = collect_pending_todos(messages);
    if todos.is_empty() {
        push_section(&mut lines, "Pending work", &infer_pending_work(messages));
    } else {
        push_section(&mut lines, "Pending todos", &todos);
    }
    push_section(
        &mut lines,
        "Key tool outcomes",
        &collect_tool_outcomes(messages),
    );

    if let Some(current_work) = infer_current_work(messages) {
        lines.push(format!("- Current work: {current_work}"));
    }

    lines.push("- Key timeline:".to_string());
    let skipped = messages.len().saturating_sub(TIMELINE_LIMIT);
    if skipped > 0 {
        lines.push(format!("  - ({skipped} earlier messages omitted)"));
    }
    for message in &messages[skipped..] {
        let role = match message.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        let content = message
            .blocks
            .iter()
            .map(summarize_block)
            .collect::<Vec<_>>()
            .join(" | ");
        lines.push(format!("  - {role}: {}", single_line(&content)));
    }
    lines.push("</summary>".to_string());
    lines.join("\n")
}

fn push_section(lines: &mut Vec<String>, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    lines.push(format!("- {title}:"));
    lines.extend(
        items
            .iter()
            .map(|item| format!("  - {}", single_line(item))),
    );
}

fn push_user_requests(
    lines: &mut Vec<String>,
    messages: &[ConversationMessage],
    latest_request_preserved: bool,
) {
    let mut requests = messages
        .iter()
        .filter(|message| is_user_request(message))
        .filter_map(first_text_block)
        .collect::<Vec<_>>();
    if !latest_request_preserved {
        if let Some(latest) = requests.pop() {
            lines.push("- Latest user request (verbatim):".to_string());
            let latest = truncate_head_tail(latest.trim(), LATEST_REQUEST_MAX_CHARS);
            lines.extend(latest.lines().map(|line| format!("  > {line}")));
        }
    }
    if requests.is_empty() {
        return;
    }
    lines.push("- Earlier user requests:".to_string());
    let omitted = requests.len().saturating_sub(EARLIER_REQUEST_LIMIT);
    if omitted > 0 {
        lines.push(format!("  - ({omitted} older requests omitted)"));
    }
    lines.extend(requests[omitted..].iter().map(|request| {
        format!(
            "  - {}",
            single_line(&truncate_summary(request, EARLIER_REQUEST_MAX_CHARS))
        )
    }));
}

fn is_user_request(message: &ConversationMessage) -> bool {
    message.role == MessageRole::User
        && first_text_block(message).is_some_and(|text| !text.starts_with(AUTO_CONTINUE_PREFIX))
}

const MODIFYING_TOOLS: &[&str] = &[
    "write_file",
    "edit_file",
    "NotebookEdit",
    "Write",
    "Edit",
    "MultiEdit",
];
const READING_TOOLS: &[&str] = &["read_file", "Read"];
const PATH_KEYS: &[&str] = &["path", "file_path", "filePath", "notebook_path"];

fn push_touched_paths(lines: &mut Vec<String>, messages: &[ConversationMessage]) {
    let mut modified = BTreeSet::new();
    let mut read = BTreeSet::new();
    let mut other = BTreeSet::new();
    for block in messages.iter().flat_map(|message| message.blocks.iter()) {
        let ContentBlock::ToolUse { name, input, .. } = block else {
            continue;
        };
        let Ok(Value::Object(object)) = serde_json::from_str::<Value>(input) else {
            continue;
        };
        for key in PATH_KEYS {
            let Some(path) = object.get(*key).and_then(Value::as_str) else {
                continue;
            };
            let path = path.trim().to_string();
            if path.is_empty() {
                continue;
            }
            if MODIFYING_TOOLS.contains(&name.as_str()) {
                modified.insert(path);
            } else if READING_TOOLS.contains(&name.as_str()) {
                read.insert(path);
            } else {
                other.insert(path);
            }
        }
    }
    for (label, paths) in [
        ("Files modified", modified),
        ("Files read", read),
        ("Other paths used by tools", other),
    ] {
        if paths.is_empty() {
            continue;
        }
        let total = paths.len();
        let mut listed = paths.into_iter().take(PATH_LIMIT).collect::<Vec<_>>();
        if total > PATH_LIMIT {
            listed.push(format!("(+{} more)", total - PATH_LIMIT));
        }
        lines.push(format!("- {label}: {}.", listed.join(", ")));
    }
}

fn collect_conclusions(messages: &[ConversationMessage]) -> Vec<String> {
    let conclusions = messages
        .iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::Text { text } if !text.trim().is_empty() => {
                Some(truncate_summary(text.trim(), CONCLUSION_MAX_CHARS))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    last_n(conclusions, CONCLUSION_LIMIT)
}

fn collect_errors(messages: &[ConversationMessage]) -> Vec<String> {
    let errors = messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_name,
                output,
                is_error: true,
                ..
            } => Some(format!(
                "{tool_name}: {}",
                truncate_summary(output.trim(), ERROR_MAX_CHARS)
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    last_n(errors, ERROR_LIMIT)
}

fn collect_pending_todos(messages: &[ConversationMessage]) -> Vec<String> {
    let Some(todos) = messages
        .iter()
        .rev()
        .flat_map(|message| message.blocks.iter().rev())
        .find_map(|block| match block {
            ContentBlock::ToolUse { name, input, .. } if name == "TodoWrite" => {
                serde_json::from_str::<Value>(input)
                    .ok()?
                    .get("todos")?
                    .as_array()
                    .cloned()
            }
            _ => None,
        })
    else {
        return Vec::new();
    };
    todos
        .iter()
        .filter_map(|todo| {
            let status = todo
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending");
            if status.eq_ignore_ascii_case("completed") {
                return None;
            }
            let content = ["content", "text", "title"]
                .iter()
                .find_map(|key| todo.get(*key).and_then(Value::as_str))?;
            Some(format!("[{status}] {}", truncate_summary(content, 200)))
        })
        .collect()
}

fn collect_tool_outcomes(messages: &[ConversationMessage]) -> Vec<String> {
    let inputs = messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, input, .. } => Some((id.as_str(), input.as_str())),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let outcomes = messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                tool_name,
                output,
                is_error: false,
            } => {
                let brief = inputs
                    .get(tool_use_id.as_str())
                    .copied()
                    .map(brief_tool_input)
                    .unwrap_or_default();
                let first_line = output
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .unwrap_or("(no output)");
                Some(format!(
                    "{tool_name}{brief} → {}",
                    truncate_summary(first_line, OUTCOME_DETAIL_CHARS)
                ))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    last_n(outcomes, OUTCOME_LIMIT)
}

fn brief_tool_input(input: &str) -> String {
    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(input) else {
        return String::new();
    };
    [
        "command",
        "path",
        "file_path",
        "pattern",
        "query",
        "url",
        "description",
    ]
    .iter()
    .find_map(|key| object.get(*key).and_then(Value::as_str))
    .map(|value| format!("({})", truncate_summary(&single_line(value), 100)))
    .unwrap_or_default()
}

fn last_n(items: Vec<String>, limit: usize) -> Vec<String> {
    let skip = items.len().saturating_sub(limit);
    items.into_iter().skip(skip).collect()
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn merge_compact_summaries(existing_summary: Option<&str>, new_summary: &str) -> String {
    let Some(existing_summary) = existing_summary else {
        return new_summary.to_string();
    };

    let previous_highlights = truncate_head_tail(
        &extract_summary_highlights(existing_summary).join("\n"),
        PREVIOUS_SUMMARY_MAX_CHARS,
    );
    let new_formatted_summary = format_compact_summary(new_summary);
    let new_highlights = extract_summary_highlights(&new_formatted_summary);
    let new_timeline = extract_summary_timeline(&new_formatted_summary);

    let mut lines = vec![
        "<summary>".to_string(),
        "Conversation summary:".to_string(),
        format!("- {DERIVED_CONTEXT_NOTE}"),
    ];

    let previous_highlights = previous_highlights
        .lines()
        .filter(|line| !line.contains(DERIVED_CONTEXT_NOTE))
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>();
    if !previous_highlights.is_empty() {
        lines.push("- Previously compacted context:".to_string());
        lines.extend(previous_highlights);
    }

    let new_highlights = new_highlights
        .into_iter()
        .filter(|line| !line.contains(DERIVED_CONTEXT_NOTE))
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>();
    if !new_highlights.is_empty() {
        lines.push("- Newly compacted context:".to_string());
        lines.extend(new_highlights);
    }

    if !new_timeline.is_empty() {
        lines.push("- Key timeline:".to_string());
        lines.extend(new_timeline.into_iter().map(|line| format!("  {line}")));
    }

    lines.push("</summary>".to_string());
    lines.join("\n")
}

fn summarize_block(block: &ContentBlock) -> String {
    let raw = match block {
        ContentBlock::Text { text } => text.clone(),
        ContentBlock::ToolUse { name, input, .. } => format!("tool_use {name}({input})"),
        ContentBlock::ToolResult {
            tool_name,
            output,
            is_error,
            ..
        } => format!(
            "tool_result {tool_name}: {}{output}",
            if *is_error { "error " } else { "" }
        ),
    };
    truncate_summary(&raw, TIMELINE_SNIPPET_CHARS)
}

fn collect_recent_role_summaries(
    messages: &[ConversationMessage],
    role: MessageRole,
    limit: usize,
) -> Vec<String> {
    messages
        .iter()
        .filter(|message| message.role == role)
        .rev()
        .filter_map(|message| first_text_block(message))
        .take(limit)
        .map(|text| truncate_summary(text, EARLIER_REQUEST_MAX_CHARS))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn infer_pending_work(messages: &[ConversationMessage]) -> Vec<String> {
    messages
        .iter()
        .rev()
        .filter_map(first_text_block)
        .filter(|text| {
            let lowered = text.to_ascii_lowercase();
            lowered.contains("todo")
                || lowered.contains("next")
                || lowered.contains("pending")
                || lowered.contains("follow up")
                || lowered.contains("remaining")
        })
        .take(3)
        .map(|text| truncate_summary(text, CONCLUSION_MAX_CHARS))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn collect_key_files(messages: &[ConversationMessage]) -> Vec<String> {
    let mut files = messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .map(|block| match block {
            ContentBlock::Text { text } => text.as_str(),
            ContentBlock::ToolUse { input, .. } => input.as_str(),
            ContentBlock::ToolResult { output, .. } => output.as_str(),
        })
        .flat_map(extract_file_candidates)
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    files.into_iter().take(12).collect()
}

fn infer_current_work(messages: &[ConversationMessage]) -> Option<String> {
    messages
        .iter()
        .rev()
        .filter_map(first_text_block)
        .find(|text| !text.trim().is_empty())
        .map(|text| truncate_summary(text, CONCLUSION_MAX_CHARS))
}

fn first_text_block(message: &ConversationMessage) -> Option<&str> {
    message.blocks.iter().find_map(|block| match block {
        ContentBlock::Text { text } if !text.trim().is_empty() => Some(text.as_str()),
        ContentBlock::ToolUse { .. }
        | ContentBlock::ToolResult { .. }
        | ContentBlock::Text { .. } => None,
    })
}

fn has_interesting_extension(candidate: &str) -> bool {
    std::path::Path::new(candidate)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            [
                "rs", "ts", "tsx", "js", "jsx", "json", "md", "toml", "py", "go", "java", "c", "h",
                "cpp", "yaml", "yml", "html", "css",
            ]
            .iter()
            .any(|expected| extension.eq_ignore_ascii_case(expected))
        })
}

fn extract_file_candidates(content: &str) -> Vec<String> {
    content
        .split_whitespace()
        .filter_map(|token| {
            let candidate = token.trim_matches(|char: char| {
                matches!(char, ',' | '.' | ':' | ';' | ')' | '(' | '"' | '\'' | '`')
            });
            if (candidate.contains('/') || candidate.contains('\\'))
                && has_interesting_extension(candidate)
            {
                Some(candidate.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn truncate_summary(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }
    let mut truncated = content.chars().take(max_chars).collect::<String>();
    truncated.push('…');
    truncated
}

fn extract_tag_block(content: &str, tag: &str) -> Option<String> {
    let start = format!("<{tag}>");
    let end = format!("</{tag}>");
    let start_index = content.find(&start)? + start.len();
    let end_index = content[start_index..].find(&end)? + start_index;
    Some(content[start_index..end_index].to_string())
}

fn strip_tag_block(content: &str, tag: &str) -> String {
    let start = format!("<{tag}>");
    let end = format!("</{tag}>");
    if let (Some(start_index), Some(end_index_rel)) = (content.find(&start), content.find(&end)) {
        let end_index = end_index_rel + end.len();
        let mut stripped = String::new();
        stripped.push_str(&content[..start_index]);
        stripped.push_str(&content[end_index..]);
        stripped
    } else {
        content.to_string()
    }
}

fn collapse_blank_lines(content: &str) -> String {
    let mut result = String::new();
    let mut last_blank = false;
    for line in content.lines() {
        let is_blank = line.trim().is_empty();
        if is_blank && last_blank {
            continue;
        }
        result.push_str(line);
        result.push('\n');
        last_blank = is_blank;
    }
    result
}

fn extract_existing_compacted_summary(message: &ConversationMessage) -> Option<String> {
    if message.role != MessageRole::System {
        return None;
    }

    let text = first_text_block(message)?;
    let summary = text.strip_prefix(COMPACT_CONTINUATION_PREAMBLE)?;
    let summary = summary
        .split_once(&format!("\n\n{COMPACT_RECENT_MESSAGES_NOTE}"))
        .map_or(summary, |(value, _)| value);
    let summary = summary
        .split_once(&format!("\n{COMPACT_DIRECT_RESUME_INSTRUCTION}"))
        .map_or(summary, |(value, _)| value);
    Some(summary.trim().to_string())
}

fn extract_summary_highlights(summary: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut in_timeline = false;

    for line in format_compact_summary(summary).lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed == "Summary:" || trimmed == "Conversation summary:" {
            continue;
        }
        if trimmed == "- Key timeline:" {
            in_timeline = true;
            continue;
        }
        if in_timeline {
            continue;
        }
        lines.push(trimmed.to_string());
    }

    lines
}

fn extract_summary_timeline(summary: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut in_timeline = false;

    for line in format_compact_summary(summary).lines() {
        let trimmed = line.trim_end();
        if trimmed == "- Key timeline:" {
            in_timeline = true;
            continue;
        }
        if !in_timeline {
            continue;
        }
        if trimmed.is_empty() {
            break;
        }
        lines.push(trimmed.to_string());
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::{
        collect_key_files, compact_session, compact_session_to_token_budget,
        format_compact_summary, get_compact_continuation_message, infer_pending_work,
        should_compact, CompactionConfig, AUTO_CONTINUE_PREFIX, DERIVED_CONTEXT_NOTE,
        TIMELINE_SNIPPET_CHARS,
    };
    use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};

    #[test]
    fn formats_compact_summary_like_upstream() {
        let summary = "<analysis>scratch</analysis>\n<summary>Kept work</summary>";
        assert_eq!(format_compact_summary(summary), "Summary:\nKept work");
    }

    #[test]
    fn leaves_small_sessions_unchanged() {
        let mut session = Session::new();
        session.messages = vec![ConversationMessage::user_text("hello")];

        let result = compact_session(&session, CompactionConfig::default());
        assert_eq!(result.removed_message_count, 0);
        assert_eq!(result.compacted_session, session);
        assert_eq!(result.summary, "");
        assert_eq!(result.formatted_summary, "");
    }

    #[test]
    fn compacts_older_messages_into_a_system_summary() {
        let mut session = Session::new();
        session.messages = vec![
            ConversationMessage::user_text("one ".repeat(200)),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "two ".repeat(200),
            }]),
            ConversationMessage::tool_result("1", "bash", "ok ".repeat(200), false),
            ConversationMessage {
                role: MessageRole::Assistant,
                blocks: vec![ContentBlock::Text {
                    text: "recent".to_string(),
                }],
                usage: None,
            },
        ];

        let result = compact_session(
            &session,
            CompactionConfig {
                preserve_recent_messages: 2,
                max_estimated_tokens: 1,
            },
        );

        // With the tool-use/tool-result boundary fix, the compaction preserves
        // one extra message to avoid an orphaned tool result at the boundary.
        // messages[1] (assistant) must be kept along with messages[2] (tool result).
        assert!(
            result.removed_message_count <= 2,
            "expected at most 2 removed, got {}",
            result.removed_message_count
        );
        assert_eq!(
            result.compacted_session.messages[0].role,
            MessageRole::System
        );
        assert!(matches!(
            &result.compacted_session.messages[0].blocks[0],
            ContentBlock::Text { text } if text.contains("Summary:")
        ));
        assert!(result.formatted_summary.contains("Scope:"));
        assert!(result.formatted_summary.contains("Key timeline:"));
        assert!(should_compact(
            &session,
            CompactionConfig {
                preserve_recent_messages: 2,
                max_estimated_tokens: 1,
            }
        ));
        // Note: with the tool-use/tool-result boundary guard the compacted session
        // may preserve one extra message at the boundary, so token reduction is
        // not guaranteed for small sessions. The invariant that matters is that
        // the removed_message_count is non-zero (something was compacted).
        assert!(
            result.removed_message_count > 0,
            "compaction must remove at least one message"
        );
    }

    #[test]
    fn keeps_previous_compacted_context_when_compacting_again() {
        let mut initial_session = Session::new();
        initial_session.messages = vec![
            ConversationMessage::user_text("Investigate rust/crates/runtime/src/compact.rs"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "I will inspect the compact flow.".to_string(),
            }]),
            ConversationMessage::user_text("Also update rust/crates/runtime/src/conversation.rs"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "Next: preserve prior summary context during auto compact.".to_string(),
            }]),
        ];
        let config = CompactionConfig {
            preserve_recent_messages: 2,
            max_estimated_tokens: 1,
        };

        let first = compact_session(&initial_session, config);
        let mut follow_up_messages = first.compacted_session.messages.clone();
        follow_up_messages.extend([
            ConversationMessage::user_text("Please add regression tests for compaction."),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "Working on regression coverage now.".to_string(),
            }]),
        ]);

        let mut second_session = Session::new();
        second_session.messages = follow_up_messages;
        let second = compact_session(&second_session, config);

        assert!(second
            .formatted_summary
            .contains("Previously compacted context:"));
        assert!(second
            .formatted_summary
            .contains("Scope: 2 earlier messages compacted"));
        assert!(second
            .formatted_summary
            .contains("Newly compacted context:"));
        assert!(second
            .formatted_summary
            .contains("Also update rust/crates/runtime/src/conversation.rs"));
        assert!(matches!(
            &second.compacted_session.messages[0].blocks[0],
            ContentBlock::Text { text }
                if text.contains("Previously compacted context:")
                    && text.contains("Newly compacted context:")
        ));
        assert!(matches!(
            &second.compacted_session.messages[1].blocks[0],
            ContentBlock::Text { text } if text.contains("Please add regression tests for compaction.")
        ));
        // The derived-context label appears exactly once even after merging.
        assert_eq!(
            second
                .formatted_summary
                .matches(DERIVED_CONTEXT_NOTE)
                .count(),
            1
        );
    }

    #[test]
    fn ignores_existing_compacted_summary_when_deciding_to_recompact() {
        let summary = "<summary>Conversation summary:\n- Scope: earlier work preserved.\n- Key timeline:\n  - user: large preserved context\n</summary>";
        let mut session = Session::new();
        session.messages = vec![
            ConversationMessage {
                role: MessageRole::System,
                blocks: vec![ContentBlock::Text {
                    text: get_compact_continuation_message(summary, true, true),
                }],
                usage: None,
            },
            ConversationMessage::user_text("tiny"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "recent".to_string(),
            }]),
        ];

        assert!(!should_compact(
            &session,
            CompactionConfig {
                preserve_recent_messages: 2,
                max_estimated_tokens: 1,
            }
        ));
    }

    #[test]
    fn truncates_long_blocks_in_summary() {
        let summary = super::summarize_block(&ContentBlock::Text {
            text: "x".repeat(TIMELINE_SNIPPET_CHARS * 3),
        });
        assert!(summary.ends_with('…'));
        assert!(summary.chars().count() <= TIMELINE_SNIPPET_CHARS + 1);
    }

    #[test]
    fn extracts_key_files_from_message_content() {
        let files = collect_key_files(&[ConversationMessage::user_text(
            "Update rust/crates/runtime/src/compact.rs and rust/crates/rusty-claude-cli/src/main.rs next.",
        )]);
        assert!(files.contains(&"rust/crates/runtime/src/compact.rs".to_string()));
        assert!(files.contains(&"rust/crates/rusty-claude-cli/src/main.rs".to_string()));
    }

    /// Regression: compaction must not split an assistant(ToolUse) /
    /// user(ToolResult) pair at the boundary. An orphaned tool-result message
    /// without the preceding assistant `tool_calls` causes a 400 on the
    /// OpenAI-compat path (gaebal-gajae repro 2026-04-09).
    #[test]
    fn compaction_does_not_split_tool_use_tool_result_pair() {
        use crate::session::{ContentBlock, Session};

        let tool_id = "call_abc";
        let mut session = Session::default();
        // Turn 1: user prompt
        session
            .push_message(ConversationMessage::user_text("Search for files"))
            .unwrap();
        // Turn 2: assistant calls a tool
        session
            .push_message(ConversationMessage::assistant(vec![
                ContentBlock::ToolUse {
                    id: tool_id.to_string(),
                    name: "search".to_string(),
                    input: "{\"q\":\"*.rs\"}".to_string(),
                },
            ]))
            .unwrap();
        // Turn 3: tool result
        session
            .push_message(ConversationMessage::tool_result(
                tool_id,
                "search",
                "found 5 files",
                false,
            ))
            .unwrap();
        // Turn 4: assistant final response
        session
            .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "Done.".to_string(),
            }]))
            .unwrap();

        // Compact preserving only 1 recent message — without the fix this
        // would cut the boundary so that the tool result (turn 3) is first,
        // without its preceding assistant tool_calls (turn 2).
        let config = CompactionConfig {
            preserve_recent_messages: 1,
            ..CompactionConfig::default()
        };
        let result = compact_session(&session, config);
        // After compaction, no two consecutive messages should have the pattern
        // tool_result immediately following a non-assistant message (i.e. an
        // orphaned tool result without a preceding assistant ToolUse).
        let messages = &result.compacted_session.messages;
        for i in 1..messages.len() {
            let curr_is_tool_result = messages[i]
                .blocks
                .first()
                .is_some_and(|b| matches!(b, ContentBlock::ToolResult { .. }));
            if curr_is_tool_result {
                let prev_has_tool_use = messages[i - 1]
                    .blocks
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
                assert!(
                    prev_has_tool_use,
                    "message[{}] is a ToolResult but message[{}] has no ToolUse: {:?}",
                    i,
                    i - 1,
                    messages[i - 1].blocks
                );
            }
        }
    }

    #[test]
    fn infers_pending_work_from_recent_messages() {
        let pending = infer_pending_work(&[
            ConversationMessage::user_text("done"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "Next: update tests and follow up on remaining CLI polish.".to_string(),
            }]),
        ]);
        assert_eq!(pending.len(), 1);
        assert!(pending[0].contains("Next: update tests"));
    }

    #[test]
    fn preserve_zero_compacts_everything_without_panicking() {
        let mut session = Session::new();
        session.messages = vec![
            ConversationMessage::user_text("first request"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "first answer".to_string(),
            }]),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "read_file".to_string(),
                input: r#"{"path":"src/lib.rs"}"#.to_string(),
            }]),
            ConversationMessage::tool_result("t1", "read_file", "fn main() {}", false),
        ];

        let result = compact_session(
            &session,
            CompactionConfig {
                preserve_recent_messages: 0,
                max_estimated_tokens: 0,
            },
        );

        assert_eq!(result.removed_message_count, 4);
        assert_eq!(result.compacted_session.messages.len(), 1);
        assert!(result.formatted_summary.contains("first request"));
    }

    fn long_tool_session() -> Session {
        let mut session = Session::new();
        session.messages.push(ConversationMessage::user_text(
            "Refactor the parser in src/parser.rs",
        ));
        session
            .messages
            .push(ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "todo".to_string(),
                name: "TodoWrite".to_string(),
                input: r#"{"todos":[{"content":"split lexer","activeForm":"splitting","status":"completed"},{"content":"add parser tests","activeForm":"adding","status":"in_progress"}]}"#.to_string(),
            }]));
        session.messages.push(ConversationMessage::tool_result(
            "todo",
            "TodoWrite",
            "ok",
            false,
        ));
        for index in 0..30 {
            let id = format!("call-{index}");
            let (name, input) = if index % 3 == 0 {
                (
                    "edit_file",
                    format!(
                        r#"{{"path":"src/parser_{index}.rs","old_string":"a","new_string":"b"}}"#
                    ),
                )
            } else {
                (
                    "read_file",
                    format!(r#"{{"path":"src/module_{index}.rs"}}"#),
                )
            };
            session.messages.push(ConversationMessage::assistant(vec![
                ContentBlock::Text {
                    text: format!("Step {index}: decided to keep the visitor pattern."),
                },
                ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.to_string(),
                    input,
                },
            ]));
            session.messages.push(ConversationMessage::tool_result(
                id,
                name,
                format!("result {index}\n{}", "line\n".repeat(200)),
                index == 7,
            ));
        }
        session
    }

    #[test]
    fn long_conversation_summary_preserves_structure_and_latest_request() {
        let session = long_tool_session();
        let result = compact_session_to_token_budget(&session, 2, 3_000);

        assert!(result.removed_message_count > 0);
        let summary = &result.formatted_summary;
        assert!(summary.contains(DERIVED_CONTEXT_NOTE));
        assert!(summary.contains("Latest user request (verbatim):"));
        assert!(summary.contains("> Refactor the parser in src/parser.rs"));
        assert!(summary.contains("Files modified:"));
        assert!(summary.contains("src/parser_0.rs"));
        assert!(summary.contains("Files read:"));
        assert!(summary.contains("src/module_1.rs"));
        assert!(summary.contains("Pending todos:"));
        assert!(summary.contains("[in_progress] add parser tests"));
        assert!(!summary.contains("split lexer"));
        assert!(summary.contains("Errors encountered:"));
        assert!(summary.contains("Decisions and conclusions:"));
        assert!(summary.contains("Key tool outcomes:"));

        // The preserved tail never starts with an orphaned tool result.
        let messages = &result.compacted_session.messages;
        assert_eq!(messages[0].role, MessageRole::System);
        assert_ne!(messages[1].role, MessageRole::Tool);
        // The tail stays within the token budget plus one tool pair.
        let tail_tokens = messages[1..]
            .iter()
            .map(crate::context_manager::estimate_message_tokens)
            .sum::<usize>();
        assert!(tail_tokens <= 3_000 + 600, "tail tokens {tail_tokens}");
    }

    #[test]
    fn token_budget_compaction_keeps_more_short_messages_than_long_ones() {
        let mut short = Session::new();
        let mut long = Session::new();
        for index in 0..20 {
            short
                .messages
                .push(ConversationMessage::user_text(format!("short {index}")));
            long.messages.push(ConversationMessage::user_text(format!(
                "long {index} {}",
                "x".repeat(4_000)
            )));
        }
        let short_result = compact_session_to_token_budget(&short, 2, 2_000);
        let long_result = compact_session_to_token_budget(&long, 2, 2_000);
        assert_eq!(short_result.removed_message_count, 0, "everything fits");
        assert_eq!(long_result.compacted_session.messages.len(), 1 + 2);
    }

    #[test]
    fn summary_skips_auto_continue_prompts_when_listing_requests() {
        let mut session = Session::new();
        session.messages = vec![
            ConversationMessage::user_text("write the report"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "partial".to_string(),
            }]),
            ConversationMessage::user_text(format!("{AUTO_CONTINUE_PREFIX} continue")),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "rest".to_string(),
            }]),
        ];
        let result = compact_session(
            &session,
            CompactionConfig {
                preserve_recent_messages: 0,
                max_estimated_tokens: 0,
            },
        );
        assert!(result.formatted_summary.contains("> write the report"));
        assert!(!result.formatted_summary.contains("Earlier user requests"));
    }
}
