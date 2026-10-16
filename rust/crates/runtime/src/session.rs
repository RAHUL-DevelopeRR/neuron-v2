use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::json::{JsonError, JsonValue};
use crate::transcript::{normalize_messages, TranscriptRepair};
use crate::usage::TokenUsage;

const SESSION_VERSION: u32 = 1;
const ROTATE_AFTER_BYTES: u64 = 256 * 1024;
const MAX_ROTATED_FILES: usize = 3;
static SESSION_ID_COUNTER: AtomicU64 = AtomicU64::new(0);
static LAST_TIMESTAMP_MS: AtomicU64 = AtomicU64::new(0);

/// Speaker role associated with a persisted conversation message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Structured message content stored inside a [`Session`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    ToolResult {
        tool_use_id: String,
        tool_name: String,
        output: String,
        is_error: bool,
    },
}

/// One conversation message with optional token-usage metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationMessage {
    pub role: MessageRole,
    pub blocks: Vec<ContentBlock>,
    pub usage: Option<TokenUsage>,
}

/// Metadata describing the latest compaction that summarized a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCompaction {
    pub count: u32,
    pub removed_message_count: usize,
    pub summary: String,
}

/// Provenance recorded when a session is forked from another session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFork {
    pub parent_session_id: String,
    pub branch_name: Option<String>,
}

/// A single user prompt recorded with a timestamp for history tracking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPromptEntry {
    pub timestamp_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionPersistence {
    path: PathBuf,
    /// The file on disk differs from memory (malformed records were skipped
    /// or the transcript was repaired on load): the next write rewrites the
    /// whole file instead of appending to it.
    rewrite_before_append: bool,
}

/// Persisted conversational state for the runtime and CLI session manager.
///
/// `workspace_root` binds the session to the worktree it was created in. The
/// global session store under `~/.local/share/opencode` is shared across every
/// `opencode serve` instance, so without an explicit workspace root parallel
/// lanes can race and report success while writes land in the wrong CWD. See
/// ROADMAP.md item 41 (Phantom completions root cause) for the full
/// background.
#[derive(Debug, Clone)]
pub struct Session {
    pub version: u32,
    pub session_id: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub messages: Vec<ConversationMessage>,
    pub compaction: Option<SessionCompaction>,
    pub fork: Option<SessionFork>,
    pub workspace_root: Option<PathBuf>,
    pub prompt_history: Vec<SessionPromptEntry>,
    /// The model used in this session, persisted so resumed sessions can
    /// report which model was originally used.
    /// Timestamp of last successful health check (ROADMAP #38)
    pub last_health_check_ms: Option<u64>,
    pub model: Option<String>,
    persistence: Option<SessionPersistence>,
    load_diagnostics: SessionLoadDiagnostics,
}

/// What happened while a session file was loaded: malformed JSONL records
/// that were skipped and transcript repairs applied by the normalizer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionLoadDiagnostics {
    pub skipped_records: usize,
    pub first_skip_reason: Option<String>,
    pub transcript_repair: TranscriptRepair,
}

impl SessionLoadDiagnostics {
    #[must_use]
    pub fn has_warnings(&self) -> bool {
        self.skipped_records > 0 || self.transcript_repair.changed()
    }

    /// One-line human readable description, `None` when the load was clean.
    #[must_use]
    pub fn warning_message(&self) -> Option<String> {
        if !self.has_warnings() {
            return None;
        }
        let mut parts = Vec::new();
        if self.skipped_records > 0 {
            let reason = self
                .first_skip_reason
                .as_deref()
                .map(|reason| format!(" (first: {reason})"))
                .unwrap_or_default();
            parts.push(format!(
                "skipped {} malformed record(s){reason}",
                self.skipped_records
            ));
        }
        let repair = &self.transcript_repair;
        if repair.synthesized_tool_results > 0 {
            parts.push(format!(
                "added {} interrupted tool result(s)",
                repair.synthesized_tool_results
            ));
        }
        if repair.dropped_orphan_tool_results > 0 {
            parts.push(format!(
                "dropped {} orphan tool result(s)",
                repair.dropped_orphan_tool_results
            ));
        }
        if repair.dropped_empty_assistant_messages > 0 {
            parts.push(format!(
                "dropped {} empty assistant message(s)",
                repair.dropped_empty_assistant_messages
            ));
        }
        Some(parts.join("; "))
    }
}

impl PartialEq for Session {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
            && self.session_id == other.session_id
            && self.created_at_ms == other.created_at_ms
            && self.updated_at_ms == other.updated_at_ms
            && self.messages == other.messages
            && self.compaction == other.compaction
            && self.fork == other.fork
            && self.workspace_root == other.workspace_root
            && self.prompt_history == other.prompt_history
            && self.last_health_check_ms == other.last_health_check_ms
    }
}

impl Eq for Session {}

/// Errors raised while loading, parsing, or saving sessions.
#[derive(Debug)]
pub enum SessionError {
    Io(std::io::Error),
    Json(JsonError),
    Format(String),
}

