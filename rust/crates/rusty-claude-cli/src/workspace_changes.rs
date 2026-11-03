//! The full-screen engine's checkpoints, and what the agent changed in its session.
//!
//! [`SessionCheckpoints`] holds the session's [`CheckpointStore`] for the engine thread,
//! which opens a checkpoint before each prompt and closes it after, and for the threads
//! running tools, which save each file before a file tool changes it and note each shell
//! command ([`ChangeWatcher`]). From the store it works out the agent's changes: every
//! file a checkpoint saved or a shell command changed, compared with what it was before
//! the session first changed it.
//!
//! Checkpoint errors never fail a turn: the first one is shown as a warning, and
//! `/rewind` then has less to go back to.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use neuron_tui::protocol::{ChangedFile, CheckpointInfo, DiffFile, NotRestoredFile};
use runtime::checkpoints::{
    CheckpointError, CheckpointStore, CheckpointSummary, FileChange, OriginalState, RewindMode,
    RewindReport, SessionFile,
};
use runtime::Session;
use tools::PendingChange;

use crate::line_diff::{count_lines, diff_lines, is_binary};
use crate::source_control::{changed_file, RepoDiff, MAX_TEXT_FILE_BYTES};

/// The checkpoints of the engine's session; `None` until one opens, or when it could
/// not.
#[derive(Debug, Default)]
pub(crate) struct SessionCheckpoints {
    store: Mutex<Option<CheckpointStore>>,
    /// A checkpoint error was shown; later ones are not, so a broken store does not
    /// flood the UI.
    warned: AtomicBool,
}

impl SessionCheckpoints {
    /// Opens the checkpoints of `session_id` in `workspace`, unless they are open
    /// already.
    pub(crate) fn open(&self, workspace: &Path, session_id: &str) {
        let mut store = self.lock();
        if store
            .as_ref()
            .is_some_and(|store| store.session_id() == session_id)
        {
            return;
        }
        *store = None;
        match CheckpointStore::open(workspace, session_id) {
            Ok(opened) => *store = Some(opened),
            Err(error) => {
                drop(store);
                self.warn(&error);
            }
        }
    }

    /// Opens the checkpoint of a prompt, before `session` has it.
    pub(crate) fn begin_turn(&self, session: &Session, prompt: &str) {
        let result = self
            .lock()
            .as_mut()
            .map(|store| store.begin_turn(session, prompt).map(|_| ()));
        if let Some(Err(error)) = result {
            self.warn(&error);
        }
    }

    /// Closes the open checkpoint, if any, recording what shell commands changed.
    pub(crate) fn finish_turn(&self) {
        let result = self.lock().as_mut().map(CheckpointStore::finish_turn);
        if let Some(Err(error)) = result {
            self.warn(&error);
        }
    }

    /// A tool is about to change files: saves the file, or notes the command.
    pub(crate) fn before_change(&self, change: PendingChange<'_>) {
        let result = self.lock().as_mut().map(|store| match change {
            PendingChange::File(path) => store.before_file_change(path).map(|_| ()),
            PendingChange::Command(command) => store.before_shell_command(command),
        });
        if let Some(Err(error)) = result {
            self.warn(&error);
        }
    }

    /// Every file the agent changed in this session and is still changed, by path.
    pub(crate) fn changes(&self) -> Vec<ChangedFile> {
        let store = self.lock();
        let Some(store) = store.as_ref() else {
            return Vec::new();
        };
        store
            .session_files()
            .iter()
            .filter_map(|file| file_change(store, file, false))
            .map(|file| changed_file(&file))
            .collect()
    }

    /// The agent's changes this session with their hunks, all of them or the ones at or
    /// below `path` (relative to the workspace).
    pub(crate) fn diff(&self, path: Option<&str>) -> RepoDiff {
        let store = self.lock();
        let Some(store) = store.as_ref() else {
            return RepoDiff::default();
        };
        let wanted = path.map(|path| workspace_key(store, path));
        let files = store
            .session_files()
            .iter()
            .filter(|file| {
                wanted.as_deref().is_none_or(|wanted| {
                    wanted.is_empty()
                        || file.path == wanted
                        || file.path.starts_with(&format!("{wanted}/"))
                })
            })
            .filter_map(|file| file_change(store, file, true))
            .collect();
        RepoDiff {
            files,
            truncated: false,
        }
    }

    /// The session's checkpoints, newest first.
    pub(crate) fn list(&self) -> Vec<CheckpointSummary> {
        let mut list = self
            .lock()
            .as_ref()
            .map(CheckpointStore::list)
            .unwrap_or_default();
        list.reverse();
        list
    }

