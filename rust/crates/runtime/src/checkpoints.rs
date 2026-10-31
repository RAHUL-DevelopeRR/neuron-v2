//! Per-turn file checkpoints and rewind.
//!
//! Before each user turn the engine opens a checkpoint. While the turn runs,
//! every file one of the agent's file tools is about to change is saved once,
//! on first touch, so the checkpoint holds each file exactly as it was when
//! the turn started. A rewind later puts the code back, cuts the conversation
//! back to that prompt, or both, and reports anything it could not undo:
//! files changed by shell commands, files outside the workspace, and files too
//! large to save.
//!
//! # Storage
//!
//! ```text
//! <workspace>/.neuron/checkpoints/
//! ├── .gitignore               "*", so git never sees anything in here
//! └── <session-id>/
//!     ├── state.json           the next checkpoint id
//!     ├── turns/00000007.json  one record per checkpoint
//!     ├── objects/3f/a29c…     saved file contents, named by SHA-256
//!     └── shell-baseline.json  only while a turn that ran a shell command is open
//! ```
//!
//! Contents are content-addressed, so a file saved unchanged by several
//! checkpoints is stored once. A record names the prompt, the conversation
//! length before it, every saved file (or the fact that it did not exist yet),
//! the shell commands the turn ran and the files those commands changed. A
//! session keeps its newest [`DEFAULT_MAX_CHECKPOINTS`] checkpoints, and
//! checkpoints and idle session folders older than [`DEFAULT_RETENTION`] are
//! removed when a store opens.
//!
//! # Engine call sites
//!
//! The engine owns one [`CheckpointStore`] per live session. Errors from steps
//! 2 to 5 should be logged and must never fail the turn.
//!
//! 1. Session start or resume: [`CheckpointStore::open`] with the workspace
//!    root and `session.session_id`. Opening also cleans up expired
//!    checkpoints.
//! 2. Start of each user turn, before the prompt is pushed (in
//!    `ConversationRuntime::run_turn`, ahead of `push_user_text`):
//!    [`CheckpointStore::begin_turn`] with the session and the prompt.
//! 3. Before a file tool writes (`write_file`, `edit_file`, `NotebookEdit`,
//!    and any later tool that creates, edits, moves or deletes files):
//!    [`CheckpointStore::before_file_change`] for each path it may change,
//!    both ends of a move. Pass the path the tool will open; a relative path
//!    is taken against the workspace root. Read-only tools need nothing.
//! 4. Before `bash`, `PowerShell` or any other shell tool runs:
//!    [`CheckpointStore::before_shell_command`] with the command line. The
//!    first one in a turn records a quick size-and-time scan of the
//!    workspace, so the files the commands change can be reported later.
//! 5. When the turn ends, however it ends: [`CheckpointStore::finish_turn`].
//!    The next `begin_turn` does this too if it was missed.
//! 6. The `rewind` command: [`CheckpointStore::list`] for the "Go back to…"
//!    menu, [`CheckpointStore::preview_rewind`] for the confirmation, and
//!    [`CheckpointStore::rewind`] to act. [`RewindMode`] picks code and chat,
//!    chat only, or code only. [`CheckpointSummary`] and [`RewindReport`]
//!    serialise straight into protocol events, and the report also renders
//!    as plain text through `Display` for the line REPL.
//!
//! # Rewind semantics
//!
//! - Code: every file saved by the chosen checkpoint or any later one goes
//!   back to its state at the chosen prompt, byte for byte. Files that did not
//!   exist then are removed, with any folders created for them that are now
//!   empty. Files changed only by shell commands are listed, never touched.
//! - Chat: the conversation is cut to the message index the checkpoint
//!   recorded (see [`truncate_conversation`]), and the removed prompt comes
//!   back in the report so a UI can offer "edit and resend". This fails
//!   before anything changes if the conversation was compacted since.
//! - A rewind that includes chat drops the chosen checkpoint and every later
//!   one, because their prompts are gone. A code-only rewind keeps them and
//!   first saves a "before rewind" checkpoint, so it can itself be undone.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fmt::{self, Display, Formatter};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::file_ops::DEFAULT_SKIPPED_DIRS;
use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session, SessionError};
use crate::transcript::normalize_messages;

/// Folder below `<workspace>/.neuron` holding every session's checkpoints.
pub const CHECKPOINTS_DIR: &str = "checkpoints";
/// Checkpoints kept per session; the oldest are dropped first.
pub const DEFAULT_MAX_CHECKPOINTS: usize = 100;
/// Age after which checkpoints, and the folders of idle sessions, are removed.
pub const DEFAULT_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// Largest file a checkpoint saves; bigger files are reported, not saved.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Files the shell-change scan looks at before it stops and says so.
pub const DEFAULT_MAX_SCAN_FILES: usize = 20_000;

const RECORD_VERSION: u32 = 1;
const STATE_FILE: &str = "state.json";
const BASELINE_FILE: &str = "shell-baseline.json";
const TURNS_DIR: &str = "turns";
const OBJECTS_DIR: &str = "objects";
const GITIGNORE_CONTENTS: &str =
    "# Neuron checkpoints: local backups of your files. Never commit them.\n*\n";
const MAX_PROMPT_PREVIEW_CHARS: usize = 200;
const MAX_COMMAND_PREVIEW_CHARS: usize = 200;
const MAX_SHELL_COMMANDS: usize = 50;
const COPY_BUFFER_BYTES: usize = 64 * 1024;
const FOLDER_IN_THE_WAY: &str = "a folder now exists at this path";
/// Folders the shell-change scan skips on top of [`DEFAULT_SKIPPED_DIRS`]:
/// Neuron's own state, caches and virtual environments.
const SCAN_SKIPPED_DIRS: &[&str] = &[
    ".neuron",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".turbo",
    ".gradle",
    ".pytest_cache",
    ".mypy_cache",
];

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Limits for one [`CheckpointStore`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointConfig {
    /// Checkpoints kept per session.
    pub max_checkpoints: usize,
    /// Age after which checkpoints and idle session folders are removed.
    pub retention: Duration,
    /// Largest file saved, in bytes.
    pub max_file_bytes: u64,
    /// Files the shell-change scan looks at before giving up.
    pub max_scan_files: usize,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            max_checkpoints: DEFAULT_MAX_CHECKPOINTS,
            retention: DEFAULT_RETENTION,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_scan_files: DEFAULT_MAX_SCAN_FILES,
        }
    }
}

/// What a rewind puts back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewindMode {
    CodeAndChat,
    ChatOnly,
    CodeOnly,
}

impl RewindMode {
    #[must_use]
    pub fn restores_code(self) -> bool {
        matches!(self, Self::CodeAndChat | Self::CodeOnly)
    }

    #[must_use]
    pub fn restores_chat(self) -> bool {
        matches!(self, Self::CodeAndChat | Self::ChatOnly)
    }

    /// The protocol name: `code_and_chat`, `chat_only` or `code_only`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CodeAndChat => "code_and_chat",
            Self::ChatOnly => "chat_only",
            Self::CodeOnly => "code_only",
        }
    }

    /// Parses a protocol name, or a short form (`both`, `chat`, `code`).
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "code_and_chat" | "both" | "all" => Some(Self::CodeAndChat),
            "chat_only" | "chat" | "conversation" => Some(Self::ChatOnly),
            "code_only" | "code" | "files" => Some(Self::CodeOnly),
            _ => None,
        }
    }
}

impl Display for RewindMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::CodeAndChat => "code and chat",
            Self::ChatOnly => "chat only",
            Self::CodeOnly => "code only",
        })
    }
}

/// One entry of the "Go back to…" list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointSummary {
    pub id: u64,
    pub created_at_ms: u64,
    /// Conversation length before the turn's prompt; a chat rewind cuts here.
    pub message_index: usize,
    /// The turn's prompt, shortened to one line.
    pub prompt_preview: String,
    /// Set on the checkpoint a code-only rewind saves first: the checkpoint
    /// that rewind went back to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_rewind_to: Option<u64>,
    /// Workspace-relative paths (with `/`) saved during this turn.
    pub files: Vec<String>,
    /// Paths outside the workspace the turn's file tools changed.
    pub outside_workspace: Vec<String>,
    /// Shell commands the turn ran, shortened to one line each.
    pub shell_commands: Vec<String>,
}

/// What [`CheckpointStore::before_file_change`] did with a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackOutcome {
    /// The file's current contents were saved.
    Saved,
    /// The file does not exist yet, so a rewind removes it.
    Missing,
    /// Already saved earlier in this turn; the first copy is kept.
    AlreadySaved,
    /// Outside the workspace: not saved, and a rewind reports it.
    OutsideWorkspace,
    /// Could not be saved (too large, not a file, unreadable); a rewind
    /// reports it with this reason.
    NotSaved(NotRestoredReason),
    /// Inside the checkpoint folder itself, which is never tracked.
    Ignored,
    /// No turn is open: [`CheckpointStore::begin_turn`] was not called.
    NoOpenTurn,
}

/// How a shell command changed a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChange {
    Created,
    Modified,
    Deleted,
}

/// Why a file is listed in [`RewindReport::not_restored`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NotRestoredReason {
    /// A shell command changed it and no file tool saved it first, so it was
    /// left as it is now.
    ChangedByShell { change: FileChange },
    /// Restored, but to its state after a shell command had already changed
    /// it during that turn: the command's changes are still there.
    ShellChangesKept,
    /// Outside the workspace, so it was never saved.
    OutsideWorkspace,
    /// Larger than the save limit.
    TooLarge { bytes: u64, limit: u64 },
    /// Not a regular file when it was about to be saved.
    NotAFile,
    /// Could not be read when it was about to be saved.
    NotSaved { error: String },
    /// Its saved copy is missing or damaged.
    BackupMissing,
    /// Writing or removing it failed.
    RestoreFailed { error: String },
}

impl Display for NotRestoredReason {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChangedByShell { change } => f.write_str(match change {
                FileChange::Created => "created by a shell command; left in place",
                FileChange::Modified => "changed by a shell command; not restored",
                FileChange::Deleted => "deleted by a shell command; not brought back",
            }),
            Self::ShellChangesKept => {
                f.write_str("restored, but a shell command had already changed it that turn")
            }
            Self::OutsideWorkspace => f.write_str("outside the workspace; never saved"),
            Self::TooLarge { bytes, limit } => write!(
                f,
                "too large to save ({} MB; the limit is {} MB)",
                bytes.div_ceil(1024 * 1024),
                limit / (1024 * 1024)
            ),
            Self::NotAFile => f.write_str("not a regular file; never saved"),
            Self::NotSaved { error } => write!(f, "could not be saved: {error}"),
            Self::BackupMissing => f.write_str("its saved copy is missing or damaged"),
            Self::RestoreFailed { error } => write!(f, "could not be restored: {error}"),
        }
    }
}

/// A file a rewind could not fully put back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotRestored {
    /// Workspace-relative path with `/`, or an absolute path for files
    /// outside the workspace.
    pub path: String,
    pub reason: NotRestoredReason,
}

/// What [`truncate_conversation`] removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationTruncation {
    /// The conversation now has this many messages.
    pub message_index: usize,
    pub removed_messages: usize,
    /// Text of the first removed message when it is a user prompt, so a UI
    /// can put it back in the composer for "edit and resend".
    pub prompt: Option<String>,
}

/// A file the agent changed in this session, with what it was before the session first
/// changed it ([`CheckpointStore::session_files`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFile {
    /// Workspace-relative path with `/`, or an absolute path for files outside the
    /// workspace.
    pub path: String,
    pub original: OriginalState,
}

/// What a file was before the session first changed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginalState {
    /// Its contents were saved: [`CheckpointStore::saved_contents`] reads them.
    Saved { hash: String, size: u64 },
    /// It did not exist.
    Missing,
    /// A shell command changed it before any file tool saved it, so only how is known.
    ChangedByShell(FileChange),
    /// Never saved: outside the workspace, too large, unreadable or not a file.
    Unknown,
}

/// The outcome of [`CheckpointStore::rewind`] or
/// [`CheckpointStore::preview_rewind`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewindReport {
    pub checkpoint_id: u64,
    pub mode: RewindMode,
    /// True for a preview: nothing was changed.
    pub dry_run: bool,
    /// Files written back from their saved copies.
    pub restored: Vec<String>,
    /// Files removed because they did not exist at the checkpoint.
    pub removed: Vec<String>,
    /// Files that already matched the checkpoint.
    pub unchanged: Vec<String>,
    pub not_restored: Vec<NotRestored>,
    /// Shell commands run since the checkpoint; their effects were not undone.
    pub shell_commands: Vec<String>,
    /// Shell commands left out of `shell_commands` to keep records small.
    pub shell_commands_omitted: usize,
    /// The workspace had more files than the shell-change scan looks at, so
    /// `not_restored` may miss some files shell commands changed.
    pub shell_scan_incomplete: bool,
    /// Set when the mode includes chat.
    pub conversation: Option<ConversationTruncation>,
}