impl Display for SessionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::Format(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<std::io::Error> for SessionError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<JsonError> for SessionError {
    fn from(value: JsonError) -> Self {
        Self::Json(value)
    }
}

impl Session {
    #[must_use]
    pub fn new() -> Self {
        let now = current_time_millis();
        Self {
            version: SESSION_VERSION,
            session_id: generate_session_id(),
            created_at_ms: now,
            updated_at_ms: now,
            messages: Vec::new(),
            compaction: None,
            fork: None,
            workspace_root: None,
            prompt_history: Vec::new(),
            last_health_check_ms: None,
            model: None,
            persistence: None,
            load_diagnostics: SessionLoadDiagnostics::default(),
        }
    }

    #[must_use]
    pub fn with_persistence_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.persistence = Some(SessionPersistence {
            path: path.into(),
            rewrite_before_append: false,
        });
        self
    }

    /// Bind this session to the workspace root it was created in.
    ///
    /// This is the per-worktree counterpart to the global session store and
    /// lets downstream tooling reject writes that drift to the wrong CWD when
    /// multiple `opencode serve` instances share `~/.local/share/opencode`.
    #[must_use]
    pub fn with_workspace_root(mut self, workspace_root: impl Into<PathBuf>) -> Self {
        self.workspace_root = Some(workspace_root.into());
        self
    }

    #[must_use]
    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace_root.as_deref()
    }

    #[must_use]
    pub fn persistence_path(&self) -> Option<&Path> {
        self.persistence.as_ref().map(|value| value.path.as_path())
    }

    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<(), SessionError> {
        let path = path.as_ref();
        let snapshot = self.render_jsonl_snapshot()?;
        rotate_session_file_if_needed(path)?;
        write_atomic(path, &snapshot)?;
        cleanup_rotated_logs(path)?;
        Ok(())
    }

    /// Loads a session from a JSONL (or legacy JSON) file.
    ///
    /// Malformed JSONL records are skipped and counted instead of failing the
    /// whole load, and the transcript is normalized (see
    /// [`crate::transcript::normalize_messages`]). Both are reported through
    /// [`Session::load_diagnostics`] so callers can warn where it matters (for
    /// example when resuming), not on every listing.
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, SessionError> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path)?;
        let mut session = match JsonValue::parse(&contents) {
            Ok(value)
                if value
                    .as_object()
                    .is_some_and(|object| object.contains_key("messages")) =>
            {
                Self::from_json(&value)?
            }
            Err(_) | Ok(_) => Self::from_jsonl(&contents)?,
        };
        session.load_diagnostics.transcript_repair = normalize_messages(&mut session.messages);
        let rewrite_before_append = session.load_diagnostics.has_warnings();
        let mut session = session.with_persistence_path(path.to_path_buf());
        if let Some(persistence) = session.persistence.as_mut() {
            persistence.rewrite_before_append = rewrite_before_append;
        }
        Ok(session)
    }

    /// Diagnostics recorded by [`Session::load_from_path`].
    #[must_use]
    pub fn load_diagnostics(&self) -> &SessionLoadDiagnostics {
        &self.load_diagnostics
    }

    /// Rewrites the persisted file (when one is configured) with the full
    /// in-memory state, so disk and memory agree after in-place edits such as
    /// compaction, pruning or transcript repair.
    pub fn persist_snapshot(&self) -> Result<(), SessionError> {
        match self.persistence_path() {
            Some(path) => self.save_to_path(path),
            None => Ok(()),
        }
    }

    pub fn push_message(&mut self, message: ConversationMessage) -> Result<(), SessionError> {
        self.touch();
        self.messages.push(message);
        let persist_result = if self.take_pending_rewrite() {
            let rewritten = self.persist_snapshot();
            if rewritten.is_err() {
                self.set_pending_rewrite();
            }
            rewritten
        } else {
            let message_ref = self.messages.last().ok_or_else(|| {
                SessionError::Format("message was just pushed but missing".to_string())
            })?;
            self.append_persisted_message(message_ref)
        };
        if let Err(error) = persist_result {
            self.messages.pop();
            return Err(error);
        }
        Ok(())
    }

    /// Returns (and clears) whether the file must be rewritten before the
    /// next append because it no longer matches memory.
    fn take_pending_rewrite(&mut self) -> bool {
        self.persistence
            .as_mut()
            .is_some_and(|persistence| std::mem::take(&mut persistence.rewrite_before_append))
    }

    fn set_pending_rewrite(&mut self) {
        if let Some(persistence) = self.persistence.as_mut() {
            persistence.rewrite_before_append = true;
        }
    }

    pub fn push_user_text(&mut self, text: impl Into<String>) -> Result<(), SessionError> {
        self.push_message(ConversationMessage::user_text(text))
    }

    pub fn record_compaction(&mut self, summary: impl Into<String>, removed_message_count: usize) {
        self.touch();
        let count = self.compaction.as_ref().map_or(1, |value| value.count + 1);
        self.compaction = Some(SessionCompaction {
            count,
            removed_message_count,
            summary: summary.into(),
        });
    }

    #[must_use]
    pub fn fork(&self, branch_name: Option<String>) -> Self {
        let now = current_time_millis();
        Self {
            version: self.version,
            session_id: generate_session_id(),
            created_at_ms: now,
            updated_at_ms: now,
            messages: self.messages.clone(),
            compaction: self.compaction.clone(),
            fork: Some(SessionFork {
                parent_session_id: self.session_id.clone(),
                branch_name: normalize_optional_string(branch_name),
            }),
            workspace_root: self.workspace_root.clone(),
            prompt_history: self.prompt_history.clone(),
            last_health_check_ms: self.last_health_check_ms,
            model: self.model.clone(),
            persistence: None,
            load_diagnostics: SessionLoadDiagnostics::default(),
        }
    }

    pub fn to_json(&self) -> Result<JsonValue, SessionError> {
        let mut object = BTreeMap::new();
        object.insert(
            "version".to_string(),
            JsonValue::Number(i64::from(self.version)),
        );
        object.insert(
            "session_id".to_string(),
            JsonValue::String(self.session_id.clone()),
        );
        object.insert(
            "created_at_ms".to_string(),
            JsonValue::Number(i64_from_u64(self.created_at_ms, "created_at_ms")?),
        );
        object.insert(
            "updated_at_ms".to_string(),
            JsonValue::Number(i64_from_u64(self.updated_at_ms, "updated_at_ms")?),
        );
        object.insert(
            "messages".to_string(),
            JsonValue::Array(
                self.messages
                    .iter()
                    .map(ConversationMessage::to_json)
                    .collect(),
            ),
        );
        if let Some(compaction) = &self.compaction {
            object.insert("compaction".to_string(), compaction.to_json()?);
        }
        if let Some(fork) = &self.fork {
            object.insert("fork".to_string(), fork.to_json());
        }
        if let Some(workspace_root) = &self.workspace_root {
            object.insert(
                "workspace_root".to_string(),
                JsonValue::String(workspace_root_to_string(workspace_root)?),
            );
        }
        if !self.prompt_history.is_empty() {
            object.insert(
                "prompt_history".to_string(),
                JsonValue::Array(
                    self.prompt_history
                        .iter()
                        .map(SessionPromptEntry::to_jsonl_record)
                        .collect(),
                ),
            );
        }
        Ok(JsonValue::Object(object))
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, SessionError> {
        let object = value
            .as_object()
            .ok_or_else(|| SessionError::Format("session must be an object".to_string()))?;
        let version = object
            .get("version")
            .and_then(JsonValue::as_i64)
            .ok_or_else(|| SessionError::Format("missing version".to_string()))?;
        let version = u32::try_from(version)
            .map_err(|_| SessionError::Format("version out of range".to_string()))?;
        let messages = object
            .get("messages")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| SessionError::Format("missing messages".to_string()))?
            .iter()
            .map(ConversationMessage::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let now = current_time_millis();
        let session_id = object
            .get("session_id")
            .and_then(JsonValue::as_str)
            .map_or_else(generate_session_id, ToOwned::to_owned);
        let created_at_ms = object
            .get("created_at_ms")
            .map(|value| required_u64_from_value(value, "created_at_ms"))
            .transpose()?
            .unwrap_or(now);
        let updated_at_ms = object
            .get("updated_at_ms")
            .map(|value| required_u64_from_value(value, "updated_at_ms"))
            .transpose()?
            .unwrap_or(created_at_ms);
        let compaction = object
            .get("compaction")
            .map(SessionCompaction::from_json)
            .transpose()?;
        let fork = object.get("fork").map(SessionFork::from_json).transpose()?;
        let workspace_root = object
            .get("workspace_root")
            .and_then(JsonValue::as_str)
            .map(PathBuf::from);
        let prompt_history = object
            .get("prompt_history")
            .and_then(JsonValue::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(SessionPromptEntry::from_json_opt)
                    .collect()
            })
            .unwrap_or_default();
        let model = object
            .get("model")
            .and_then(JsonValue::as_str)
            .map(String::from);
        Ok(Self {
            version,
            session_id,
            created_at_ms,
            updated_at_ms,
            messages,
            compaction,
            fork,
            workspace_root,
            prompt_history,
            last_health_check_ms: None,
            model,
            persistence: None,
            load_diagnostics: SessionLoadDiagnostics::default(),
        })
    }

    fn from_jsonl(contents: &str) -> Result<Self, SessionError> {
        let mut state = JsonlLoadState::default();
        let mut first_error = None;
        for (line_index, raw_line) in contents.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }
            match state.apply_record(line_index + 1, line) {
                Ok(()) => state.valid_records += 1,
                Err(error) => {
                    state.diagnostics.skipped_records += 1;
                    if state.diagnostics.first_skip_reason.is_none() {
                        state.diagnostics.first_skip_reason = Some(error.to_string());
                    }
                    first_error.get_or_insert(error);
                }
            }
        }
        if state.valid_records == 0 {
            if let Some(error) = first_error {
                // Nothing usable at all: this is not a session file.
                return Err(SessionError::Format(format!(
                    "session file contains no valid records ({} malformed): {error}",
                    state.diagnostics.skipped_records
                )));
            }
        }

        let now = current_time_millis();
        let created_at_ms = state.created_at_ms;
        Ok(Self {
            version: state.version.unwrap_or(SESSION_VERSION),
            session_id: state.session_id.unwrap_or_else(generate_session_id),
            created_at_ms: created_at_ms.unwrap_or(now),
            updated_at_ms: state.updated_at_ms.unwrap_or(created_at_ms.unwrap_or(now)),
            messages: state.messages,
            compaction: state.compaction,
            fork: state.fork,
            workspace_root: state.workspace_root,
            prompt_history: state.prompt_history,
            last_health_check_ms: None,
            model: state.model,
            persistence: None,
            load_diagnostics: state.diagnostics,
        })
    }

    /// Record a user prompt with the current wall-clock timestamp.
    ///
    /// The entry is appended to the in-memory history and, when a persistence
    /// path is configured, incrementally written to the JSONL session file.
    pub fn push_prompt_entry(&mut self, text: impl Into<String>) -> Result<(), SessionError> {
        let timestamp_ms = current_time_millis();
        let entry = SessionPromptEntry {
            timestamp_ms,
            text: text.into(),
        };
        self.prompt_history.push(entry);
        if self.take_pending_rewrite() {
            let rewritten = self.persist_snapshot();
            if rewritten.is_err() {
                self.set_pending_rewrite();
            }
            return rewritten;
        }
        let entry_ref = self.prompt_history.last().expect("entry was just pushed");
        self.append_persisted_prompt_entry(entry_ref)
    }

    fn render_jsonl_snapshot(&self) -> Result<String, SessionError> {
        let mut lines = vec![self.meta_record()?.render()];
        if let Some(compaction) = &self.compaction {
            lines.push(compaction.to_jsonl_record()?.render());
        }
        lines.extend(
            self.prompt_history
                .iter()
                .map(|entry| entry.to_jsonl_record().render()),
        );
        lines.extend(
            self.messages
                .iter()
                .map(|message| message_record(message).render()),
        );
        let mut rendered = lines.join("\n");
        rendered.push('\n');
        Ok(rendered)
    }

    fn append_persisted_message(&self, message: &ConversationMessage) -> Result<(), SessionError> {
        let Some(path) = self.persistence_path() else {
            return Ok(());
        };

        let needs_bootstrap = !path.exists() || fs::metadata(path)?.len() == 0;
        if needs_bootstrap {
            self.save_to_path(path)?;
            return Ok(());
        }

        let mut file = open_for_record_append(path)?;
        writeln!(file, "{}", message_record(message).render())?;
        Ok(())
    }

    fn append_persisted_prompt_entry(
        &self,
        entry: &SessionPromptEntry,
    ) -> Result<(), SessionError> {
        let Some(path) = self.persistence_path() else {
            return Ok(());
        };

        let needs_bootstrap = !path.exists() || fs::metadata(path)?.len() == 0;
        if needs_bootstrap {
            self.save_to_path(path)?;
            return Ok(());
        }

        let mut file = open_for_record_append(path)?;
        writeln!(file, "{}", entry.to_jsonl_record().render())?;
        Ok(())
    }

    fn meta_record(&self) -> Result<JsonValue, SessionError> {
        let mut object = BTreeMap::new();
        object.insert(
            "type".to_string(),
            JsonValue::String("session_meta".to_string()),
        );
        object.insert(
            "version".to_string(),
            JsonValue::Number(i64::from(self.version)),
        );
        object.insert(
            "session_id".to_string(),
            JsonValue::String(self.session_id.clone()),
        );
        object.insert(
            "created_at_ms".to_string(),
            JsonValue::Number(i64_from_u64(self.created_at_ms, "created_at_ms")?),
        );
        object.insert(
            "updated_at_ms".to_string(),
            JsonValue::Number(i64_from_u64(self.updated_at_ms, "updated_at_ms")?),
        );
        if let Some(fork) = &self.fork {
            object.insert("fork".to_string(), fork.to_json());
        }
        if let Some(workspace_root) = &self.workspace_root {
            object.insert(
                "workspace_root".to_string(),
                JsonValue::String(workspace_root_to_string(workspace_root)?),
            );
        }
        if let Some(model) = &self.model {
            object.insert("model".to_string(), JsonValue::String(model.clone()));
        }
        Ok(JsonValue::Object(object))
    }

    fn touch(&mut self) {
        self.updated_at_ms = current_time_millis();
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationMessage {
    #[must_use]
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text { text: text.into() }],
            usage: None,
        }
    }

    #[must_use]
    pub fn assistant(blocks: Vec<ContentBlock>) -> Self {
        Self {
            role: MessageRole::Assistant,
            blocks,
            usage: None,
        }
    }

    #[must_use]
    pub fn assistant_with_usage(blocks: Vec<ContentBlock>, usage: Option<TokenUsage>) -> Self {
        Self {
            role: MessageRole::Assistant,
            blocks,
            usage,
        }
    }

    #[must_use]
    pub fn tool_result(
        tool_use_id: impl Into<String>,
        tool_name: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.into(),
                tool_name: tool_name.into(),
                output: output.into(),
                is_error,
            }],
            usage: None,
        }
    }

    #[must_use]
    pub fn to_json(&self) -> JsonValue {
        let mut object = BTreeMap::new();
        object.insert(
            "role".to_string(),
            JsonValue::String(
                match self.role {
                    MessageRole::System => "system",
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::Tool => "tool",
                }
                .to_string(),
            ),
        );
        object.insert(
            "blocks".to_string(),
            JsonValue::Array(self.blocks.iter().map(ContentBlock::to_json).collect()),
        );
        if let Some(usage) = self.usage {
            object.insert("usage".to_string(), usage_to_json(usage));
        }
        JsonValue::Object(object)
    }

    fn from_json(value: &JsonValue) -> Result<Self, SessionError> {
        let object = value
            .as_object()
            .ok_or_else(|| SessionError::Format("message must be an object".to_string()))?;
        let role = match object
            .get("role")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| SessionError::Format("missing role".to_string()))?
        {
            "system" => MessageRole::System,
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "tool" => MessageRole::Tool,
            other => {
                return Err(SessionError::Format(format!(
                    "unsupported message role: {other}"
                )))
            }
        };
        let blocks = object
            .get("blocks")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| SessionError::Format("missing blocks".to_string()))?
            .iter()
            .map(ContentBlock::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let usage = object.get("usage").map(usage_from_json).transpose()?;
        Ok(Self {
            role,
            blocks,
            usage,
        })
    }
}