    /// Checkpoint `id`, if the session has it.
    pub(crate) fn get(&self, id: u64) -> Option<CheckpointSummary> {
        self.lock().as_ref().and_then(|store| store.get(id))
    }

    /// What going back to checkpoint `id` would do; changes nothing.
    pub(crate) fn preview_rewind(
        &self,
        id: u64,
        mode: RewindMode,
        session: &Session,
    ) -> Result<RewindReport, String> {
        let store = self.lock();
        let store = store.as_ref().ok_or_else(unavailable)?;
        store
            .preview_rewind(id, mode, session)
            .map_err(|error| error.to_string())
    }

    /// Goes back to checkpoint `id`: puts the files back and, for the chat, cuts
    /// `session` back to that prompt and saves it.
    pub(crate) fn rewind(
        &self,
        id: u64,
        mode: RewindMode,
        session: &mut Session,
    ) -> Result<RewindReport, String> {
        let mut store = self.lock();
        let store = store.as_mut().ok_or_else(unavailable)?;
        store
            .rewind(id, mode, session)
            .map_err(|error| error.to_string())
    }

    fn lock(&self) -> MutexGuard<'_, Option<CheckpointStore>> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn warn(&self, error: &CheckpointError) {
        if !self.warned.swap(true, Ordering::SeqCst) {
            runtime::ui_hooks::notice(&format!(
                "warning: checkpoints: {error}. /rewind may not be able to undo everything the agent does in this session."
            ));
        }
    }
}

/// Saves checkpoints from every thread that runs tools (`tools::set_change_watcher`)
/// until dropped.
pub(crate) struct ChangeWatcher;

impl ChangeWatcher {
    pub(crate) fn install(checkpoints: &Arc<SessionCheckpoints>) -> Self {
        let checkpoints = Arc::clone(checkpoints);
        tools::set_change_watcher(move |change| checkpoints.before_change(change));
        Self
    }
}

impl Drop for ChangeWatcher {
    fn drop(&mut self) {
        tools::clear_change_watcher();
    }
}

fn unavailable() -> String {
    "Checkpoints are not available in this session.".to_string()
}

/// `path` as the user gave it, as a key of the store: relative to the workspace, with
/// `/`, without `./` or a trailing `/`.
fn workspace_key(store: &CheckpointStore, path: &str) -> String {
    let path = path.trim();
    let relative = Path::new(path)
        .strip_prefix(store.workspace_root())
        .map_or_else(
            |_| path.to_string(),
            |relative| relative.to_string_lossy().into_owned(),
        );
    let key = relative.replace('\\', "/");
    key.trim_start_matches("./")
        .trim_end_matches('/')
        .to_string()
}

/// A file as it is now.
enum Current {
    Missing,
    Text(Vec<u8>),
    /// Larger than [`MAX_TEXT_FILE_BYTES`]: compared, not diffed.
    Large,
}

fn read_current(path: &Path) -> Current {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.len() > MAX_TEXT_FILE_BYTES => {
            Current::Large
        }
        Ok(metadata) if metadata.is_file() => {
            fs::read(path).map_or(Current::Missing, Current::Text)
        }
        _ => Current::Missing,
    }
}

/// How `file` differs from what it was before the session changed it, or `None` when
/// it is the same again. The hunks only `with_patch`. Line counts are 0 when a side
/// is unknown (a shell command changed it before it was saved), binary or too large.
fn file_change(store: &CheckpointStore, file: &SessionFile, with_patch: bool) -> Option<DiffFile> {
    let current = read_current(&store.file_path(&file.path));
    let entry = |status: &str| DiffFile {
        path: file.path.clone(),
        status: status.to_string(),
        added: 0,
        removed: 0,
        patch: String::new(),
        binary: false,
    };
    match (&file.original, current) {
        (OriginalState::Missing, Current::Missing)
        | (OriginalState::ChangedByShell(FileChange::Created), Current::Missing) => None,
        (OriginalState::Saved { hash, size }, Current::Missing) => {
            let original = (*size <= MAX_TEXT_FILE_BYTES)
                .then(|| store.saved_contents(hash))
                .flatten();
            Some(with_lines(
                entry("deleted"),
                original.as_deref(),
                Some(&[]),
                with_patch,
            ))
        }
        (OriginalState::Saved { hash, .. }, Current::Large) => {
            (!store.matches_saved(&file.path, hash)).then(|| entry("modified"))
        }
        (OriginalState::Saved { hash, size }, Current::Text(now)) => {
            let original = (*size <= MAX_TEXT_FILE_BYTES)
                .then(|| store.saved_contents(hash))
                .flatten();
            match original {
                Some(original) if original == now => None,
                Some(original) => Some(with_lines(
                    entry("modified"),
                    Some(&original),
                    Some(&now),
                    with_patch,
                )),
                None => (!store.matches_saved(&file.path, hash)).then(|| entry("modified")),
            }
        }
        (OriginalState::Missing, Current::Text(now))
        | (OriginalState::ChangedByShell(FileChange::Created), Current::Text(now)) => Some(
            with_lines(entry("created"), Some(&[]), Some(&now), with_patch),
        ),
        (
            OriginalState::Missing | OriginalState::ChangedByShell(FileChange::Created),
            Current::Large,
        ) => Some(entry("created")),
        (OriginalState::ChangedByShell(_) | OriginalState::Unknown, Current::Missing) => {
            Some(entry("deleted"))
        }
        (OriginalState::ChangedByShell(_) | OriginalState::Unknown, _) => Some(entry("modified")),
    }
}