impl RewindReport {
    fn new(checkpoint_id: u64, mode: RewindMode, dry_run: bool) -> Self {
        Self {
            checkpoint_id,
            mode,
            dry_run,
            restored: Vec::new(),
            removed: Vec::new(),
            unchanged: Vec::new(),
            not_restored: Vec::new(),
            shell_commands: Vec::new(),
            shell_commands_omitted: 0,
            shell_scan_incomplete: false,
            conversation: None,
        }
    }
}

impl Display for RewindReport {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let verb = if self.dry_run {
            "Would go back"
        } else {
            "Went back"
        };
        writeln!(
            f,
            "{verb} to checkpoint #{} ({}).",
            self.checkpoint_id, self.mode
        )?;
        if self.mode.restores_code() {
            write_path_list(f, "Restored", &self.restored)?;
            write_path_list(f, "Removed", &self.removed)?;
            if self.restored.is_empty() && self.removed.is_empty() {
                writeln!(f, "No files needed changing.")?;
            }
        }
        if let Some(conversation) = &self.conversation {
            writeln!(
                f,
                "Conversation: {} message{} removed.",
                conversation.removed_messages,
                plural(conversation.removed_messages)
            )?;
        }
        if !self.not_restored.is_empty() {
            writeln!(f, "Not restored:")?;
            for item in &self.not_restored {
                writeln!(f, "  {}: {}", item.path, item.reason)?;
            }
        }
        if !self.shell_commands.is_empty() {
            writeln!(f, "Shell commands run since then (not undone):")?;
            for command in &self.shell_commands {
                writeln!(f, "  $ {command}")?;
            }
        }
        if self.shell_commands_omitted > 0 {
            writeln!(f, "  … and {} more", self.shell_commands_omitted)?;
        }
        if self.shell_scan_incomplete {
            writeln!(
                f,
                "The workspace has too many files to check them all for shell changes."
            )?;
        }
        Ok(())
    }
}

fn write_path_list(f: &mut Formatter<'_>, label: &str, paths: &[String]) -> fmt::Result {
    if paths.is_empty() {
        return Ok(());
    }
    writeln!(
        f,
        "{label} {} file{}: {}",
        paths.len(),
        plural(paths.len()),
        paths.join(", ")
    )
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

/// Errors from the checkpoint store. File-level problems during a rewind are
/// reported in [`RewindReport::not_restored`] instead.
#[derive(Debug)]
pub enum CheckpointError {
    Io {
        context: String,
        source: io::Error,
    },
    InvalidSessionId(String),
    SessionMismatch {
        expected: String,
        actual: String,
    },
    UnknownCheckpoint(u64),
    ConversationCompacted {
        checkpoint: u64,
    },
    ConversationChanged {
        checkpoint: u64,
        message_index: usize,
        messages: usize,
    },
    InvalidMessageIndex {
        message_index: usize,
        messages: usize,
    },
    Session(SessionError),
}

impl Display for CheckpointError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { context, source } => write!(f, "{context}: {source}"),
            Self::InvalidSessionId(id) => write!(f, "invalid session id for checkpoints: {id:?}"),
            Self::SessionMismatch { expected, actual } => write!(
                f,
                "these checkpoints belong to session {expected}, not {actual}"
            ),
            Self::UnknownCheckpoint(id) => write!(f, "no checkpoint #{id} in this session"),
            Self::ConversationCompacted { checkpoint } => write!(
                f,
                "the conversation was compacted after checkpoint #{checkpoint}, so only code can go back to it"
            ),
            Self::ConversationChanged {
                checkpoint,
                message_index,
                messages,
            } => write!(
                f,
                "the conversation no longer matches checkpoint #{checkpoint} (message {message_index} of {messages}), so only code can go back to it"
            ),
            Self::InvalidMessageIndex {
                message_index,
                messages,
            } => write!(
                f,
                "message {message_index} is past the end of the conversation ({messages} messages)"
            ),
            Self::Session(error) => write!(f, "could not save the shortened conversation: {error}"),
        }
    }
}

impl std::error::Error for CheckpointError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Session(error) => Some(error),
            _ => None,
        }
    }
}