impl ContentBlock {
    #[must_use]
    pub fn to_json(&self) -> JsonValue {
        let mut object = BTreeMap::new();
        match self {
            Self::Text { text } => {
                object.insert("type".to_string(), JsonValue::String("text".to_string()));
                object.insert("text".to_string(), JsonValue::String(text.clone()));
            }
            Self::ToolUse { id, name, input } => {
                object.insert(
                    "type".to_string(),
                    JsonValue::String("tool_use".to_string()),
                );
                object.insert("id".to_string(), JsonValue::String(id.clone()));
                object.insert("name".to_string(), JsonValue::String(name.clone()));
                object.insert("input".to_string(), JsonValue::String(input.clone()));
            }
            Self::ToolResult {
                tool_use_id,
                tool_name,
                output,
                is_error,
            } => {
                object.insert(
                    "type".to_string(),
                    JsonValue::String("tool_result".to_string()),
                );
                object.insert(
                    "tool_use_id".to_string(),
                    JsonValue::String(tool_use_id.clone()),
                );
                object.insert(
                    "tool_name".to_string(),
                    JsonValue::String(tool_name.clone()),
                );
                object.insert("output".to_string(), JsonValue::String(output.clone()));
                object.insert("is_error".to_string(), JsonValue::Bool(*is_error));
            }
        }
        JsonValue::Object(object)
    }