/// `file` with the lines added and removed between `before` and `after` (and the
/// hunks, `with_patch`), when both sides are known text.
fn with_lines(
    mut file: DiffFile,
    before: Option<&[u8]>,
    after: Option<&[u8]>,
    with_patch: bool,
) -> DiffFile {
    let (Some(before), Some(after)) = (before, after) else {
        return file;
    };
    if is_binary(before) || is_binary(after) {
        file.binary = true;
        return file;
    }
    if !with_patch && before.is_empty() {
        file.added = count_lines(after);
        return file;
    }
    if !with_patch && after.is_empty() {
        file.removed = count_lines(before);
        return file;
    }
    let diff = diff_lines(
        &String::from_utf8_lossy(before),
        &String::from_utf8_lossy(after),
    );
    file.added = diff.added;
    file.removed = diff.removed;
    if with_patch {
        file.patch = diff.patch;
    }
    file
}

/// A checkpoint as the "go back to" list shows it.
pub(crate) fn checkpoint_info(summary: &CheckpointSummary) -> CheckpointInfo {
    CheckpointInfo {
        id: summary.id,
        created_at_ms: summary.created_at_ms,
        prompt_preview: summary.prompt_preview.clone(),
        files: summary.files.clone(),
        shell_commands: summary.shell_commands.clone(),
        before_rewind_to: summary.before_rewind_to,
    }
}

/// The files a rewind could not put back, with the reasons as people read them.
pub(crate) fn not_restored_files(report: &RewindReport) -> Vec<NotRestoredFile> {
    report
        .not_restored
        .iter()
        .map(|item| NotRestoredFile {
            path: item.path.clone(),
            reason: item.reason.to_string(),
        })
        .collect()
}

/// The agent's changes as one plain-text notice.
pub(crate) fn render_changes_text(files: &[ChangedFile]) -> String {
    if files.is_empty() {
        return "The agent has not changed any files in this session.".to_string();
    }
    let mut text = String::from("Files the agent changed in this session:");
    for file in files {
        text.push_str(&format!(
            "\n  {:<8} {}  +{} -{}",
            file.status, file.path, file.added, file.removed
        ));
    }
    text
}

/// The session's checkpoints (newest first) as one plain-text notice.
pub(crate) fn render_checkpoints_text(checkpoints: &[CheckpointSummary]) -> String {
    if checkpoints.is_empty() {
        return "No checkpoints yet: one is saved with each prompt.".to_string();
    }
    let mut text = String::from("Checkpoints, newest first:");
    for checkpoint in checkpoints {
        let mut line = format!("\n  #{}  {}", checkpoint.id, checkpoint.prompt_preview);
        if let Some(target) = checkpoint.before_rewind_to {
            line = format!("\n  #{}  (before going back to #{target})", checkpoint.id);
        }
        let files = checkpoint.files.len();
        if files > 0 {
            line.push_str(&format!(
                "  ({files} file{})",
                if files == 1 { "" } else { "s" }
            ));
        }
        text.push_str(&line);
    }
    text.push_str("\nGo back with /rewind <id> [code|chat|both].");
    text
}

#[cfg(test)]
mod tests {
    use super::{render_changes_text, render_checkpoints_text, SessionCheckpoints};
    use neuron_tui::protocol::ChangedFile;
    use runtime::checkpoints::RewindMode;
    use runtime::Session;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tools::PendingChange;

    struct TempWorkspace(PathBuf);