fn io_error(context: impl Display, source: io::Error) -> CheckpointError {
    CheckpointError::Io {
        context: context.to_string(),
        source,
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CheckpointRecord {
    version: u32,
    id: u64,
    session_id: String,
    created_at_ms: u64,
    message_index: usize,
    #[serde(default)]
    compaction_count: u32,
    #[serde(default)]
    prompt_preview: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    before_rewind_to: Option<u64>,
    #[serde(default)]
    files: BTreeMap<String, SavedFile>,
    #[serde(default)]
    outside_workspace: BTreeSet<String>,
    #[serde(default)]
    shell_commands: Vec<String>,
    #[serde(default)]
    shell_commands_omitted: usize,
    #[serde(default)]
    shell_changes: BTreeMap<String, FileChange>,
    #[serde(default)]
    shell_scan_incomplete: bool,
}

impl CheckpointRecord {
    /// Rejects records this version cannot read, and any record whose paths
    /// could point outside the workspace (a checkpoint folder is just files
    /// in the workspace, so a cloned repository could ship a planted one).
    fn is_valid(&self, session_id: &str) -> bool {
        (1..=RECORD_VERSION).contains(&self.version)
            && self.session_id == session_id
            && self
                .files
                .iter()
                .all(|(key, saved)| valid_key(key) && saved.state.is_valid(key))
            && self.shell_changes.keys().all(|key| valid_key(key))
    }

    fn summary(&self) -> CheckpointSummary {
        CheckpointSummary {
            id: self.id,
            created_at_ms: self.created_at_ms,
            message_index: self.message_index,
            prompt_preview: self.prompt_preview.clone(),
            before_rewind_to: self.before_rewind_to,
            files: self.files.keys().cloned().collect(),
            outside_workspace: self.outside_workspace.iter().cloned().collect(),
            shell_commands: self.shell_commands.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedFile {
    state: SavedState,
    /// Saved after a shell command in the same turn had already changed it.
    #[serde(default)]
    after_shell: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SavedState {
    Present {
        hash: String,
        size: u64,
        #[serde(default)]
        mode: Option<u32>,
    },
    Missing {
        /// The outermost folder that did not exist either, removed again
        /// (when empty) after the file is.
        #[serde(default)]
        created_dirs_from: Option<String>,
    },
    TooLarge {
        bytes: u64,
    },
    NotAFile,
    Unreadable {
        error: String,
    },
}

impl SavedState {
    fn is_valid(&self, key: &str) -> bool {
        match self {
            Self::Present { hash, .. } => valid_hash(hash),
            Self::Missing {
                created_dirs_from: Some(dir),
            } => valid_key(dir) && is_strict_ancestor(dir, key),
            _ => true,
        }
    }

    fn original(&self) -> OriginalState {
        match self {
            Self::Present { hash, size, .. } => OriginalState::Saved {
                hash: hash.clone(),
                size: *size,
            },
            Self::Missing { .. } => OriginalState::Missing,
            Self::TooLarge { .. } | Self::NotAFile | Self::Unreadable { .. } => {
                OriginalState::Unknown
            }
        }
    }

    fn outcome(&self, limit: u64) -> TrackOutcome {
        match self {
            Self::Present { .. } => TrackOutcome::Saved,
            Self::Missing { .. } => TrackOutcome::Missing,
            Self::TooLarge { bytes } => TrackOutcome::NotSaved(NotRestoredReason::TooLarge {
                bytes: *bytes,
                limit,
            }),
            Self::NotAFile => TrackOutcome::NotSaved(NotRestoredReason::NotAFile),
            Self::Unreadable { error } => TrackOutcome::NotSaved(NotRestoredReason::NotSaved {
                error: error.clone(),
            }),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct WorkspaceScan {
    complete: bool,
    /// Workspace-relative path to `[size in bytes, modified time in ns]`.
    files: BTreeMap<String, [u64; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ShellBaseline {
    checkpoint_id: u64,
    scan: WorkspaceScan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreState {
    version: u32,
    next_id: u64,
}

enum Located {
    Inside(String),
    Outside(String),
    Ignored,
}

enum BlobOutcome {
    Stored { hash: String, size: u64 },
    TooLarge(u64),
    Unreadable(String),
}

struct PendingShellChanges {
    checkpoint_id: u64,
    changes: BTreeMap<String, FileChange>,
    complete: bool,
}

#[derive(Default)]
struct RewindPlan {
    files: Vec<PlannedFile>,
    not_restored: Vec<NotRestored>,
    shell_commands: Vec<String>,
    shell_commands_omitted: usize,
    shell_scan_incomplete: bool,
}

struct PlannedFile {
    key: String,
    action: FileAction,
    /// A shell command changed the file before its saved copy was taken.
    shell_kept: bool,
}

enum FileAction {
    Restore { hash: String, mode: Option<u32> },
    Remove { created_dirs_from: Option<String> },
}

enum FileOutcome {
    Restored,
    Removed,
    Unchanged,
    Failed(NotRestoredReason),
}

/// The checkpoints of one session in one workspace. See the module docs for
/// where the engine calls each method.
#[derive(Debug)]
pub struct CheckpointStore {
    root: PathBuf,
    session_id: String,
    dir: PathBuf,
    config: CheckpointConfig,
    records: BTreeMap<u64, CheckpointRecord>,
    next_id: u64,
    active: Option<u64>,
    baseline: Option<ShellBaseline>,
}

impl CheckpointStore {
    /// Opens (or prepares) the checkpoints of `session_id` in the workspace,
    /// with the default limits. Nothing is written until the first file is
    /// saved, but expired checkpoints are cleaned up straight away.
    pub fn open(
        workspace_root: impl AsRef<Path>,
        session_id: &str,
    ) -> Result<Self, CheckpointError> {
        Self::open_with_config(workspace_root, session_id, CheckpointConfig::default())
    }

    pub fn open_with_config(
        workspace_root: impl AsRef<Path>,
        session_id: &str,
        config: CheckpointConfig,
    ) -> Result<Self, CheckpointError> {
        if !valid_session_id(session_id) {
            return Err(CheckpointError::InvalidSessionId(session_id.to_owned()));
        }
        let given = workspace_root.as_ref();
        let canonical = fs::canonicalize(given)
            .map_err(|error| io_error(format!("workspace {}", given.display()), error))?;
        let root = simplify_path(&canonical);
        let dir = checkpoints_root(&root).join(session_id);
        let mut store = Self {
            root,
            session_id: session_id.to_owned(),
            dir,
            config,
            records: BTreeMap::new(),
            next_id: 1,
            active: None,
            baseline: None,
        };
        store.load()?;
        let now = now_ms();
        // Other sessions' folders are best-effort: a locked folder must not
        // stop this session from starting.
        let _ = cleanup_expired_at(&store.root, store.config.retention, Some(session_id), now);
        store.expire_records(now)?;
        if store.baseline.is_some() {
            // The engine stopped mid-turn after a shell command: record what
            // the commands changed before anything else happens.
            store.finish_turn()?;
        }
        Ok(store)
    }

    /// The canonical workspace root, without any `\\?\` prefix.
    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// `<workspace>/.neuron/checkpoints/<session>`.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The checkpoint of the turn in progress, if any.
    #[must_use]
    pub fn active_checkpoint(&self) -> Option<u64> {
        self.active
    }

    /// Every checkpoint of the session, oldest first.
    #[must_use]
    pub fn list(&self) -> Vec<CheckpointSummary> {
        self.records
            .values()
            .map(CheckpointRecord::summary)
            .collect()
    }

    #[must_use]
    pub fn get(&self, id: u64) -> Option<CheckpointSummary> {
        self.records.get(&id).map(CheckpointRecord::summary)
    }

    /// Every file the session's checkpoints record a change to, sorted by path, with its
    /// state before the first of them: the oldest saved copy, unless a shell command
    /// changed the file before it was saved. Shell changes of a turn still open are
    /// recorded, and listed, once it finishes.
    #[must_use]
    pub fn session_files(&self) -> Vec<SessionFile> {
        let mut files: BTreeMap<String, SessionFile> = BTreeMap::new();
        for record in self.records.values() {
            for (key, saved) in &record.files {
                let shell_change = find_shell_change(&record.shell_changes, key);
                let original = match (saved.after_shell, shell_change) {
                    (true, Some(change)) => OriginalState::ChangedByShell(change),
                    _ => saved.state.original(),
                };
                files.entry(fold_key(key)).or_insert_with(|| SessionFile {
                    path: key.clone(),
                    original,
                });
            }
            for (key, change) in &record.shell_changes {
                files.entry(fold_key(key)).or_insert_with(|| SessionFile {
                    path: key.clone(),
                    original: OriginalState::ChangedByShell(*change),
                });
            }
            for path in &record.outside_workspace {
                files.entry(fold_key(path)).or_insert_with(|| SessionFile {
                    path: path.clone(),
                    original: OriginalState::Unknown,
                });
            }
        }
        let mut files = files.into_values().collect::<Vec<_>>();
        files.sort_by(|left, right| left.path.cmp(&right.path));
        files
    }

    /// The contents saved under `hash` ([`OriginalState::Saved`]), or `None` when that
    /// copy is gone.
    #[must_use]
    pub fn saved_contents(&self, hash: &str) -> Option<Vec<u8>> {
        if !valid_hash(hash) {
            return None;
        }
        fs::read(self.object_path(hash)).ok()
    }

    /// Whether the file at `path` (as [`SessionFile::path`] gives it) now holds exactly
    /// the contents saved under `hash`.
    #[must_use]
    pub fn matches_saved(&self, path: &str, hash: &str) -> bool {
        hash_file(&self.file_path(path)).is_ok_and(|current| current == hash)
    }

    /// Where the file of [`SessionFile::path`] is on disk.
    #[must_use]
    pub fn file_path(&self, path: &str) -> PathBuf {
        if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            key_path(&self.root, path)
        }
    }

    /// Opens the checkpoint for a new user turn. Call it before the prompt is
    /// pushed, so the recorded message index is where a chat rewind cuts.
    pub fn begin_turn(
        &mut self,
        session: &Session,
        prompt: &str,
    ) -> Result<CheckpointSummary, CheckpointError> {
        self.check_session(session)?;
        self.finish_turn()?;
        self.ensure_dirs()?;
        let record = CheckpointRecord {
            version: RECORD_VERSION,
            id: self.next_id,
            session_id: self.session_id.clone(),
            created_at_ms: now_ms(),
            message_index: session.messages.len(),
            compaction_count: compaction_count(session),
            prompt_preview: one_line_preview(prompt, MAX_PROMPT_PREVIEW_CHARS),
            ..CheckpointRecord::default()
        };
        let summary = record.summary();
        let id = self.insert_record(record)?;
        self.active = Some(id);
        self.prune_over_limit()?;
        Ok(summary)
    }

    /// Saves `path` as it is now, unless this turn already saved it. Call it
    /// before every file tool that may create, change, move or delete the
    /// file, with the path the tool will open.
    pub fn before_file_change(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<TrackOutcome, CheckpointError> {
        let Some(active) = self.active else {
            return Ok(TrackOutcome::NoOpenTurn);
        };
        let key = match self.locate(path.as_ref()) {
            Located::Inside(key) => key,
            Located::Ignored => return Ok(TrackOutcome::Ignored),
            Located::Outside(display) => {
                if self.record_mut(active)?.outside_workspace.insert(display) {
                    self.write_record(active)?;
                }
                return Ok(TrackOutcome::OutsideWorkspace);
            }
        };
        if find_saved(&self.record(active)?.files, &key).is_some() {
            return Ok(TrackOutcome::AlreadySaved);
        }
        let after_shell = self.changed_since_baseline(&key);
        let state = self.save_file(&key)?;
        let outcome = state.outcome(self.config.max_file_bytes);
        self.record_mut(active)?
            .files
            .insert(key, SavedFile { state, after_shell });
        self.write_record(active)?;
        Ok(outcome)
    }

    /// Notes a shell command the agent is about to run. The first one in a
    /// turn also scans the workspace, so [`Self::finish_turn`] can tell which
    /// files the commands changed.
    pub fn before_shell_command(&mut self, command: &str) -> Result<(), CheckpointError> {
        let Some(active) = self.active else {
            return Ok(());
        };
        let record = self.record_mut(active)?;
        if record.shell_commands.len() < MAX_SHELL_COMMANDS {
            record
                .shell_commands
                .push(one_line_preview(command, MAX_COMMAND_PREVIEW_CHARS));
        } else {
            record.shell_commands_omitted += 1;
        }
        if self.baseline.is_none() {
            let baseline = ShellBaseline {
                checkpoint_id: active,
                scan: scan_workspace(&self.root, self.config.max_scan_files),
            };
            self.ensure_dirs()?;
            write_json(&self.dir.join(BASELINE_FILE), &baseline)?;
            self.baseline = Some(baseline);
        }
        self.write_record(active)
    }

    /// Closes the open turn. If it ran shell commands, the workspace is
    /// scanned again and the files they changed are recorded.
    pub fn finish_turn(&mut self) -> Result<(), CheckpointError> {
        self.active = None;
        let Some(baseline) = self.baseline.take() else {
            return Ok(());
        };
        let after = scan_workspace(&self.root, self.config.max_scan_files);
        let (changes, complete) = diff_scans(&baseline.scan, &after);
        if let Some(record) = self.records.get_mut(&baseline.checkpoint_id) {
            for (key, change) in changes {
                record.shell_changes.entry(key).or_insert(change);
            }
            record.shell_scan_incomplete |= !complete;
            self.write_record(baseline.checkpoint_id)?;
        }
        remove_if_exists(&self.dir.join(BASELINE_FILE))
    }

    /// Works out what [`Self::rewind`] would do, without changing anything.
    /// Safe to call at any time, even while a turn is open.
    pub fn preview_rewind(
        &self,
        id: u64,
        mode: RewindMode,
        session: &Session,
    ) -> Result<RewindReport, CheckpointError> {
        let record = self.record(id)?;
        if mode.restores_chat() {
            self.check_conversation(record, session)?;
        }
        let mut report = RewindReport::new(id, mode, true);
        if mode.restores_code() {
            let plan = self.plan(id, self.pending_shell_changes().as_ref());
            self.apply_plan(plan, &mut report, true);
        }
        if mode.restores_chat() {
            report.conversation = Some(preview_truncation(session, record.message_index));
        }
        Ok(report)
    }

    /// Goes back to checkpoint `id`; call it between turns. Chat problems (a
    /// compacted or changed conversation, or a failure saving it) stop the
    /// rewind before any file is touched; file problems are listed in the
    /// report.
    pub fn rewind(
        &mut self,
        id: u64,
        mode: RewindMode,
        session: &mut Session,
    ) -> Result<RewindReport, CheckpointError> {
        self.finish_turn()?;
        let record = self.record(id)?;
        if mode.restores_chat() {
            self.check_conversation(record, session)?;
        }
        let message_index = record.message_index;
        let plan = mode.restores_code().then(|| self.plan(id, None));
        let mut report = RewindReport::new(id, mode, false);
        if mode.restores_chat() {
            report.conversation = Some(truncate_conversation(session, message_index)?);
        }
        if let Some(plan) = plan {
            if !mode.restores_chat() {
                self.save_before_rewind(id, &plan, session)?;
            }
            self.apply_plan(plan, &mut report, false);
        }
        if mode.restores_chat() {
            self.discard_from(id)?;
        } else {
            self.prune_over_limit()?;
        }
        Ok(report)
    }

    /// Files shell commands changed in the open turn, not yet recorded
    /// because the turn has not finished.
    fn pending_shell_changes(&self) -> Option<PendingShellChanges> {
        let baseline = self.baseline.as_ref()?;
        let after = scan_workspace(&self.root, self.config.max_scan_files);
        let (changes, complete) = diff_scans(&baseline.scan, &after);
        Some(PendingShellChanges {
            checkpoint_id: baseline.checkpoint_id,
            changes,
            complete,
        })
    }

    fn check_session(&self, session: &Session) -> Result<(), CheckpointError> {
        if session.session_id == self.session_id {
            Ok(())
        } else {
            Err(CheckpointError::SessionMismatch {
                expected: self.session_id.clone(),
                actual: session.session_id.clone(),
            })
        }
    }

    fn check_conversation(
        &self,
        record: &CheckpointRecord,
        session: &Session,
    ) -> Result<(), CheckpointError> {
        self.check_session(session)?;
        if compaction_count(session) != record.compaction_count {
            return Err(CheckpointError::ConversationCompacted {
                checkpoint: record.id,
            });
        }
        let messages = session.messages.len();
        let starts_with_prompt = session
            .messages
            .get(record.message_index)
            .is_none_or(|message| message.role == MessageRole::User);
        if record.message_index > messages || !starts_with_prompt {
            return Err(CheckpointError::ConversationChanged {
                checkpoint: record.id,
                message_index: record.message_index,
                messages,
            });
        }
        Ok(())
    }

    fn record(&self, id: u64) -> Result<&CheckpointRecord, CheckpointError> {
        self.records
            .get(&id)
            .ok_or(CheckpointError::UnknownCheckpoint(id))
    }

    fn record_mut(&mut self, id: u64) -> Result<&mut CheckpointRecord, CheckpointError> {
        self.records
            .get_mut(&id)
            .ok_or(CheckpointError::UnknownCheckpoint(id))
    }

    fn turn_path(&self, id: u64) -> PathBuf {
        self.dir.join(TURNS_DIR).join(format!("{id:08}.json"))
    }

    fn object_path(&self, hash: &str) -> PathBuf {
        self.dir.join(OBJECTS_DIR).join(&hash[..2]).join(&hash[2..])
    }

    fn load(&mut self) -> Result<(), CheckpointError> {
        let turns = self.dir.join(TURNS_DIR);
        let entries = match fs::read_dir(&turns) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_error(turns.display(), error)),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension() != Some(OsStr::new("json")) {
                continue;
            }
            let Some(record) = read_json::<CheckpointRecord>(&path) else {
                continue;
            };
            if record.is_valid(&self.session_id) && path == self.turn_path(record.id) {
                self.records.insert(record.id, record);
            }
        }
        let newest = self.records.keys().next_back().copied().unwrap_or(0);
        let saved_next =
            read_json::<StoreState>(&self.dir.join(STATE_FILE)).map_or(1, |state| state.next_id);
        self.next_id = saved_next.max(newest + 1);
        self.baseline =
            read_json::<ShellBaseline>(&self.dir.join(BASELINE_FILE)).filter(|baseline| {
                self.records.contains_key(&baseline.checkpoint_id)
                    && baseline.scan.files.keys().all(|key| valid_key(key))
            });
        Ok(())
    }

    fn ensure_dirs(&self) -> Result<(), CheckpointError> {
        for dir in [self.dir.join(TURNS_DIR), self.dir.join(OBJECTS_DIR)] {
            fs::create_dir_all(&dir).map_err(|error| io_error(dir.display(), error))?;
        }
        let gitignore = checkpoints_root(&self.root).join(".gitignore");
        if !gitignore.exists() {
            write_atomic(&gitignore, GITIGNORE_CONTENTS.as_bytes())?;
        }
        Ok(())
    }

    fn insert_record(&mut self, record: CheckpointRecord) -> Result<u64, CheckpointError> {
        let id = record.id;
        self.next_id = id + 1;
        write_json(&self.turn_path(id), &record)?;
        write_json(
            &self.dir.join(STATE_FILE),
            &StoreState {
                version: RECORD_VERSION,
                next_id: self.next_id,
            },
        )?;
        self.records.insert(id, record);
        Ok(id)
    }

    fn write_record(&self, id: u64) -> Result<(), CheckpointError> {
        write_json(&self.turn_path(id), self.record(id)?)
    }

    fn remove_record(&mut self, id: u64) -> Result<(), CheckpointError> {
        self.records.remove(&id);
        if self.active == Some(id) {
            self.active = None;
        }
        if self
            .baseline
            .as_ref()
            .is_some_and(|baseline| baseline.checkpoint_id == id)
        {
            self.baseline = None;
            remove_if_exists(&self.dir.join(BASELINE_FILE))?;
        }
        remove_if_exists(&self.turn_path(id))
    }

    fn prune_over_limit(&mut self) -> Result<(), CheckpointError> {
        let limit = self.config.max_checkpoints.max(1);
        let excess = self.records.len().saturating_sub(limit);
        let oldest = self
            .records
            .keys()
            .take(excess)
            .copied()
            .collect::<Vec<_>>();
        for id in &oldest {
            self.remove_record(*id)?;
        }
        if !oldest.is_empty() {
            self.collect_garbage();
        }
        Ok(())
    }

    fn expire_records(&mut self, now: u64) -> Result<(), CheckpointError> {
        let cutoff = now.saturating_sub(duration_ms(self.config.retention));
        let expired = self
            .records
            .values()
            .filter(|record| record.created_at_ms < cutoff)
            .map(|record| record.id)
            .collect::<Vec<_>>();
        for id in &expired {
            self.remove_record(*id)?;
        }
        if !expired.is_empty() {
            self.collect_garbage();
        }
        Ok(())
    }

    fn discard_from(&mut self, id: u64) -> Result<(), CheckpointError> {
        let later = self
            .records
            .range(id..)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in later {
            self.remove_record(id)?;
        }
        self.collect_garbage();
        Ok(())
    }

    /// Removes saved contents no remaining checkpoint refers to, and any
    /// temporary files a crash left behind.
    fn collect_garbage(&self) {
        let referenced = self
            .records
            .values()
            .flat_map(|record| record.files.values())
            .filter_map(|saved| match &saved.state {
                SavedState::Present { hash, .. } => Some(hash.as_str()),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let Ok(prefixes) = fs::read_dir(self.dir.join(OBJECTS_DIR)) else {
            return;
        };
        for prefix in prefixes.flatten() {
            let prefix_path = prefix.path();
            if !prefix_path.is_dir() {
                let _ = fs::remove_file(&prefix_path);
                continue;
            }
            let prefix_name = prefix.file_name().to_string_lossy().into_owned();
            let Ok(objects) = fs::read_dir(&prefix_path) else {
                continue;
            };
            for object in objects.flatten() {
                let hash = format!("{prefix_name}{}", object.file_name().to_string_lossy());
                if !referenced.contains(hash.as_str()) {
                    let _ = fs::remove_file(object.path());
                }
            }
            // Succeeds only when the folder is now empty.
            let _ = fs::remove_dir(&prefix_path);
        }
    }

    fn locate(&self, path: &Path) -> Located {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let resolved = resolve_path(&absolute);
        match relative_key(&self.root, &resolved) {
            Some(key) if is_checkpoint_key(&key) => Located::Ignored,
            Some(key) => Located::Inside(key),
            None => Located::Outside(resolved.display().to_string()),
        }
    }

    /// Whether a shell command this turn changed `key` before it was saved.
    fn changed_since_baseline(&self, key: &str) -> bool {
        let Some(baseline) = &self.baseline else {
            return false;
        };
        let now = fs::symlink_metadata(key_path(&self.root, key))
            .ok()
            .filter(|metadata| !metadata.is_dir())
            .map(|metadata| fingerprint(&metadata));
        match baseline.scan.files.get(key) {
            Some(before) => now.as_ref() != Some(before),
            None => now.is_some() && baseline.scan.complete,
        }
    }

    fn save_file(&self, key: &str) -> Result<SavedState, CheckpointError> {
        let path = key_path(&self.root, key);
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(SavedState::Missing {
                    created_dirs_from: first_missing_dir(&self.root, key),
                });
            }
            Err(error) => {
                return Ok(SavedState::Unreadable {
                    error: error.to_string(),
                })
            }
        };
        if !metadata.is_file() {
            return Ok(SavedState::NotAFile);
        }
        if metadata.len() > self.config.max_file_bytes {
            return Ok(SavedState::TooLarge {
                bytes: metadata.len(),
            });
        }
        self.ensure_dirs()?;
        Ok(match self.store_blob(&path)? {
            BlobOutcome::Stored { hash, size } => SavedState::Present {
                hash,
                size,
                mode: file_mode(&metadata),
            },
            BlobOutcome::TooLarge(bytes) => SavedState::TooLarge { bytes },
            BlobOutcome::Unreadable(error) => SavedState::Unreadable { error },
        })
    }

    /// Copies `source` into the object store while hashing it, then names
    /// the copy by its hash. Problems reading `source` are reported in the
    /// outcome; problems writing the store are errors.
    fn store_blob(&self, source: &Path) -> Result<BlobOutcome, CheckpointError> {
        let mut input = match File::open(source) {
            Ok(file) => file,
            Err(error) => return Ok(BlobOutcome::Unreadable(error.to_string())),
        };
        let objects = self.dir.join(OBJECTS_DIR);
        let temp = temp_sibling(&objects.join("blob"));
        let mut output = File::create(&temp).map_err(|error| io_error(temp.display(), error))?;
        let copied = copy_hashing(&mut input, &mut output, self.config.max_file_bytes);
        drop(output);
        let (hash, size) = match copied {
            Ok(Copied::Done { hash, size }) => (hash, size),
            Ok(Copied::OverLimit(bytes)) => {
                let _ = fs::remove_file(&temp);
                return Ok(BlobOutcome::TooLarge(bytes));
            }
            Err(CopyError::Read(error)) => {
                let _ = fs::remove_file(&temp);
                return Ok(BlobOutcome::Unreadable(error.to_string()));
            }
            Err(CopyError::Write(error)) => {
                let _ = fs::remove_file(&temp);
                return Err(io_error(temp.display(), error));
            }
        };
        let destination = self.object_path(&hash);
        if destination.is_file() {
            let _ = fs::remove_file(&temp);
        } else {
            let parent = objects.join(&hash[..2]);
            let stored = fs::create_dir_all(&parent).and_then(|()| fs::rename(&temp, &destination));
            if let Err(error) = stored {
                let _ = fs::remove_file(&temp);
                if !destination.is_file() {
                    return Err(io_error(destination.display(), error));
                }
            }
        }
        Ok(BlobOutcome::Stored { hash, size })
    }

    /// For each file, the earliest copy saved at or after checkpoint `id` is
    /// its state at that prompt: a file tool saves a file before its first
    /// change in a turn, so nothing changed it earlier in that range, except
    /// shell commands, which are tracked separately. `pending` adds the shell
    /// changes of a turn that has not finished yet (for previews).
    fn plan(&self, id: u64, pending: Option<&PendingShellChanges>) -> RewindPlan {
        let mut plan = RewindPlan::default();
        let mut governing: BTreeMap<String, (&str, u64, &SavedFile)> = BTreeMap::new();
        let mut shell_changed: BTreeMap<String, (&str, u64, FileChange)> = BTreeMap::new();
        let mut outside = BTreeSet::new();
        for record in self.records.range(id..).map(|(_, record)| record) {
            for (key, saved) in &record.files {
                governing
                    .entry(fold_key(key))
                    .or_insert((key.as_str(), record.id, saved));
            }
            for (key, change) in &record.shell_changes {
                shell_changed
                    .entry(fold_key(key))
                    .or_insert((key.as_str(), record.id, *change));
            }
            outside.extend(record.outside_workspace.iter().cloned());
            plan.shell_commands
                .extend(record.shell_commands.iter().cloned());
            plan.shell_commands_omitted += record.shell_commands_omitted;
            plan.shell_scan_incomplete |= record.shell_scan_incomplete;
        }
        if let Some(pending) = pending.filter(|pending| pending.checkpoint_id >= id) {
            for (key, change) in &pending.changes {
                shell_changed.entry(fold_key(key)).or_insert((
                    key.as_str(),
                    pending.checkpoint_id,
                    *change,
                ));
            }
            plan.shell_scan_incomplete |= !pending.complete;
        }
        for (folded, (key, saved_in, saved)) in &governing {
            let shell_kept = shell_changed.get(folded).is_some_and(|(_, changed_in, _)| {
                *changed_in < *saved_in || (*changed_in == *saved_in && saved.after_shell)
            });
            let action = match &saved.state {
                SavedState::Present { hash, mode, .. } => FileAction::Restore {
                    hash: hash.clone(),
                    mode: *mode,
                },
                SavedState::Missing { created_dirs_from } => FileAction::Remove {
                    created_dirs_from: created_dirs_from.clone(),
                },
                SavedState::TooLarge { bytes } => {
                    plan.not_restored.push(not_restored(
                        key,
                        NotRestoredReason::TooLarge {
                            bytes: *bytes,
                            limit: self.config.max_file_bytes,
                        },
                    ));
                    continue;
                }
                SavedState::NotAFile => {
                    plan.not_restored
                        .push(not_restored(key, NotRestoredReason::NotAFile));
                    continue;
                }
                SavedState::Unreadable { error } => {
                    plan.not_restored.push(not_restored(
                        key,
                        NotRestoredReason::NotSaved {
                            error: error.clone(),
                        },
                    ));
                    continue;
                }
            };
            plan.files.push(PlannedFile {
                key: (*key).to_owned(),
                action,
                shell_kept,
            });
        }
        for (folded, (key, _, change)) in &shell_changed {
            if !governing.contains_key(folded) {
                plan.not_restored.push(not_restored(
                    key,
                    NotRestoredReason::ChangedByShell { change: *change },
                ));
            }
        }
        for path in &outside {
            plan.not_restored
                .push(not_restored(path, NotRestoredReason::OutsideWorkspace));
        }
        plan
    }

    /// Saves every file a code-only rewind is about to change as a new
    /// checkpoint, so going back to it undoes the rewind. Nothing is saved
    /// when no file will change.
    fn save_before_rewind(
        &mut self,
        target: u64,
        plan: &RewindPlan,
        session: &Session,
    ) -> Result<(), CheckpointError> {
        let changing = plan
            .files
            .iter()
            .filter(|planned| {
                matches!(
                    self.apply_file(planned, true),
                    FileOutcome::Restored | FileOutcome::Removed
                )
            })
            .collect::<Vec<_>>();
        if changing.is_empty() {
            return Ok(());
        }
        self.ensure_dirs()?;
        let mut record = CheckpointRecord {
            version: RECORD_VERSION,
            id: self.next_id,
            session_id: self.session_id.clone(),
            created_at_ms: now_ms(),
            message_index: session.messages.len(),
            compaction_count: compaction_count(session),
            before_rewind_to: Some(target),
            ..CheckpointRecord::default()
        };
        for planned in changing {
            let state = self.save_file(&planned.key)?;
            record.files.insert(
                planned.key.clone(),
                SavedFile {
                    state,
                    after_shell: false,
                },
            );
        }
        self.insert_record(record)?;
        Ok(())
    }

    fn apply_file(&self, planned: &PlannedFile, dry_run: bool) -> FileOutcome {
        match &planned.action {
            FileAction::Restore { hash, mode } => {
                self.restore_file(&planned.key, hash, *mode, dry_run)
            }
            FileAction::Remove { created_dirs_from } => {
                self.remove_file(&planned.key, created_dirs_from.as_deref(), dry_run)
            }
        }
    }

    fn apply_plan(&self, plan: RewindPlan, report: &mut RewindReport, dry_run: bool) {
        for planned in plan.files {
            match self.apply_file(&planned, dry_run) {
                FileOutcome::Restored => report.restored.push(planned.key.clone()),
                FileOutcome::Removed => report.removed.push(planned.key.clone()),
                FileOutcome::Unchanged => report.unchanged.push(planned.key.clone()),
                FileOutcome::Failed(reason) => {
                    report.not_restored.push(not_restored(&planned.key, reason));
                    continue;
                }
            }
            if planned.shell_kept {
                report.not_restored.push(not_restored(
                    &planned.key,
                    NotRestoredReason::ShellChangesKept,
                ));
            }
        }
        report.not_restored.extend(plan.not_restored);
        report
            .not_restored
            .sort_by(|left, right| left.path.cmp(&right.path));
        report.shell_commands = plan.shell_commands;
        report.shell_commands_omitted = plan.shell_commands_omitted;
        report.shell_scan_incomplete = plan.shell_scan_incomplete;
    }

    fn restore_file(&self, key: &str, hash: &str, mode: Option<u32>, dry_run: bool) -> FileOutcome {
        let target = key_path(&self.root, key);
        if fs::symlink_metadata(&target).is_ok_and(|metadata| metadata.is_dir()) {
            return FileOutcome::Failed(NotRestoredReason::RestoreFailed {
                error: FOLDER_IN_THE_WAY.to_owned(),
            });
        }
        if hash_file(&target).ok().as_deref() == Some(hash) {
            return FileOutcome::Unchanged;
        }
        let blob = self.object_path(hash);
        if !blob.is_file() {
            return FileOutcome::Failed(NotRestoredReason::BackupMissing);
        }
        if dry_run {
            return FileOutcome::Restored;
        }
        match write_from_blob(&blob, hash, &target, mode) {
            Ok(()) => FileOutcome::Restored,
            Err(RestoreError::Corrupt) => FileOutcome::Failed(NotRestoredReason::BackupMissing),
            Err(RestoreError::Io(error)) => FileOutcome::Failed(NotRestoredReason::RestoreFailed {
                error: error.to_string(),
            }),
        }
    }

    fn remove_file(
        &self,
        key: &str,
        created_dirs_from: Option<&str>,
        dry_run: bool,
    ) -> FileOutcome {
        let target = key_path(&self.root, key);
        let outcome = match fs::symlink_metadata(&target) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => FileOutcome::Unchanged,
            Err(error) => FileOutcome::Failed(NotRestoredReason::RestoreFailed {
                error: error.to_string(),
            }),
            Ok(metadata) if metadata.is_dir() => {
                FileOutcome::Failed(NotRestoredReason::RestoreFailed {
                    error: FOLDER_IN_THE_WAY.to_owned(),
                })
            }
            Ok(_) if dry_run => FileOutcome::Removed,
            Ok(_) => match fs::remove_file(&target) {
                Ok(()) => FileOutcome::Removed,
                Err(error) => FileOutcome::Failed(NotRestoredReason::RestoreFailed {
                    error: error.to_string(),
                }),
            },
        };
        if !dry_run && matches!(outcome, FileOutcome::Removed | FileOutcome::Unchanged) {
            if let Some(from) = created_dirs_from {
                remove_created_dirs(&self.root, key, from);
            }
        }
        outcome
    }
}

/// Cuts the conversation back to its first `message_index` messages and
/// saves it. The removed prompt is returned for "edit and resend", and the
/// prompt history used for ↑ recall is kept. If the cut lands inside a tool
/// exchange, the transcript is repaired the same way a resumed session is.
pub fn truncate_conversation(
    session: &mut Session,
    message_index: usize,
) -> Result<ConversationTruncation, CheckpointError> {
    let messages = session.messages.len();
    if message_index > messages {
        return Err(CheckpointError::InvalidMessageIndex {
            message_index,
            messages,
        });
    }
    if message_index == messages {
        return Ok(ConversationTruncation {
            message_index,
            removed_messages: 0,
            prompt: None,
        });
    }
    let original = session.messages.clone();
    let original_updated_at = session.updated_at_ms;
    let removed = session.messages.split_off(message_index);
    normalize_messages(&mut session.messages);
    session.updated_at_ms = now_ms().max(original_updated_at);
    if let Err(error) = session.persist_snapshot() {
        session.messages = original;
        session.updated_at_ms = original_updated_at;
        return Err(CheckpointError::Session(error));
    }
    Ok(ConversationTruncation {
        message_index: session.messages.len(),
        removed_messages: removed.len(),
        prompt: removed.first().and_then(prompt_text),
    })
}

/// Removes the checkpoint folders of sessions idle for longer than
/// `retention` in this workspace, returning how many were removed.
/// [`CheckpointStore::open`] runs this for every other session.
pub fn cleanup_expired(
    workspace_root: impl AsRef<Path>,
    retention: Duration,
) -> Result<usize, CheckpointError> {
    cleanup_expired_at(workspace_root.as_ref(), retention, None, now_ms())
}

/// `<workspace>/.neuron/checkpoints`.
#[must_use]
pub fn checkpoints_root(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".neuron").join(CHECKPOINTS_DIR)
}

fn cleanup_expired_at(
    root: &Path,
    retention: Duration,
    keep: Option<&str>,
    now: u64,
) -> Result<usize, CheckpointError> {
    let base = checkpoints_root(root);
    let entries = match fs::read_dir(&base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(io_error(base.display(), error)),
    };
    let cutoff = now.saturating_sub(duration_ms(retention));
    let mut removed = 0;
    for entry in entries.flatten() {
        // `file_type` does not follow symlinks, so a linked folder is skipped.
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !valid_session_id(name) || keep == Some(name) {
            continue;
        }
        let path = entry.path();
        if last_activity_ms(&path).is_some_and(|last| last < cutoff)
            && fs::remove_dir_all(&path).is_ok()
        {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Newest modification time of a session folder, its state file and its
/// records folder.
fn last_activity_ms(dir: &Path) -> Option<u64> {
    [dir.to_path_buf(), dir.join(STATE_FILE), dir.join(TURNS_DIR)]
        .iter()
        .filter_map(|path| {
            let modified = fs::metadata(path).ok()?.modified().ok()?;
            let since_epoch = modified.duration_since(UNIX_EPOCH).ok()?;
            u64::try_from(since_epoch.as_millis()).ok()
        })
        .max()
}

fn preview_truncation(session: &Session, message_index: usize) -> ConversationTruncation {
    let removed = &session.messages[message_index.min(session.messages.len())..];
    ConversationTruncation {
        message_index,
        removed_messages: removed.len(),
        prompt: removed.first().and_then(prompt_text),
    }
}

fn prompt_text(message: &ConversationMessage) -> Option<String> {
    if message.role != MessageRole::User {
        return None;
    }
    let text = message
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

fn compaction_count(session: &Session) -> u32 {
    session
        .compaction
        .as_ref()
        .map_or(0, |compaction| compaction.count)
}

fn not_restored(path: &str, reason: NotRestoredReason) -> NotRestored {
    NotRestored {
        path: path.to_owned(),
        reason,
    }
}

fn find_saved<'a>(files: &'a BTreeMap<String, SavedFile>, key: &str) -> Option<&'a SavedFile> {
    files.get(key).or_else(|| {
        let folded = fold_key(key);
        files
            .iter()
            .find(|(saved, _)| fold_key(saved) == folded)
            .map(|(_, saved)| saved)
    })
}

fn find_shell_change(changes: &BTreeMap<String, FileChange>, key: &str) -> Option<FileChange> {
    changes.get(key).copied().or_else(|| {
        let folded = fold_key(key);
        changes
            .iter()
            .find(|(changed, _)| fold_key(changed) == folded)
            .map(|(_, change)| *change)
    })
}

/// The outermost folder on the way to `key` that does not exist yet.
fn first_missing_dir(root: &Path, key: &str) -> Option<String> {
    let parts = key.split('/').collect::<Vec<_>>();
    (1..parts.len())
        .map(|depth| parts[..depth].join("/"))
        .find(|dir| !key_path(root, dir).exists())
}

/// Removes the now-empty folders from the file's parent up to `from`, which
/// were created after the checkpoint for the file that was just removed.
fn remove_created_dirs(root: &Path, key: &str, from: &str) {
    if !is_strict_ancestor(from, key) {
        return;
    }
    let mut parts = key.split('/').collect::<Vec<_>>();
    parts.pop();
    let floor = from.split('/').count();
    while parts.len() >= floor {
        if fs::remove_dir(key_path(root, &parts.join("/"))).is_err() {
            break;
        }
        parts.pop();
    }
}

enum Copied {
    Done { hash: String, size: u64 },
    OverLimit(u64),
}

enum CopyError {
    Read(io::Error),
    Write(io::Error),
}

/// Copies `reader` into `writer` while hashing, stopping once more than
/// `limit` bytes have been read.
fn copy_hashing(
    reader: &mut impl Read,
    writer: &mut impl Write,
    limit: u64,
) -> Result<Copied, CopyError> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut size = 0_u64;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(CopyError::Read(error)),
        };
        size += u64::try_from(read).unwrap_or(u64::MAX);
        if size > limit {
            return Ok(Copied::OverLimit(size));
        }
        hasher.update(&buffer[..read]);
        writer
            .write_all(&buffer[..read])
            .map_err(CopyError::Write)?;
    }
    writer.flush().map_err(CopyError::Write)?;
    Ok(Copied::Done {
        hash: hex_digest(&hasher.finalize()),
        size,
    })
}

fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    match copy_hashing(&mut file, &mut io::sink(), u64::MAX) {
        Ok(Copied::Done { hash, .. }) => Ok(hash),
        Ok(Copied::OverLimit(_)) => Err(io::Error::other("file grew past the hashing limit")),
        Err(CopyError::Read(error) | CopyError::Write(error)) => Err(error),
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

enum RestoreError {
    Corrupt,
    Io(io::Error),
}

impl From<io::Error> for RestoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Writes a saved copy back over `target` through a temporary file in the
/// same folder, so a failed restore never leaves a half-written file.
fn write_from_blob(
    blob: &Path,
    expected_hash: &str,
    target: &Path,
    mode: Option<u32>,
) -> Result<(), RestoreError> {
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::other("the path has no parent folder"))?;
    fs::create_dir_all(parent)?;
    let temp = temp_sibling(target);
    let written = copy_blob_to_temp(blob, expected_hash, target, &temp, mode);
    let result = written.and_then(|()| fs::rename(&temp, target).map_err(RestoreError::Io));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn copy_blob_to_temp(
    blob: &Path,
    expected_hash: &str,
    target: &Path,
    temp: &Path,
    mode: Option<u32>,
) -> Result<(), RestoreError> {
    let mut input = File::open(blob)?;
    let mut output = File::create(temp)?;
    let hash = match copy_hashing(&mut input, &mut output, u64::MAX) {
        Ok(Copied::Done { hash, .. }) => hash,
        Ok(Copied::OverLimit(_)) => return Err(RestoreError::Corrupt),
        Err(CopyError::Read(error) | CopyError::Write(error)) => return Err(error.into()),
    };
    drop(output);
    if hash != expected_hash {
        return Err(RestoreError::Corrupt);
    }
    // Keep the permissions the file has now; fall back to the saved ones.
    let mode = fs::metadata(target)
        .ok()
        .and_then(|metadata| file_mode(&metadata))
        .or(mode);
    if let Some(mode) = mode {
        apply_mode(temp, mode)?;
    }
    Ok(())
}

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode())
}

#[cfg(not(unix))]
fn file_mode(_metadata: &fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn apply_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn apply_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

/// Lists the workspace's files with their size and modification time,
/// skipping build output, dependencies and Neuron's own folder. Stops after
/// `max_files` files and marks the scan incomplete.
fn scan_workspace(root: &Path, max_files: usize) -> WorkspaceScan {
    let mut scan = WorkspaceScan {
        complete: true,
        files: BTreeMap::new(),
    };
    let mut pending = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            let key = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}/{name}")
            };
            if kind.is_dir() {
                if !is_scan_skipped(name) {
                    pending.push((entry.path(), key));
                }
                continue;
            }
            if scan.files.len() >= max_files {
                scan.complete = false;
                return scan;
            }
            if let Ok(metadata) = entry.metadata() {
                scan.files.insert(key, fingerprint(&metadata));
            }
        }
    }
    scan
}

fn is_scan_skipped(name: &str) -> bool {
    DEFAULT_SKIPPED_DIRS
        .iter()
        .chain(SCAN_SKIPPED_DIRS)
        .any(|skipped| *skipped == name)
}

fn fingerprint(metadata: &fs::Metadata) -> [u64; 2] {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| {
            u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
        });
    [metadata.len(), modified]
}

/// Files created, changed or deleted between two scans. When either scan
/// stopped early, only changes to files in both are trusted.
fn diff_scans(
    before: &WorkspaceScan,
    after: &WorkspaceScan,
) -> (BTreeMap<String, FileChange>, bool) {
    let complete = before.complete && after.complete;
    let mut changes = BTreeMap::new();
    for (key, old) in &before.files {
        match after.files.get(key) {
            Some(new) if new != old => {
                changes.insert(key.clone(), FileChange::Modified);
            }
            None if complete => {
                changes.insert(key.clone(), FileChange::Deleted);
            }
            _ => {}
        }
    }
    if complete {
        for key in after.files.keys() {
            if !before.files.contains_key(key) {
                changes.insert(key.clone(), FileChange::Created);
            }
        }
    }
    (changes, complete)
}

/// Drops the `\\?\` prefix Windows puts on canonical paths (and turns
/// `\\?\UNC\server\share` back into `\\server\share`), upper-cases the drive
/// letter, and folds `.` and `..` away, so the same file always gives the
/// same path. Other platforms only fold `.` and `..`.
fn simplify_path(path: &Path) -> PathBuf {
    let mut simplified = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::VerbatimDisk(letter) | Prefix::Disk(letter) => {
                    simplified.push(format!("{}:", char::from(letter).to_ascii_uppercase()));
                }
                Prefix::VerbatimUNC(server, share) | Prefix::UNC(server, share) => {
                    simplified.push(format!(
                        r"\\{}\{}",
                        server.to_string_lossy(),
                        share.to_string_lossy()
                    ));
                }
                _ => simplified.push(prefix.as_os_str()),
            },
            Component::RootDir => simplified.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    simplified.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    simplified.pop();
                }
            }
            Component::Normal(name) => simplified.push(name),
        }
    }
    simplified
}