    fn from_json(value: &JsonValue) -> Result<Self, SessionError> {
        let object = value
            .as_object()
            .ok_or_else(|| SessionError::Format("block must be an object".to_string()))?;
        match object
            .get("type")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| SessionError::Format("missing block type".to_string()))?
        {
            "text" => Ok(Self::Text {
                text: required_string(object, "text")?,
            }),
            "tool_use" => Ok(Self::ToolUse {
                id: required_string(object, "id")?,
                name: required_string(object, "name")?,
                input: required_string(object, "input")?,
            }),
            "tool_result" => Ok(Self::ToolResult {
                tool_use_id: required_string(object, "tool_use_id")?,
                tool_name: required_string(object, "tool_name")?,
                output: required_string(object, "output")?,
                is_error: object
                    .get("is_error")
                    .and_then(JsonValue::as_bool)
                    .ok_or_else(|| SessionError::Format("missing is_error".to_string()))?,
            }),
            other => Err(SessionError::Format(format!(
                "unsupported block type: {other}"
            ))),
        }
    }
}

impl SessionCompaction {
    pub fn to_json(&self) -> Result<JsonValue, SessionError> {
        let mut object = BTreeMap::new();
        object.insert(
            "count".to_string(),
            JsonValue::Number(i64::from(self.count)),
        );
        object.insert(
            "removed_message_count".to_string(),
            JsonValue::Number(i64_from_usize(
                self.removed_message_count,
                "removed_message_count",
            )?),
        );
        object.insert(
            "summary".to_string(),
            JsonValue::String(self.summary.clone()),
        );
        Ok(JsonValue::Object(object))
    }

    pub fn to_jsonl_record(&self) -> Result<JsonValue, SessionError> {
        let mut object = BTreeMap::new();
        object.insert(
            "type".to_string(),
            JsonValue::String("compaction".to_string()),
        );
        object.insert(
            "count".to_string(),
            JsonValue::Number(i64::from(self.count)),
        );
        object.insert(
            "removed_message_count".to_string(),
            JsonValue::Number(i64_from_usize(
                self.removed_message_count,
                "removed_message_count",
            )?),
        );
        object.insert(
            "summary".to_string(),
            JsonValue::String(self.summary.clone()),
        );
        Ok(JsonValue::Object(object))
    }

    fn from_json(value: &JsonValue) -> Result<Self, SessionError> {
        let object = value
            .as_object()
            .ok_or_else(|| SessionError::Format("compaction must be an object".to_string()))?;
        Ok(Self {
            count: required_u32(object, "count")?,
            removed_message_count: required_usize(object, "removed_message_count")?,
            summary: required_string(object, "summary")?,
        })
    }
}

impl SessionFork {
    #[must_use]
    pub fn to_json(&self) -> JsonValue {
        let mut object = BTreeMap::new();
        object.insert(
            "parent_session_id".to_string(),
            JsonValue::String(self.parent_session_id.clone()),
        );
        if let Some(branch_name) = &self.branch_name {
            object.insert(
                "branch_name".to_string(),
                JsonValue::String(branch_name.clone()),
            );
        }
        JsonValue::Object(object)
    }

    fn from_json(value: &JsonValue) -> Result<Self, SessionError> {
        let object = value
            .as_object()
            .ok_or_else(|| SessionError::Format("fork metadata must be an object".to_string()))?;
        Ok(Self {
            parent_session_id: required_string(object, "parent_session_id")?,
            branch_name: object
                .get("branch_name")
                .and_then(JsonValue::as_str)
                .map(ToOwned::to_owned),
        })
    }
}

impl SessionPromptEntry {
    #[must_use]
    pub fn to_jsonl_record(&self) -> JsonValue {
        let mut object = BTreeMap::new();
        object.insert(
            "type".to_string(),
            JsonValue::String("prompt_history".to_string()),
        );
        object.insert(
            "timestamp_ms".to_string(),
            JsonValue::Number(i64::try_from(self.timestamp_ms).unwrap_or(i64::MAX)),
        );
        object.insert("text".to_string(), JsonValue::String(self.text.clone()));
        JsonValue::Object(object)
    }