    impl TempWorkspace {
        fn new(label: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "neuron-workspace-changes-{label}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("workspace");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn file(&self, relative: &str) -> String {
            self.0.join(relative).to_string_lossy().into_owned()
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn changed(path: &str, status: &str, added: u64, removed: u64) -> ChangedFile {
        ChangedFile {
            path: path.to_string(),
            status: status.to_string(),
            added,
            removed,
        }
    }

    /// A "file tool" write as the watcher sees it: saved first, then written.
    fn tool_write(checkpoints: &SessionCheckpoints, path: &str, contents: Option<&str>) {
        checkpoints.before_change(PendingChange::File(path));
        match contents {
            Some(contents) => {
                fs::create_dir_all(Path::new(path).parent().expect("parent")).expect("folder");
                fs::write(path, contents).expect("write");
            }
            None => fs::remove_file(path).expect("remove"),
        }
    }

    #[test]
    fn changes_compare_each_file_with_the_session_original() {
        let workspace = TempWorkspace::new("changes");
        let (style, notes, old, same) = (
            workspace.file("site/style.css"),
            workspace.file("notes.md"),
            workspace.file("old.txt"),
            workspace.file("same.txt"),
        );
        fs::write(&notes, "one\ntwo\nthree\n").expect("notes");
        fs::write(&old, "a\nb\n").expect("old");
        fs::write(&same, "keep\n").expect("same");
        let mut session = Session::new();
        let checkpoints = SessionCheckpoints::default();
        checkpoints.open(workspace.path(), &session.session_id);

        checkpoints.begin_turn(&session, "style the site");
        session.push_user_text("style the site").expect("prompt");
        tool_write(&checkpoints, &style, Some("body {\n  margin: 0;\n}\n"));
        tool_write(&checkpoints, &notes, Some("one\n2\nthree\nfour\n"));
        tool_write(&checkpoints, &old, None);
        tool_write(&checkpoints, &same, Some("changed\n"));
        checkpoints.finish_turn();
        checkpoints.begin_turn(&session, "undo same");
        tool_write(&checkpoints, &same, Some("keep\n"));
        tool_write(&checkpoints, &notes, Some("one\n2\nthree\nfour\nfive\n"));
        checkpoints.finish_turn();

        assert_eq!(
            checkpoints.changes(),
            [
                changed("notes.md", "modified", 3, 1),
                changed("old.txt", "deleted", 0, 2),
                changed("site/style.css", "created", 3, 0),
            ]
        );
        let diff = checkpoints.diff(Some("site/"));
        assert_eq!(diff.files.len(), 1);
        assert_eq!(
            diff.files[0].patch,
            "@@ -0,0 +1,3 @@\n+body {\n+  margin: 0;\n+}\n"
        );
        let all = checkpoints.diff(None);
        assert_eq!(all.files.len(), 3);
        assert_eq!(all.files[1].patch, "@@ -1,2 +0,0 @@\n-a\n-b\n");

        // Newest first; going back over both prompts puts every file back.
        let list = checkpoints.list();
        assert_eq!(
            list.iter()
                .map(|item| item.prompt_preview.as_str())
                .collect::<Vec<_>>(),
            ["undo same", "style the site"]
        );
        let first = list[1].id;
        let preview = checkpoints
            .preview_rewind(first, RewindMode::CodeOnly, &session)
            .expect("preview");
        assert!(preview.dry_run && Path::new(&style).exists());
        checkpoints
            .rewind(first, RewindMode::CodeOnly, &mut session)
            .expect("rewind");
        assert_eq!(checkpoints.changes(), []);
        assert!(!Path::new(&style).exists());
        assert_eq!(
            fs::read_to_string(&notes).expect("notes"),
            "one\ntwo\nthree\n"
        );
        assert_eq!(fs::read_to_string(&old).expect("old"), "a\nb\n");
    }

    #[test]
    fn without_a_store_nothing_is_tracked_and_rewinds_say_why() {
        let checkpoints = SessionCheckpoints::default();
        checkpoints.before_change(PendingChange::Command("make"));
        assert!(checkpoints.changes().is_empty() && checkpoints.list().is_empty());
        let error = checkpoints
            .preview_rewind(1, RewindMode::CodeAndChat, &Session::new())
            .expect_err("no store");
        assert_eq!(error, "Checkpoints are not available in this session.");
    }

    #[test]
    fn plain_text_lists_name_each_change_and_checkpoint() {
        assert_eq!(
            render_changes_text(&[changed("a.rs", "created", 3, 0)]),
            "Files the agent changed in this session:\n  created  a.rs  +3 -0"
        );
        assert_eq!(
            render_changes_text(&[]),
            "The agent has not changed any files in this session."
        );
        assert!(render_checkpoints_text(&[]).starts_with("No checkpoints yet"));
    }
}