/// Canonicalises the deepest part of `path` that exists (resolving symlinks,
/// 8.3 short names and letter case) and keeps the rest as written, so a file
/// about to be created maps to the same key it will have once it exists.
fn resolve_path(path: &Path) -> PathBuf {
    let simplified = simplify_path(path);
    let mut existing = simplified.as_path();
    let mut missing = Vec::new();
    loop {
        if let Ok(canonical) = fs::canonicalize(existing) {
            let mut resolved = simplify_path(&canonical);
            for name in missing.iter().rev() {
                resolved.push(name);
            }
            return resolved;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_os_string());
                existing = parent;
            }
            _ => return simplified,
        }
    }
}

/// `path` relative to `root`, joined with `/`, or `None` when it is not
/// strictly inside `root` or a name is not valid UTF-8.
fn relative_key(root: &Path, path: &Path) -> Option<String> {
    let mut rest = path.components();
    for expected in root.components() {
        let actual = rest.next()?;
        if fold_name(expected.as_os_str()) != fold_name(actual.as_os_str()) {
            return None;
        }
    }
    let mut parts = Vec::new();
    for component in rest {
        match component {
            Component::Normal(name) => parts.push(name.to_str()?),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn key_path(root: &Path, key: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in key.split('/') {
        path.push(part);
    }
    path
}

/// File names compare without case on Windows and macOS, whose default file
/// systems ignore it.
const CASE_INSENSITIVE_PATHS: bool = cfg!(any(windows, target_os = "macos"));

fn fold_name(name: &OsStr) -> String {
    let name = name.to_string_lossy();
    if CASE_INSENSITIVE_PATHS {
        name.to_lowercase()
    } else {
        name.into_owned()
    }
}

fn fold_key(key: &str) -> String {
    if CASE_INSENSITIVE_PATHS {
        key.to_lowercase()
    } else {
        key.to_owned()
    }
}

fn is_checkpoint_key(key: &str) -> bool {
    let folded = fold_key(key);
    let base = format!(".neuron/{CHECKPOINTS_DIR}");
    folded == base || folded.starts_with(&format!("{base}/"))
}

/// A stored path: `/`-separated plain names, nothing that climbs out of the
/// workspace or names a drive or root.
fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.split('/').all(|part| {
            let mut components = Path::new(part).components();
            !part.contains('\\')
                && matches!(components.next(), Some(Component::Normal(name)) if name == part)
                && components.next().is_none()
        })
}

fn is_strict_ancestor(dir: &str, key: &str) -> bool {
    key.len() > dir.len() && key.starts_with(dir) && key.as_bytes()[dir.len()] == b'/'
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id != "."
        && id != ".."
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn one_line_preview(text: &str, max_chars: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= max_chars {
        return line;
    }
    let mut shortened = line.chars().take(max_chars).collect::<String>();
    shortened.push('…');
    shortened
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn temp_sibling(path: &Path) -> PathBuf {
    let name = path.file_name().map_or_else(
        || "file".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(
        ".{name}.neuron-tmp-{}-{counter}",
        std::process::id()
    ))
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), CheckpointError> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| io_error(path.display(), io::Error::other(error)))?;
    write_atomic(path, &bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), CheckpointError> {
    let temp = temp_sibling(path);
    if let Err(error) = fs::write(&temp, bytes).and_then(|()| fs::rename(&temp, path)) {
        let _ = fs::remove_file(&temp);
        return Err(io_error(path.display(), error));
    }
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<(), CheckpointError> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            Err(io_error(path.display(), error))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    /// A temporary workspace folder, removed when dropped.
    struct TempWorkspace(PathBuf);

    impl TempWorkspace {
        fn new(label: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "neuron-checkpoints-{label}-{}-{nanos}-{counter}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create workspace");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn file(&self, relative: &str) -> PathBuf {
            key_path(&self.0, relative)
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, bytes).expect("write file");
    }

    fn read(path: &Path) -> Vec<u8> {
        fs::read(path).expect("read file")
    }

    fn open(workspace: &TempWorkspace, session: &Session) -> CheckpointStore {
        CheckpointStore::open(workspace.path(), &session.session_id).expect("open store")
    }

    /// One agent turn: open the checkpoint, push the prompt, let "file tools"
    /// write each file (`None` deletes it), answer, and close the turn.
    fn agent_turn(
        store: &mut CheckpointStore,
        session: &mut Session,
        prompt: &str,
        writes: &[(&Path, Option<&[u8]>)],
    ) -> u64 {
        let checkpoint = store.begin_turn(session, prompt).expect("begin turn");
        session.push_user_text(prompt).expect("push prompt");
        for (path, contents) in writes {
            store.before_file_change(path).expect("save before change");
            match contents {
                Some(bytes) => write(path, bytes),
                None => fs::remove_file(path).expect("delete file"),
            }
        }
        session
            .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
                text: format!("done: {prompt}"),
            }]))
            .expect("push answer");
        store.finish_turn().expect("finish turn");
        checkpoint.id
    }

    /// Opens a turn by hand, for tests that interleave shell commands.
    fn open_turn(store: &mut CheckpointStore, session: &mut Session, prompt: &str) -> u64 {
        let id = store.begin_turn(session, prompt).expect("begin turn").id;
        session.push_user_text(prompt).expect("push prompt");
        id
    }

    fn binary_pattern(len: usize, seed: u8) -> Vec<u8> {
        (0..len)
            .map(|index| {
                let low = u8::try_from(index % 251).expect("fits");
                low.wrapping_mul(31).wrapping_add(seed)
            })
            .collect()
    }

    fn object_count(store: &CheckpointStore) -> usize {
        fs::read_dir(store.dir().join(OBJECTS_DIR)).map_or(0, |prefixes| {
            prefixes
                .flatten()
                .filter(|prefix| prefix.path().is_dir())
                .map(|prefix| fs::read_dir(prefix.path()).map_or(0, Iterator::count))
                .sum()
        })
    }

    fn reasons_for<'a>(report: &'a RewindReport, path: &str) -> Vec<&'a NotRestoredReason> {
        report
            .not_restored
            .iter()
            .filter(|item| item.path == path)
            .map(|item| &item.reason)
            .collect()
    }

    fn changed_by_shell(change: FileChange) -> NotRestoredReason {
        NotRestoredReason::ChangedByShell { change }
    }

    #[test]
    fn checkpoints_rewind_three_turns_restores_originals_and_removes_new_files() {
        let workspace = TempWorkspace::new("three-turns");
        let (a, b, c) = (
            workspace.file("a.txt"),
            workspace.file("assets/b.bin"),
            workspace.file("src/c.rs"),
        );
        let a_original = b"alpha\r\nline two without newline".to_vec();
        let b_original = binary_pattern(4096, 7);
        let c_original = b"fn main() {}\n".to_vec();
        write(&a, &a_original);
        write(&b, &b_original);
        write(&c, &c_original);
        let new_one = workspace.file("new1.txt");
        let new_two = workspace.file("generated/deep/new2.txt");
        let b_changed = binary_pattern(5000, 99);

        let mut session = Session::new();
        let mut store = open(&workspace, &session);
        let first = agent_turn(
            &mut store,
            &mut session,
            "turn one",
            &[(&a, Some(b"alpha v2\n")), (&new_one, Some(b"brand new\n"))],
        );
        agent_turn(
            &mut store,
            &mut session,
            "turn two",
            &[(&b, Some(&b_changed)), (&new_two, Some(b"nested\n"))],
        );
        agent_turn(
            &mut store,
            &mut session,
            "turn three",
            &[
                (&c, Some(b"fn main() { panic!() }\n")),
                (&a, Some(b"alpha v3\n")),
            ],
        );
        assert_eq!(store.list().len(), 3);
        assert_eq!(session.messages.len(), 6);

        let report = store
            .rewind(first, RewindMode::CodeAndChat, &mut session)
            .expect("rewind");

        assert_eq!(read(&a), a_original);
        assert_eq!(read(&b), b_original);
        assert_eq!(read(&c), c_original);
        assert!(!new_one.exists());
        assert!(!workspace.file("generated").exists());
        assert_eq!(report.restored, vec!["a.txt", "assets/b.bin", "src/c.rs"]);
        assert_eq!(report.removed, vec!["generated/deep/new2.txt", "new1.txt"]);
        assert!(report.not_restored.is_empty(), "{report}");
        let conversation = report.conversation.expect("chat was rewound");
        assert_eq!(conversation.removed_messages, 6);
        assert_eq!(conversation.prompt.as_deref(), Some("turn one"));
        assert_eq!(session.messages.len(), 0);
        assert!(store.list().is_empty(), "later checkpoints are dropped");
        assert_eq!(object_count(&store), 0, "unreferenced copies are removed");
    }

    #[test]
    fn checkpoints_code_only_rewind_keeps_chat_and_can_be_undone() {
        let workspace = TempWorkspace::new("code-only");
        let file = workspace.file("notes.md");
        write(&file, b"v1\n");
        let mut session = Session::new();
        let mut store = open(&workspace, &session);
        agent_turn(&mut store, &mut session, "one", &[(&file, Some(b"v2\n"))]);
        let second = agent_turn(&mut store, &mut session, "two", &[(&file, Some(b"v3\n"))]);

        let report = store
            .rewind(second, RewindMode::CodeOnly, &mut session)
            .expect("rewind code");
        assert_eq!(read(&file), b"v2\n");
        assert_eq!(report.restored, vec!["notes.md"]);
        assert!(report.conversation.is_none());
        assert_eq!(session.messages.len(), 4, "chat is untouched");

        let checkpoints = store.list();
        assert_eq!(checkpoints.len(), 3);
        let undo = checkpoints.last().expect("before-rewind checkpoint");
        assert_eq!(undo.before_rewind_to, Some(second));
        store
            .rewind(undo.id, RewindMode::CodeOnly, &mut session)
            .expect("undo the rewind");
        assert_eq!(read(&file), b"v3\n");

        let before = store.list().len();
        let again = store
            .rewind(undo.id, RewindMode::CodeOnly, &mut session)
            .expect("rewind with nothing to change");
        assert_eq!(again.unchanged, vec!["notes.md"]);
        assert_eq!(
            store.list().len(),
            before,
            "no empty before-rewind checkpoint"
        );
    }

    #[test]
    fn checkpoints_chat_only_rewind_leaves_files_alone() {
        let workspace = TempWorkspace::new("chat-only");
        let file = workspace.file("keep.txt");
        write(&file, b"before\n");
        let mut session = Session::new();
        let mut store = open(&workspace, &session);
        agent_turn(&mut store, &mut session, "first", &[]);
        let second = agent_turn(
            &mut store,
            &mut session,
            "second",
            &[(&file, Some(b"after\n"))],
        );

        let report = store
            .rewind(second, RewindMode::ChatOnly, &mut session)
            .expect("rewind chat");
        assert_eq!(read(&file), b"after\n");
        assert!(report.restored.is_empty() && report.removed.is_empty());
        assert_eq!(session.messages.len(), 2);
        assert_eq!(
            report.conversation.and_then(|chat| chat.prompt).as_deref(),
            Some("second")
        );
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn checkpoints_preview_changes_nothing() {
        let workspace = TempWorkspace::new("preview");
        let file = workspace.file("a.txt");
        write(&file, b"one");
        let mut session = Session::new();
        let mut store = open(&workspace, &session);
        let first = agent_turn(&mut store, &mut session, "edit", &[(&file, Some(b"two"))]);

        let report = store
            .preview_rewind(first, RewindMode::CodeAndChat, &session)
            .expect("preview");
        assert!(report.dry_run);
        assert_eq!(report.restored, vec!["a.txt"]);
        assert_eq!(
            report
                .conversation
                .as_ref()
                .map(|chat| chat.removed_messages),
            Some(2)
        );
        assert_eq!(read(&file), b"two");
        assert_eq!(session.messages.len(), 2);
        assert_eq!(store.list().len(), 1);
        assert!(report
            .to_string()
            .starts_with("Would go back to checkpoint #1"));
    }

    #[test]
    fn checkpoints_list_the_session_files_with_their_state_before_the_session() {
        let workspace = TempWorkspace::new("session-files");
        let (a, b, new) = (
            workspace.file("a.txt"),
            workspace.file("b.txt"),
            workspace.file("src/new.rs"),
        );
        write(&a, b"one\n");
        write(&b, b"keep\n");
        let mut session = Session::new();
        let mut store = open(&workspace, &session);
        agent_turn(
            &mut store,
            &mut session,
            "first",
            &[(&a, Some(b"two\n")), (&new, Some(b"fn new() {}\n"))],
        );
        open_turn(&mut store, &mut session, "second");
        store.before_file_change(&a).expect("save a");
        write(&a, b"three\n");
        store.before_file_change(&b).expect("save b");
        write(&b, b"keep\n");
        store.before_shell_command("make").expect("shell");
        write(&workspace.file("made.o"), b"object");
        // Still open: the shell's changes are not known yet.
        assert_eq!(store.session_files().len(), 3);
        store.finish_turn().expect("finish");

        let files = store.session_files();
        let paths = files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, ["a.txt", "b.txt", "made.o", "src/new.rs"]);
        let OriginalState::Saved { hash, size } = &files[0].original else {
            panic!("a.txt was saved: {files:?}");
        };
        assert_eq!(*size, 4);
        assert_eq!(store.saved_contents(hash).as_deref(), Some(&b"one\n"[..]));
        assert!(!store.matches_saved("a.txt", hash));
        let OriginalState::Saved { hash, .. } = &files[1].original else {
            panic!("b.txt was saved: {files:?}");
        };
        assert!(store.matches_saved("b.txt", hash), "written back unchanged");
        assert_eq!(
            files[2].original,
            OriginalState::ChangedByShell(FileChange::Created)
        );
        assert_eq!(files[3].original, OriginalState::Missing);
        assert_eq!(store.file_path("src/new.rs"), new);
        assert_eq!(store.saved_contents("not-a-hash"), None);
    }

    #[test]
    fn checkpoints_preview_mid_turn_keeps_the_turn_open() {
        let workspace = TempWorkspace::new("preview-open");
        let file = workspace.file("a.txt");
        write(&file, b"one");
        let mut session = Session::new();
        let mut store = open(&workspace, &session);
        let first = open_turn(&mut store, &mut session, "work");
        store.before_shell_command("make").expect("shell");
        write(&workspace.file("made.o"), b"object");

        let report = store
            .preview_rewind(first, RewindMode::CodeOnly, &session)
            .expect("preview");
        let created = changed_by_shell(FileChange::Created);
        assert_eq!(reasons_for(&report, "made.o"), vec![&created]);
        assert_eq!(store.active_checkpoint(), Some(first));
        assert_eq!(
            store.before_file_change(&file).expect("track"),
            TrackOutcome::Saved
        );
    }

    #[test]
    fn checkpoints_leave_files_alone_when_the_chat_cannot_be_saved() {
        let workspace = TempWorkspace::new("chat-save-fails");
        let file = workspace.file("a.txt");
        write(&file, b"one");
        let mut session = Session::new();
        let mut store = open(&workspace, &session);
        let first = agent_turn(&mut store, &mut session, "edit", &[(&file, Some(b"two"))]);
        // A folder where the session file should be makes saving fail.
        let blocked = workspace.file("session-is-a-folder");
        fs::create_dir_all(&blocked).expect("folder");
        let mut session = session.with_persistence_path(&blocked);

        let error = store
            .rewind(first, RewindMode::CodeAndChat, &mut session)
            .expect_err("saving the chat fails");
        assert!(matches!(error, CheckpointError::Session(_)), "{error}");
        assert_eq!(read(&file), b"two");
        assert_eq!(session.messages.len(), 2);
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn checkpoints_binary_and_files_over_five_megabytes_round_trip() {
        let workspace = TempWorkspace::new("large");
        let big = workspace.file("data/model.bin");
        let original = binary_pattern(6 * 1024 * 1024 + 17, 3);
        write(&big, &original);
        let mut session = Session::new();
        let mut store = open(&workspace, &session);
        let replacement = binary_pattern(1024, 200);
        let first = agent_turn(
            &mut store,
            &mut session,
            "rewrite the model",
            &[(&big, Some(&replacement))],
        );

        let report = store
            .rewind(first, RewindMode::CodeOnly, &mut session)
            .expect("rewind");
        assert_eq!(report.restored, vec!["data/model.bin"]);
        assert!(read(&big) == original, "6 MB binary restored byte for byte");
    }

    #[test]
    fn checkpoints_report_files_over_the_size_limit() {
        let workspace = TempWorkspace::new("over-limit");
        let huge = workspace.file("huge.bin");
        write(&huge, &binary_pattern(2 * 1024 * 1024, 1));
        let mut session = Session::new();
        let config = CheckpointConfig {
            max_file_bytes: 1024 * 1024,
            ..CheckpointConfig::default()
        };
        let mut store =
            CheckpointStore::open_with_config(workspace.path(), &session.session_id, config)
                .expect("open");
        let first = open_turn(&mut store, &mut session, "grow it");
        let outcome = store.before_file_change(&huge).expect("track");
        assert!(matches!(
            outcome,
            TrackOutcome::NotSaved(NotRestoredReason::TooLarge { .. })
        ));
        write(&huge, b"small now");
        store.finish_turn().expect("finish");

        let report = store
            .rewind(first, RewindMode::CodeOnly, &mut session)
            .expect("rewind");
        assert_eq!(read(&huge), b"small now");
        let too_large = NotRestoredReason::TooLarge {
            bytes: 2 * 1024 * 1024,
            limit: 1024 * 1024,
        };
        assert_eq!(reasons_for(&report, "huge.bin"), vec![&too_large]);
        assert!(report.to_string().contains("huge.bin: too large to save"));
    }

    #[test]
    fn checkpoints_report_lists_files_changed_by_shell_commands() {
        let workspace = TempWorkspace::new("shell");
        let readme = workspace.file("README.md");
        let old_log = workspace.file("old.log");
        let tracked = workspace.file("src/lib.rs");
        write(&readme, b"# readme\n");
        write(&old_log, b"log\n");
        write(&tracked, b"pub fn a() {}\n");
        write(&workspace.file("target/debug/app"), b"binary");
        let mut session = Session::new();
        let mut store = open(&workspace, &session);

        let first = open_turn(&mut store, &mut session, "build and tidy");
        store.before_file_change(&tracked).expect("track");
        write(&tracked, b"pub fn a() { todo!() }\n");
        store
            .before_shell_command("cargo build && echo done > notes.txt")
            .expect("shell");
        // What the shell command did:
        write(&readme, b"# readme, rewritten by a script\n");
        write(&workspace.file("notes.txt"), b"done\n");
        fs::remove_file(&old_log).expect("delete log");
        write(&workspace.file("target/debug/app"), b"rebuilt binary");
        store.finish_turn().expect("finish");

        let report = store
            .rewind(first, RewindMode::CodeOnly, &mut session)
            .expect("rewind");
        assert_eq!(read(&tracked), b"pub fn a() {}\n");
        assert_eq!(report.restored, vec!["src/lib.rs"]);
        let modified = changed_by_shell(FileChange::Modified);
        let created = changed_by_shell(FileChange::Created);
        let deleted = changed_by_shell(FileChange::Deleted);
        assert_eq!(reasons_for(&report, "README.md"), vec![&modified]);
        assert_eq!(reasons_for(&report, "notes.txt"), vec![&created]);
        assert_eq!(reasons_for(&report, "old.log"), vec![&deleted]);
        assert_eq!(reasons_for(&report, "src/lib.rs").len(), 0);
        assert!(
            reasons_for(&report, "target/debug/app").is_empty(),
            "build output"
        );
        assert_eq!(
            report.shell_commands,
            vec!["cargo build && echo done > notes.txt"]
        );
        assert_eq!(read(&readme), b"# readme, rewritten by a script\n");
        let text = report.to_string();
        assert!(text.contains("README.md: changed by a shell command; not restored"));
        assert!(text.contains("$ cargo build"));
    }

    #[test]
    fn checkpoints_flag_files_a_shell_command_changed_before_they_were_saved() {
        let workspace = TempWorkspace::new("shell-first");
        let config_file = workspace.file("config.toml");
        let untouched_by_shell = workspace.file("other.txt");
        write(&config_file, b"a = 1\n");
        write(&untouched_by_shell, b"x\n");
        let mut session = Session::new();
        let mut store = open(&workspace, &session);

        let first = open_turn(&mut store, &mut session, "format");
        store.before_shell_command("taplo fmt").expect("shell");
        write(&config_file, b"a = 1 # formatted by the shell\n");
        store.before_file_change(&config_file).expect("track");
        write(&config_file, b"a = 2\n");
        store
            .before_file_change(&untouched_by_shell)
            .expect("track");
        write(&untouched_by_shell, b"y, a longer line\n");
        store.finish_turn().expect("finish");

        let report = store
            .rewind(first, RewindMode::CodeOnly, &mut session)
            .expect("rewind");
        assert_eq!(read(&config_file), b"a = 1 # formatted by the shell\n");
        assert_eq!(
            reasons_for(&report, "config.toml"),
            vec![&NotRestoredReason::ShellChangesKept]
        );
        assert_eq!(read(&untouched_by_shell), b"x\n");
        assert!(reasons_for(&report, "other.txt").is_empty(), "{report}");
    }

    #[test]
    fn checkpoints_report_files_outside_the_workspace() {
        let workspace = TempWorkspace::new("inside");
        let elsewhere = TempWorkspace::new("outside");
        let outside_file = elsewhere.file("shared.txt");
        write(&outside_file, b"shared\n");
        let mut session = Session::new();
        let mut store = open(&workspace, &session);

        let first = open_turn(&mut store, &mut session, "touch shared");
        assert_eq!(
            store.before_file_change(&outside_file).expect("track"),
            TrackOutcome::OutsideWorkspace
        );
        let climbing = PathBuf::from("..").join("sibling.txt");
        assert_eq!(
            store.before_file_change(&climbing).expect("track"),
            TrackOutcome::OutsideWorkspace
        );
        write(&outside_file, b"changed\n");
        store.finish_turn().expect("finish");

        let report = store
            .rewind(first, RewindMode::CodeOnly, &mut session)
            .expect("rewind");
        let outside = report
            .not_restored
            .iter()
            .filter(|item| item.reason == NotRestoredReason::OutsideWorkspace)
            .count();
        assert_eq!(outside, 2, "{report}");
        assert_eq!(read(&outside_file), b"changed\n");
    }

    #[test]
    fn checkpoints_keep_the_first_copy_saved_in_a_turn() {
        let workspace = TempWorkspace::new("first-copy");
        let file = workspace.file("a.txt");
        write(&file, b"original");
        let mut session = Session::new();
        let mut store = open(&workspace, &session);
        let first = open_turn(&mut store, &mut session, "twice");
        assert_eq!(
            store.before_file_change(&file).expect("track"),
            TrackOutcome::Saved
        );
        write(&file, b"edit one");
        assert_eq!(
            store.before_file_change(&file).expect("track"),
            TrackOutcome::AlreadySaved
        );
        write(&file, b"edit two");
        store.finish_turn().expect("finish");

        store
            .rewind(first, RewindMode::CodeOnly, &mut session)
            .expect("rewind");
        assert_eq!(read(&file), b"original");
    }

    #[test]
    fn checkpoints_store_identical_contents_once() {
        let workspace = TempWorkspace::new("dedupe");
        let one = workspace.file("one.txt");
        let two = workspace.file("two.txt");
        write(&one, b"same bytes");
        write(&two, b"same bytes");
        let mut session = Session::new();
        let mut store = open(&workspace, &session);
        agent_turn(
            &mut store,
            &mut session,
            "edit both",
            &[(&one, Some(b"a")), (&two, Some(b"b"))],
        );
        assert_eq!(object_count(&store), 1);
    }

    #[test]
    fn checkpoints_keep_only_the_newest_per_session() {
        let workspace = TempWorkspace::new("limit");
        let mut session = Session::new();
        let config = CheckpointConfig {
            max_checkpoints: 3,
            ..CheckpointConfig::default()
        };
        let mut store =
            CheckpointStore::open_with_config(workspace.path(), &session.session_id, config)
                .expect("open");
        for turn in 1..=5_u8 {
            let file = workspace.file(&format!("file{turn}.txt"));
            write(&file, &[b'a' + turn; 8]);
            agent_turn(
                &mut store,
                &mut session,
                &format!("turn {turn}"),
                &[(&file, Some(b"edited"))],
            );
        }
        let ids = store
            .list()
            .iter()
            .map(|summary| summary.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![3, 4, 5]);
        assert_eq!(
            object_count(&store),
            3,
            "copies used only by dropped turns are gone"
        );
        assert!(!store.turn_path(1).exists());
    }

    #[test]
    fn checkpoints_expire_after_thirty_days() {
        let workspace = TempWorkspace::new("expiry");
        let file = workspace.file("a.txt");
        write(&file, b"a");
        let mut old_session = Session::new();
        let mut old_store = open(&workspace, &old_session);
        agent_turn(
            &mut old_store,
            &mut old_session,
            "old",
            &[(&file, Some(b"b"))],
        );
        let old_dir = old_store.dir().to_path_buf();
        drop(old_store);

        let mut session = Session::new();
        let mut store = open(&workspace, &session);
        agent_turn(&mut store, &mut session, "new", &[(&file, Some(b"c"))]);
        let day_ms = duration_ms(Duration::from_secs(24 * 60 * 60));
        let in_29_days = now_ms() + 29 * day_ms;
        let in_31_days = now_ms() + 31 * day_ms;
        let keep = Some(session.session_id.as_str());

        let removed = cleanup_expired_at(workspace.path(), DEFAULT_RETENTION, keep, in_29_days)
            .expect("cleanup");
        assert_eq!(removed, 0);
        let removed = cleanup_expired_at(workspace.path(), DEFAULT_RETENTION, keep, in_31_days)
            .expect("cleanup");
        assert_eq!(removed, 1);
        assert!(!old_dir.exists());
        assert_eq!(store.list().len(), 1, "the open session's folder is kept");

        store.expire_records(in_31_days).expect("expire");
        assert_eq!(store.list().len(), 0);
        assert_eq!(object_count(&store), 0);
    }

    #[test]
    fn checkpoints_folder_is_ignored_by_git() {
        let workspace = TempWorkspace::new("gitignore");
        let file = workspace.file("a.txt");
        write(&file, b"a");
        let mut session = Session::new();
        let mut store = open(&workspace, &session);
        agent_turn(&mut store, &mut session, "edit", &[(&file, Some(b"b"))]);
        let gitignore = checkpoints_root(workspace.path()).join(".gitignore");
        assert_eq!(
            fs::read_to_string(&gitignore).expect("gitignore"),
            GITIGNORE_CONTENTS
        );

        let git = |args: &[&str]| {
            Command::new("git")
                .args(["-c", "core.fsmonitor=false"])
                .args(args)
                .current_dir(workspace.path())
                .output()
        };
        let Ok(init) = git(&["init", "--quiet"]) else {
            return; // git is not installed; the file check above still ran.
        };
        assert!(init.status.success());
        let status = git(&["status", "--porcelain", "--untracked-files=all"]).expect("status");
        let listed = String::from_utf8_lossy(&status.stdout);
        assert!(listed.contains("a.txt"), "{listed}");
        assert!(!listed.contains(".neuron"), "{listed}");
    }

    #[test]
    fn checkpoints_survive_an_engine_restart_mid_turn() {
        let workspace = TempWorkspace::new("restart");
        let file = workspace.file("a.txt");
        let script_output = workspace.file("out.txt");
        write(&file, b"before");
        let mut session = Session::new();
        let mut crashed = open(&workspace, &session);
        let first = open_turn(&mut crashed, &mut session, "edit then crash");
        crashed.before_file_change(&file).expect("track");
        write(&file, b"after");
        crashed
            .before_shell_command("./generate.sh")
            .expect("shell");
        write(&script_output, b"generated");
        // The engine stops here without finishing the turn.
        drop(crashed);

        let mut store = open(&workspace, &session);
        assert_eq!(store.list().len(), 1);
        let report = store
            .rewind(first, RewindMode::CodeAndChat, &mut session)
            .expect("rewind");
        assert_eq!(read(&file), b"before");
        let created = changed_by_shell(FileChange::Created);
        assert_eq!(reasons_for(&report, "out.txt"), vec![&created]);
        assert_eq!(session.messages.len(), 0);
    }

    #[test]
    fn checkpoints_refuse_chat_rewind_after_compaction() {
        let workspace = TempWorkspace::new("compacted");
        let file = workspace.file("a.txt");
        write(&file, b"one");
        let mut session = Session::new();
        let mut store = open(&workspace, &session);
        let first = agent_turn(&mut store, &mut session, "edit", &[(&file, Some(b"two"))]);
        session.record_compaction("summary", 1);

        let error = store
            .rewind(first, RewindMode::CodeAndChat, &mut session)
            .expect_err("chat cannot go back past a compaction");
        assert!(matches!(
            error,
            CheckpointError::ConversationCompacted { checkpoint } if checkpoint == first
        ));
        assert_eq!(
            read(&file),
            b"two",
            "nothing changed when the chat check failed"
        );
        store
            .rewind(first, RewindMode::CodeOnly, &mut session)
            .expect("code still works");
        assert_eq!(read(&file), b"one");
    }

    #[test]
    fn checkpoints_ignore_planted_records_that_escape_the_workspace() {
        let workspace = TempWorkspace::new("planted");
        let mut session = Session::new();
        let store = open(&workspace, &session);
        let turns = store.dir().join(TURNS_DIR);
        fs::create_dir_all(&turns).expect("turns");
        let planted = serde_json::json!({
            "version": 1,
            "id": 9,
            "session_id": session.session_id,
            "created_at_ms": now_ms(),
            "message_index": 0,
            "files": {
                "../escape.txt": { "state": { "kind": "missing" } }
            }
        });
        fs::write(turns.join("00000009.json"), planted.to_string()).expect("plant");

        let mut store = open(&workspace, &session);
        assert_eq!(store.list().len(), 0);
        let error = store
            .rewind(9, RewindMode::CodeOnly, &mut session)
            .expect_err("planted record is ignored");
        assert!(matches!(error, CheckpointError::UnknownCheckpoint(9)));
    }

    #[test]
    fn checkpoints_reject_bad_session_ids_and_other_sessions() {
        let workspace = TempWorkspace::new("ids");
        for bad in ["", "..", "../x", "a/b", r"a\b"] {
            assert!(matches!(
                CheckpointStore::open(workspace.path(), bad),
                Err(CheckpointError::InvalidSessionId(_))
            ));
        }
        let session = Session::new();
        let mut store = open(&workspace, &session);
        let other = Session::new();
        assert!(matches!(
            store.begin_turn(&other, "wrong session"),
            Err(CheckpointError::SessionMismatch { .. })
        ));
        assert_eq!(
            store
                .before_file_change(workspace.file("a.txt"))
                .expect("track"),
            TrackOutcome::NoOpenTurn
        );
    }

    #[test]
    fn checkpoints_never_track_their_own_folder() {
        let workspace = TempWorkspace::new("own-folder");
        let session = Session::new();
        let mut store = open(&workspace, &session);
        store.begin_turn(&session, "sneaky").expect("begin");
        let inside = checkpoints_root(workspace.path()).join("x.json");
        assert_eq!(
            store.before_file_change(inside).expect("track"),
            TrackOutcome::Ignored
        );
    }

    #[test]
    fn checkpoints_truncate_conversation_persists_and_validates() {
        let workspace = TempWorkspace::new("truncate");
        let path = workspace.file("session.jsonl");
        let mut session = Session::new().with_persistence_path(&path);
        for text in ["one", "two", "three"] {
            session.push_user_text(text).expect("push");
            session
                .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: format!("re: {text}"),
                }]))
                .expect("push answer");
        }

        assert!(matches!(
            truncate_conversation(&mut session, 7),
            Err(CheckpointError::InvalidMessageIndex {
                message_index: 7,
                messages: 6
            })
        ));
        let nothing = truncate_conversation(&mut session, 6).expect("no-op");
        assert_eq!(nothing.removed_messages, 0);

        let cut = truncate_conversation(&mut session, 2).expect("truncate");
        assert_eq!(cut.removed_messages, 4);
        assert_eq!(cut.prompt.as_deref(), Some("two"));
        let reloaded = Session::load_from_path(&path).expect("reload");
        assert_eq!(reloaded.messages.len(), 2);
        assert_eq!(reloaded.messages, session.messages);
    }

    #[test]
    fn checkpoints_parse_rewind_modes() {
        for mode in [
            RewindMode::CodeAndChat,
            RewindMode::ChatOnly,
            RewindMode::CodeOnly,
        ] {
            assert_eq!(RewindMode::parse(mode.as_str()), Some(mode));
            let json = serde_json::to_string(&mode).expect("serialize");
            assert_eq!(json, format!("\"{}\"", mode.as_str()));
        }
        assert_eq!(RewindMode::parse("code"), Some(RewindMode::CodeOnly));
        assert_eq!(RewindMode::parse(" Chat "), Some(RewindMode::ChatOnly));
        assert_eq!(RewindMode::parse("both"), Some(RewindMode::CodeAndChat));
        assert_eq!(RewindMode::parse("everything"), None);
    }

    #[test]
    fn checkpoints_validate_stored_keys() {
        for good in [
            "a.txt",
            "src/lib.rs",
            "dir with spaces/file name.txt",
            ".github/ci.yml",
        ] {
            assert!(valid_key(good), "{good}");
        }
        for bad in ["", "/abs", "a//b", "../x", "a/../b", "./a", r"a\b", "a/"] {
            assert!(!valid_key(bad), "{bad}");
        }
        assert!(is_strict_ancestor("a", "a/b"));
        assert!(!is_strict_ancestor("a", "ab/c"));
        assert!(!is_strict_ancestor("a/b", "a/b"));
    }

    #[cfg(windows)]
    mod windows_paths {
        use super::*;

        fn canonical_text(path: &Path) -> String {
            simplify_path(&fs::canonicalize(path).expect("canonical"))
                .display()
                .to_string()
        }

        fn verbatim(path: &Path) -> PathBuf {
            PathBuf::from(format!(r"\\?\{}", canonical_text(path)))
        }

        fn lower_drive(path: &Path) -> PathBuf {
            let text = canonical_text(path);
            let mut chars = text.chars();
            let drive = chars.next().expect("drive").to_ascii_lowercase();
            PathBuf::from(format!("{drive}{}", chars.as_str()))
        }

        #[test]
        fn checkpoints_simplify_verbatim_and_drive_prefixes() {
            let cases = [
                (r"\\?\C:\Work\x.txt", r"C:\Work\x.txt"),
                (r"c:\Work\.\a\..\x.txt", r"C:\Work\x.txt"),
                (
                    r"\\?\UNC\server\share\dir\x.txt",
                    r"\\server\share\dir\x.txt",
                ),
                (r"C:\..\x", r"C:\x"),
                (
                    r"C:\Program Files\My App\a b.txt",
                    r"C:\Program Files\My App\a b.txt",
                ),
            ];
            for (given, expected) in cases {
                assert_eq!(simplify_path(Path::new(given)), PathBuf::from(expected));
            }
            let key = relative_key(
                Path::new(r"C:\Work Space"),
                Path::new(r"c:\work space\Sub Dir\a.txt"),
            );
            assert_eq!(key.as_deref(), Some("Sub Dir/a.txt"));
            let other_drive = relative_key(Path::new(r"C:\Work"), Path::new(r"D:\Work\a.txt"));
            assert_eq!(other_drive, None);
            let sibling = relative_key(Path::new(r"C:\Work"), Path::new(r"C:\Workspace\a.txt"));
            assert_eq!(sibling, None);
        }

        #[test]
        fn checkpoints_match_the_same_file_through_any_windows_spelling() {
            let workspace = TempWorkspace::new("win paths");
            let file = workspace.file("My Docs/read me.txt");
            write(&file, b"original\r\n");
            let mut session = Session::new();
            // Open through the \\?\ form, then track through plain, lower-case
            // drive, forward-slash, upper-case, \\?\ and relative spellings.
            let mut store = CheckpointStore::open(verbatim(workspace.path()), &session.session_id)
                .expect("open via verbatim root");
            let root = store.workspace_root().display().to_string();
            assert!(!root.starts_with(r"\\?\"), "{root}");
            let first = open_turn(&mut store, &mut session, "spellings");
            assert_eq!(
                store.before_file_change(&file).expect("plain"),
                TrackOutcome::Saved
            );
            write(&file, b"changed once\r\n");
            let spellings = [
                lower_drive(&file),
                PathBuf::from(file.display().to_string().replace('\\', "/")),
                PathBuf::from(file.display().to_string().to_uppercase()),
                verbatim(&file),
                PathBuf::from(r"My Docs\read me.txt"),
            ];
            for spelling in &spellings {
                assert_eq!(
                    store.before_file_change(spelling).expect("track"),
                    TrackOutcome::AlreadySaved,
                    "{}",
                    spelling.display()
                );
            }
            write(&file, b"changed twice\r\n");
            let created = workspace.file("New Folder/new file.txt");
            let created_verbatim = verbatim(workspace.path()).join(r"New Folder\new file.txt");
            assert_eq!(
                store.before_file_change(created_verbatim).expect("new"),
                TrackOutcome::Missing
            );
            write(&created, b"new");
            store.finish_turn().expect("finish");

            let report = store
                .rewind(first, RewindMode::CodeAndChat, &mut session)
                .expect("rewind");
            assert_eq!(read(&file), b"original\r\n");
            assert!(!workspace.file("New Folder").exists());
            assert_eq!(report.restored, vec!["My Docs/read me.txt"]);
            assert_eq!(report.removed, vec!["New Folder/new file.txt"]);
        }

        #[test]
        fn checkpoints_treat_other_drives_as_outside() {
            let workspace = TempWorkspace::new("drives");
            let mut session = Session::new();
            let mut store = open(&workspace, &session);
            open_turn(&mut store, &mut session, "other drive");
            let root = store.workspace_root().display().to_string();
            let other_drive = if root.starts_with('Z') { 'Y' } else { 'Z' };
            let elsewhere = PathBuf::from(format!(r"{other_drive}:\{}", &root[3..])).join("a.txt");
            assert_eq!(
                store.before_file_change(&elsewhere).expect("track"),
                TrackOutcome::OutsideWorkspace
            );
        }
    }
}