    fn from_json_opt(value: &JsonValue) -> Option<Self> {
        let object = value.as_object()?;
        let timestamp_ms = object
            .get("timestamp_ms")
            .and_then(JsonValue::as_i64)
            .and_then(|value| u64::try_from(value).ok())?;
        let text = object.get("text").and_then(JsonValue::as_str)?.to_string();
        Some(Self { timestamp_ms, text })
    }
}

/// Accumulates the records of a JSONL session file. Each record is applied
/// atomically: a malformed record leaves the state untouched.
#[derive(Default)]
struct JsonlLoadState {
    version: Option<u32>,
    session_id: Option<String>,
    created_at_ms: Option<u64>,
    updated_at_ms: Option<u64>,
    messages: Vec<ConversationMessage>,
    compaction: Option<SessionCompaction>,
    fork: Option<SessionFork>,
    workspace_root: Option<PathBuf>,
    model: Option<String>,
    prompt_history: Vec<SessionPromptEntry>,
    valid_records: usize,
    diagnostics: SessionLoadDiagnostics,
}

impl JsonlLoadState {
    fn apply_record(&mut self, line_number: usize, line: &str) -> Result<(), SessionError> {
        let value = JsonValue::parse(line).map_err(|error| {
            SessionError::Format(format!(
                "invalid JSONL record at line {line_number}: {error}"
            ))
        })?;
        let object = value.as_object().ok_or_else(|| {
            SessionError::Format(format!(
                "JSONL record at line {line_number} must be an object"
            ))
        })?;
        let record_type = object
            .get("type")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| {
                SessionError::Format(format!("JSONL record at line {line_number} missing type"))
            })?;
        match record_type {
            "session_meta" => {
                let version = required_u32(object, "version")?;
                let session_id = required_string(object, "session_id")?;
                let created_at_ms = required_u64(object, "created_at_ms")?;
                let updated_at_ms = required_u64(object, "updated_at_ms")?;
                let fork = object.get("fork").map(SessionFork::from_json).transpose()?;
                self.version = Some(version);
                self.session_id = Some(session_id);
                self.created_at_ms = Some(created_at_ms);
                self.updated_at_ms = Some(updated_at_ms);
                self.fork = fork;
                self.workspace_root = object
                    .get("workspace_root")
                    .and_then(JsonValue::as_str)
                    .map(PathBuf::from);
                self.model = object
                    .get("model")
                    .and_then(JsonValue::as_str)
                    .map(String::from);
            }
            "message" => {
                let message_value = object.get("message").ok_or_else(|| {
                    SessionError::Format(format!(
                        "JSONL record at line {line_number} missing message"
                    ))
                })?;
                let message = ConversationMessage::from_json(message_value).map_err(|error| {
                    SessionError::Format(format!(
                        "invalid message record at line {line_number}: {error}"
                    ))
                })?;
                self.messages.push(message);
            }
            "compaction" => {
                self.compaction = Some(SessionCompaction::from_json(&JsonValue::Object(
                    object.clone(),
                ))?);
            }
            "prompt_history" => {
                let entry = SessionPromptEntry::from_json_opt(&JsonValue::Object(object.clone()))
                    .ok_or_else(|| {
                    SessionError::Format(format!(
                        "invalid prompt_history record at line {line_number}"
                    ))
                })?;
                self.prompt_history.push(entry);
            }
            other => {
                return Err(SessionError::Format(format!(
                    "unsupported JSONL record type at line {line_number}: {other}"
                )))
            }
        }
        Ok(())
    }
}

fn message_record(message: &ConversationMessage) -> JsonValue {
    let mut object = BTreeMap::new();
    object.insert("type".to_string(), JsonValue::String("message".to_string()));
    object.insert("message".to_string(), message.to_json());
    JsonValue::Object(object)
}

fn usage_to_json(usage: TokenUsage) -> JsonValue {
    let mut object = BTreeMap::new();
    object.insert(
        "input_tokens".to_string(),
        JsonValue::Number(i64::from(usage.input_tokens)),
    );
    object.insert(
        "output_tokens".to_string(),
        JsonValue::Number(i64::from(usage.output_tokens)),
    );
    object.insert(
        "cache_creation_input_tokens".to_string(),
        JsonValue::Number(i64::from(usage.cache_creation_input_tokens)),
    );
    object.insert(
        "cache_read_input_tokens".to_string(),
        JsonValue::Number(i64::from(usage.cache_read_input_tokens)),
    );
    JsonValue::Object(object)
}

fn usage_from_json(value: &JsonValue) -> Result<TokenUsage, SessionError> {
    let object = value
        .as_object()
        .ok_or_else(|| SessionError::Format("usage must be an object".to_string()))?;
    Ok(TokenUsage {
        input_tokens: required_u32(object, "input_tokens")?,
        output_tokens: required_u32(object, "output_tokens")?,
        cache_creation_input_tokens: required_u32(object, "cache_creation_input_tokens")?,
        cache_read_input_tokens: required_u32(object, "cache_read_input_tokens")?,
    })
}

fn required_string(
    object: &BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<String, SessionError> {
    object
        .get(key)
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| SessionError::Format(format!("missing {key}")))
}

fn required_u32(object: &BTreeMap<String, JsonValue>, key: &str) -> Result<u32, SessionError> {
    let value = object
        .get(key)
        .and_then(JsonValue::as_i64)
        .ok_or_else(|| SessionError::Format(format!("missing {key}")))?;
    u32::try_from(value).map_err(|_| SessionError::Format(format!("{key} out of range")))
}

fn required_u64(object: &BTreeMap<String, JsonValue>, key: &str) -> Result<u64, SessionError> {
    let value = object
        .get(key)
        .ok_or_else(|| SessionError::Format(format!("missing {key}")))?;
    required_u64_from_value(value, key)
}

fn required_u64_from_value(value: &JsonValue, key: &str) -> Result<u64, SessionError> {
    let value = value
        .as_i64()
        .ok_or_else(|| SessionError::Format(format!("missing {key}")))?;
    u64::try_from(value).map_err(|_| SessionError::Format(format!("{key} out of range")))
}

fn required_usize(object: &BTreeMap<String, JsonValue>, key: &str) -> Result<usize, SessionError> {
    let value = object
        .get(key)
        .and_then(JsonValue::as_i64)
        .ok_or_else(|| SessionError::Format(format!("missing {key}")))?;
    usize::try_from(value).map_err(|_| SessionError::Format(format!("{key} out of range")))
}

fn i64_from_u64(value: u64, key: &str) -> Result<i64, SessionError> {
    i64::try_from(value)
        .map_err(|_| SessionError::Format(format!("{key} out of range for JSON number")))
}

fn i64_from_usize(value: usize, key: &str) -> Result<i64, SessionError> {
    i64::try_from(value)
        .map_err(|_| SessionError::Format(format!("{key} out of range for JSON number")))
}

fn workspace_root_to_string(path: &Path) -> Result<String, SessionError> {
    path.to_str().map(ToOwned::to_owned).ok_or_else(|| {
        SessionError::Format(format!(
            "workspace_root is not valid UTF-8: {}",
            path.display()
        ))
    })
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn current_time_millis() -> u64 {
    let wall_clock = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default();

    let mut candidate = wall_clock;
    loop {
        let previous = LAST_TIMESTAMP_MS.load(Ordering::Relaxed);
        if candidate <= previous {
            candidate = previous.saturating_add(1);
        }
        match LAST_TIMESTAMP_MS.compare_exchange(
            previous,
            candidate,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => return candidate,
            Err(actual) => candidate = actual.saturating_add(1),
        }
    }
}

fn generate_session_id() -> String {
    let millis = current_time_millis();
    let counter = SESSION_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("session-{millis}-{counter}")
}

/// Opens a JSONL file for appending one record. When the file does not end
/// with a newline (a crash cut the last record off), a newline is written
/// first so the new record does not merge into the partial one.
fn open_for_record_append(path: &Path) -> Result<fs::File, SessionError> {
    let mut file = OpenOptions::new().read(true).append(true).open(path)?;
    let len = file.metadata()?.len();
    if len > 0 {
        file.seek(SeekFrom::Start(len - 1))?;
        let mut last = [0_u8; 1];
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            file.write_all(b"\n")?;
        }
    }
    Ok(file)
}

fn write_atomic(path: &Path, contents: &str) -> Result<(), SessionError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = temporary_path_for(path);
    fs::write(&temp_path, contents)?;
    fs::rename(temp_path, path)?;
    Ok(())
}

fn temporary_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("session");
    path.with_file_name(format!(
        "{file_name}.tmp-{}-{}",
        current_time_millis(),
        SESSION_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

fn rotate_session_file_if_needed(path: &Path) -> Result<(), SessionError> {
    let Ok(metadata) = fs::metadata(path) else {
        return Ok(());
    };
    if metadata.len() < ROTATE_AFTER_BYTES {
        return Ok(());
    }
    let rotated_path = rotated_log_path(path);
    fs::rename(path, rotated_path)?;
    Ok(())
}

fn rotated_log_path(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("session");
    path.with_file_name(format!("{stem}.rot-{}.jsonl", current_time_millis()))
}

fn cleanup_rotated_logs(path: &Path) -> Result<(), SessionError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("session");
    let prefix = format!("{stem}.rot-");
    let mut rotated_paths = fs::read_dir(parent)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|entry_path| {
            entry_path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| {
                    name.starts_with(&prefix)
                        && Path::new(name)
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
                })
        })
        .collect::<Vec<_>>();

    rotated_paths.sort_by_key(|entry_path| {
        fs::metadata(entry_path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH)
    });

    let remove_count = rotated_paths.len().saturating_sub(MAX_ROTATED_FILES);
    for stale_path in rotated_paths.into_iter().take(remove_count) {
        fs::remove_file(stale_path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        cleanup_rotated_logs, current_time_millis, rotate_session_file_if_needed, ContentBlock,
        ConversationMessage, MessageRole, Session, SessionFork,
    };
    use crate::json::JsonValue;
    use crate::usage::TokenUsage;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn session_timestamps_are_monotonic_under_tight_loops() {
        let first = current_time_millis();
        let second = current_time_millis();
        let third = current_time_millis();

        assert!(first < second);
        assert!(second < third);
    }

    #[test]
    fn persists_and_restores_session_jsonl() {
        let mut session = Session::new();
        session
            .push_user_text("hello")
            .expect("user message should append");
        session
            .push_message(ConversationMessage::assistant_with_usage(
                vec![
                    ContentBlock::Text {
                        text: "thinking".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "bash".to_string(),
                        input: "echo hi".to_string(),
                    },
                ],
                Some(TokenUsage {
                    input_tokens: 10,
                    output_tokens: 4,
                    cache_creation_input_tokens: 1,
                    cache_read_input_tokens: 2,
                }),
            ))
            .expect("assistant message should append");
        session
            .push_message(ConversationMessage::tool_result(
                "tool-1", "bash", "hi", false,
            ))
            .expect("tool result should append");

        let path = temp_session_path("jsonl");
        session.save_to_path(&path).expect("session should save");
        let restored = Session::load_from_path(&path).expect("session should load");
        fs::remove_file(&path).expect("temp file should be removable");

        assert_eq!(restored, session);
        assert_eq!(restored.messages[2].role, MessageRole::Tool);
        assert_eq!(
            restored.messages[1].usage.expect("usage").total_tokens(),
            17
        );
        assert_eq!(restored.session_id, session.session_id);
    }

    #[test]
    fn loads_legacy_session_json_object() {
        let path = temp_session_path("legacy");
        let legacy = JsonValue::Object(
            [
                ("version".to_string(), JsonValue::Number(1)),
                (
                    "messages".to_string(),
                    JsonValue::Array(vec![ConversationMessage::user_text("legacy").to_json()]),
                ),
            ]
            .into_iter()
            .collect(),
        );
        fs::write(&path, legacy.render()).expect("legacy file should write");

        let restored = Session::load_from_path(&path).expect("legacy session should load");
        fs::remove_file(&path).expect("temp file should be removable");

        assert_eq!(restored.messages.len(), 1);
        assert_eq!(
            restored.messages[0],
            ConversationMessage::user_text("legacy")
        );
        assert_ne!(restored.session_id, "");
    }

    #[test]
    fn appends_messages_to_persisted_jsonl_session() {
        let path = temp_session_path("append");
        let mut session = Session::new().with_persistence_path(path.clone());
        session
            .save_to_path(&path)
            .expect("initial save should succeed");
        session
            .push_user_text("hi")
            .expect("user append should succeed");
        session
            .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "hello".to_string(),
            }]))
            .expect("assistant append should succeed");

        let restored = Session::load_from_path(&path).expect("session should replay from jsonl");
        fs::remove_file(&path).expect("temp file should be removable");

        assert_eq!(restored.messages.len(), 2);
        assert_eq!(restored.messages[0], ConversationMessage::user_text("hi"));
    }

    #[test]
    fn persists_compaction_metadata() {
        let path = temp_session_path("compaction");
        let mut session = Session::new();
        session
            .push_user_text("before")
            .expect("message should append");
        session.record_compaction("summarized earlier work", 4);
        session.save_to_path(&path).expect("session should save");

        let restored = Session::load_from_path(&path).expect("session should load");
        fs::remove_file(&path).expect("temp file should be removable");

        let compaction = restored.compaction.expect("compaction metadata");
        assert_eq!(compaction.count, 1);
        assert_eq!(compaction.removed_message_count, 4);
        assert!(compaction.summary.contains("summarized"));
    }

    #[test]
    fn forks_sessions_with_branch_metadata_and_persists_it() {
        let path = temp_session_path("fork");
        let mut session = Session::new();
        session
            .push_user_text("before fork")
            .expect("message should append");

        let forked = session
            .fork(Some("investigation".to_string()))
            .with_persistence_path(path.clone());
        forked
            .save_to_path(&path)
            .expect("forked session should save");

        let restored = Session::load_from_path(&path).expect("forked session should load");
        fs::remove_file(&path).expect("temp file should be removable");

        assert_ne!(restored.session_id, session.session_id);
        assert_eq!(
            restored.fork,
            Some(SessionFork {
                parent_session_id: session.session_id,
                branch_name: Some("investigation".to_string()),
            })
        );
        assert_eq!(restored.messages, forked.messages);
    }

    #[test]
    fn rotates_and_cleans_up_large_session_logs() {
        // given
        let path = temp_session_path("rotation");
        let oversized_length =
            usize::try_from(super::ROTATE_AFTER_BYTES + 10).expect("rotate threshold should fit");
        fs::write(&path, "x".repeat(oversized_length)).expect("oversized file should write");

        // when
        rotate_session_file_if_needed(&path).expect("rotation should succeed");

        // then
        assert!(
            !path.exists(),
            "original path should be rotated away before rewrite"
        );

        for _ in 0..5 {
            let rotated = super::rotated_log_path(&path);
            fs::write(&rotated, "old").expect("rotated file should write");
        }
        cleanup_rotated_logs(&path).expect("cleanup should succeed");

        let rotated_count = rotation_files(&path).len();
        assert!(rotated_count <= super::MAX_ROTATED_FILES);
        for rotated in rotation_files(&path) {
            fs::remove_file(rotated).expect("rotated file should be removable");
        }
    }

    #[test]
    fn rejects_jsonl_record_without_type() {
        // given
        let path = write_temp_session_file(
            "missing-type",
            r#"{"message":{"role":"user","blocks":[{"type":"text","text":"hello"}]}}"#,
        );

        // when
        let error = Session::load_from_path(&path)
            .expect_err("session should reject JSONL records without a type");

        // then
        assert!(error.to_string().contains("missing type"));
        fs::remove_file(path).expect("temp file should be removable");
    }

    #[test]
    fn rejects_jsonl_message_record_without_message_payload() {
        // given
        let path = write_temp_session_file("missing-message", r#"{"type":"message"}"#);

        // when
        let error = Session::load_from_path(&path)
            .expect_err("session should reject JSONL message records without message payload");

        // then
        assert!(error.to_string().contains("missing message"));
        fs::remove_file(path).expect("temp file should be removable");
    }

    #[test]
    fn rejects_jsonl_record_with_unknown_type() {
        // given
        let path = write_temp_session_file("unknown-type", r#"{"type":"mystery"}"#);

        // when
        let error = Session::load_from_path(&path)
            .expect_err("session should reject unknown JSONL record types");

        // then
        assert!(error.to_string().contains("unsupported JSONL record type"));
        fs::remove_file(path).expect("temp file should be removable");
    }

    #[test]
    fn rejects_legacy_session_json_without_messages() {
        // given
        let session = JsonValue::Object(
            [("version".to_string(), JsonValue::Number(1))]
                .into_iter()
                .collect(),
        );

        // when
        let error = Session::from_json(&session)
            .expect_err("legacy session objects should require messages");

        // then
        assert!(error.to_string().contains("missing messages"));
    }

    #[test]
    fn normalizes_blank_fork_branch_name_to_none() {
        // given
        let session = Session::new();

        // when
        let forked = session.fork(Some("   ".to_string()));

        // then
        assert_eq!(forked.fork.expect("fork metadata").branch_name, None);
    }

    #[test]
    fn rejects_unknown_content_block_type() {
        // given
        let block = JsonValue::Object(
            [("type".to_string(), JsonValue::String("unknown".to_string()))]
                .into_iter()
                .collect(),
        );

        // when
        let error = ContentBlock::from_json(&block)
            .expect_err("content blocks should reject unknown types");

        // then
        assert!(error.to_string().contains("unsupported block type"));
    }

    #[test]
    fn persists_workspace_root_round_trip_and_forks_inherit_it() {
        // given
        let path = temp_session_path("workspace-root");
        let workspace_root = PathBuf::from("/tmp/b4-phantom-diag");
        let mut session = Session::new().with_workspace_root(workspace_root.clone());
        session
            .push_user_text("write to the right cwd")
            .expect("user message should append");

        // when
        session
            .save_to_path(&path)
            .expect("workspace-bound session should save");
        let restored = Session::load_from_path(&path).expect("session should load");
        let forked = restored.fork(Some("phantom-diag".to_string()));
        fs::remove_file(&path).expect("temp file should be removable");

        // then
        assert_eq!(restored.workspace_root(), Some(workspace_root.as_path()));
        assert_eq!(forked.workspace_root(), Some(workspace_root.as_path()));
    }

    #[test]
    fn tolerant_jsonl_load_skips_malformed_lines_and_counts_them() {
        // given a session file with a truncated write and an unknown record
        let contents = [
            r#"{"type":"session_meta","version":1,"session_id":"tolerant","created_at_ms":1,"updated_at_ms":2}"#,
            r#"{"type":"message","message":{"role":"user","blocks":[{"type":"text","text":"first"}]}}"#,
            r#"{"type":"message","message":{"role":"assistant","blocks":[{"type":"te"#,
            r#"{"type":"mystery"}"#,
            r#"{"type":"message","message":{"role":"assistant","blocks":[{"type":"text","text":"second"}]}}"#,
        ]
        .join("\n");
        let path = write_temp_session_file("tolerant", &contents);

        // when
        let restored = Session::load_from_path(&path).expect("tolerant load should succeed");
        fs::remove_file(&path).expect("temp file should be removable");

        // then
        assert_eq!(restored.session_id, "tolerant");
        assert_eq!(restored.messages.len(), 2);
        let diagnostics = restored.load_diagnostics();
        assert_eq!(diagnostics.skipped_records, 2);
        assert!(diagnostics
            .first_skip_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("line 3")));
        assert!(diagnostics
            .warning_message()
            .is_some_and(|message| message.contains("skipped 2 malformed record(s)")));
    }

    #[test]
    fn first_write_after_a_lossy_load_rewrites_the_file_cleanly() {
        // given a session whose last record was cut off by a crash (no
        // trailing newline)
        let path = temp_session_path("truncated-tail");
        let contents = [
            r#"{"type":"session_meta","version":1,"session_id":"tail","created_at_ms":1,"updated_at_ms":2}"#,
            r#"{"type":"message","message":{"role":"user","blocks":[{"type":"text","text":"first"}]}}"#,
            r#"{"type":"message","message":{"role":"assistant","blocks":[{"type":"te"#,
        ]
        .join("\n");
        fs::write(&path, contents).expect("write session");
        let mut session = Session::load_from_path(&path).expect("tolerant load");
        assert_eq!(session.load_diagnostics().skipped_records, 1);

        // when the next message is recorded
        session.push_user_text("second").expect("append");

        // then the file holds both messages and no malformed record
        let restored = Session::load_from_path(&path).expect("reload");
        assert_eq!(restored.load_diagnostics().skipped_records, 0);
        assert_eq!(
            restored.messages,
            vec![
                ConversationMessage::user_text("first"),
                ConversationMessage::user_text("second"),
            ]
        );
        // and later messages are appended normally again
        session.push_user_text("third").expect("append");
        let restored = Session::load_from_path(&path).expect("reload");
        fs::remove_file(&path).expect("cleanup");
        assert_eq!(restored.messages.len(), 3);
        assert_eq!(restored.load_diagnostics().skipped_records, 0);
    }

    #[test]
    fn append_never_merges_into_a_partial_last_line() {
        // given a cleanly loaded session whose file later lost its tail
        let path = temp_session_path("partial-append");
        let mut session = Session::new().with_persistence_path(path.clone());
        session.push_user_text("first").expect("append");
        let mut file = OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open session");
        file.write_all(br#"{"type":"message","message":{"role":"assis"#)
            .expect("partial record");
        drop(file);

        // when
        session.push_user_text("second").expect("append");

        // then only the partial record is lost, never the new message
        let restored = Session::load_from_path(&path).expect("reload");
        fs::remove_file(&path).expect("cleanup");
        assert_eq!(restored.load_diagnostics().skipped_records, 1);
        assert_eq!(
            restored.messages,
            vec![
                ConversationMessage::user_text("first"),
                ConversationMessage::user_text("second"),
            ]
        );
    }

    #[test]
    fn load_repairs_dangling_tool_use_from_interrupted_session() {
        // given a session that crashed after the model requested a tool
        let mut session = Session::new();
        session.push_user_text("run it").expect("user message");
        session
            .push_message(ConversationMessage::assistant(vec![
                ContentBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "bash".to_string(),
                    input: "{}".to_string(),
                },
            ]))
            .expect("assistant message");
        let path = temp_session_path("dangling");
        session.save_to_path(&path).expect("session should save");

        // when
        let restored = Session::load_from_path(&path).expect("session should load");
        fs::remove_file(&path).expect("temp file should be removable");

        // then the dangling call has a synthetic error result
        assert_eq!(restored.messages.len(), 3);
        assert!(matches!(
            &restored.messages[2].blocks[0],
            ContentBlock::ToolResult { tool_use_id, is_error: true, .. } if tool_use_id == "tool-1"
        ));
        assert_eq!(
            restored
                .load_diagnostics()
                .transcript_repair
                .synthesized_tool_results,
            1
        );
    }

    #[test]
    fn persist_snapshot_rewrites_file_with_in_memory_state() {
        let path = temp_session_path("snapshot");
        let mut session = Session::new().with_persistence_path(path.clone());
        session.push_user_text("one").expect("append");
        session.push_user_text("two").expect("append");
        session.messages.remove(0);

        session.persist_snapshot().expect("snapshot should persist");
        let restored = Session::load_from_path(&path).expect("session should load");
        fs::remove_file(&path).expect("temp file should be removable");

        assert_eq!(
            restored.messages,
            vec![ConversationMessage::user_text("two")]
        );
    }

    fn temp_session_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("runtime-session-{label}-{nanos}.json"))
    }

    fn write_temp_session_file(label: &str, contents: &str) -> PathBuf {
        let path = temp_session_path(label);
        fs::write(&path, format!("{contents}\n")).expect("temp session file should write");
        path
    }

    fn rotation_files(path: &Path) -> Vec<PathBuf> {
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .expect("temp path should have file stem")
            .to_string();
        fs::read_dir(path.parent().expect("temp path should have parent"))
            .expect("temp dir should read")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|entry_path| {
                entry_path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| {
                        name.starts_with(&format!("{stem}.rot-"))
                            && Path::new(name)
                                .extension()
                                .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
                    })
            })
            .collect()
    }
}

/// Per-worktree session isolation: returns a session directory namespaced
/// by the workspace fingerprint of the given working directory.
/// This prevents parallel `opencode serve` instances from colliding.
/// Called by external consumers (e.g. clawhip) to enumerate sessions for a CWD.
#[allow(dead_code)]
pub fn workspace_sessions_dir(cwd: &std::path::Path) -> Result<std::path::PathBuf, SessionError> {
    let store = crate::session_control::SessionStore::from_cwd(cwd)
        .map_err(|e| SessionError::Io(std::io::Error::other(e.to_string())))?;
    Ok(store.sessions_dir().to_path_buf())
}

#[cfg(test)]
mod workspace_sessions_dir_tests {
    use super::*;
    use std::fs;

    #[test]
    fn workspace_sessions_dir_returns_fingerprinted_path_for_valid_cwd() {
        let tmp = std::env::temp_dir().join("claw-session-dir-test");
        fs::create_dir_all(&tmp).expect("create temp dir");

        let result = workspace_sessions_dir(&tmp);
        assert!(
            result.is_ok(),
            "workspace_sessions_dir should succeed for a valid CWD, got: {result:?}"
        );
        let dir = result.unwrap();
        // The returned path should be non-empty and end with a hash component
        assert!(!dir.as_os_str().is_empty());
        // Two calls with the same CWD should produce identical paths (deterministic)
        let result2 = workspace_sessions_dir(&tmp).unwrap();
        assert_eq!(dir, result2, "workspace_sessions_dir must be deterministic");

        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn workspace_sessions_dir_differs_for_different_cwds() {
        let tmp_a = std::env::temp_dir().join("claw-session-dir-a");
        let tmp_b = std::env::temp_dir().join("claw-session-dir-b");
        fs::create_dir_all(&tmp_a).expect("create dir a");
        fs::create_dir_all(&tmp_b).expect("create dir b");

        let dir_a = workspace_sessions_dir(&tmp_a).expect("dir a");
        let dir_b = workspace_sessions_dir(&tmp_b).expect("dir b");
        assert_ne!(
            dir_a, dir_b,
            "different CWDs must produce different session dirs"
        );

        fs::remove_dir_all(&tmp_a).ok();
        fs::remove_dir_all(&tmp_b).ok();
    }
}
