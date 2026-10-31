//! Session management: CRUD operations for managed sessions.
//!
//! Extracted from the main module for maintainability.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use runtime::Session;

use crate::brand::*;
use crate::tool_ui::display_clean_path;

/// Lightweight session identifier + path pair.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub id: String,
    pub path: PathBuf,
}

/// Summary of a saved session for listing.
#[derive(Debug, Clone)]
pub struct ManagedSessionSummary {
    pub id: String,
    pub path: PathBuf,
    pub updated_at_ms: u64,
    pub modified_epoch_millis: u128,
    pub message_count: usize,
    pub parent_session_id: Option<String>,
    pub branch_name: Option<String>,
    /// The first prompt, on one line and shortened.
    pub title: Option<String>,
    pub model: Option<String>,
}

impl From<runtime::session_control::ManagedSessionSummary> for ManagedSessionSummary {
    fn from(session: runtime::session_control::ManagedSessionSummary) -> Self {
        Self {
            id: session.id,
            path: session.path,
            updated_at_ms: session.updated_at_ms,
            modified_epoch_millis: session.modified_epoch_millis,
            message_count: session.message_count,
            parent_session_id: session.parent_session_id,
            branch_name: session.branch_name,
            title: session.title,
            model: session.model,
        }
    }
}

pub fn sessions_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(current_session_store()?.sessions_dir().to_path_buf())
}

pub fn current_session_store() -> Result<runtime::SessionStore, Box<dyn std::error::Error>> {
    session_store_in(&env::current_dir()?)
}

/// The session store of the workspace at `cwd`.
pub fn session_store_in(cwd: &Path) -> Result<runtime::SessionStore, Box<dyn std::error::Error>> {
    runtime::SessionStore::from_cwd(cwd).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

pub fn new_cli_session() -> Result<Session, Box<dyn std::error::Error>> {
    Ok(Session::new().with_workspace_root(env::current_dir()?))
}

pub fn create_managed_session_handle(
    session_id: &str,
) -> Result<SessionHandle, Box<dyn std::error::Error>> {
    let store = current_session_store()?;
    crate::session_presence::record_project(store.workspace_root());
    let handle = store.create_handle(session_id);
    Ok(SessionHandle {
        id: handle.id,
        path: handle.path,
    })
}

pub fn resolve_session_reference(
    reference: &str,
) -> Result<SessionHandle, Box<dyn std::error::Error>> {
    resolve_session_reference_in_store(&current_session_store()?, reference)
}

pub fn resolve_session_reference_in_store(
    store: &runtime::SessionStore,
    reference: &str,
) -> Result<SessionHandle, Box<dyn std::error::Error>> {
    let handle = store
        .resolve_reference(reference)
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    Ok(SessionHandle {
        id: handle.id,
        path: handle.path,
    })
}

pub fn resolve_managed_session_path(
    session_id: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    current_session_store()?
        .resolve_managed_path(session_id)
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

pub fn list_managed_sessions() -> Result<Vec<ManagedSessionSummary>, Box<dyn std::error::Error>> {
    Ok(current_session_store()?
        .list_sessions()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?
        .into_iter()
        .map(ManagedSessionSummary::from)
        .collect())
}

/// Whether this workspace has a saved session other than the one at `current`. Cheap:
/// file names and sizes only, no session is read.
pub fn has_other_saved_session(current: &Path) -> bool {
    let Some(entries) = sessions_dir().ok().and_then(|dir| fs::read_dir(dir).ok()) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        path != current
            && path
                .extension()
                .is_some_and(|ext| ext == "jsonl" || ext == "json")
            && entry
                .metadata()
                .is_ok_and(|meta| meta.is_file() && meta.len() > 0)
    })
}

pub fn latest_managed_session() -> Result<ManagedSessionSummary, Box<dyn std::error::Error>> {
    let session = current_session_store()?
        .latest_session()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    Ok(session.into())
}

pub fn load_session_reference(
    reference: &str,
) -> Result<(SessionHandle, Session), Box<dyn std::error::Error>> {
    load_session_reference_in_store(&current_session_store()?, reference)
}

pub fn load_session_reference_in_store(
    store: &runtime::SessionStore,
    reference: &str,
) -> Result<(SessionHandle, Session), Box<dyn std::error::Error>> {
    let loaded = store
        .load_session(reference)
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    Ok((
        SessionHandle {
            id: loaded.handle.id,
            path: loaded.handle.path,
        },
        loaded.session,
    ))
}

/// This run's hold on a session file. A run that writes a session (`neuron prompt`, the
/// REPL, the full-screen UI's engine) holds one for as long as the session is its own, so
/// a second run that would continue the same session is refused instead of overwriting
/// the first one's turns: each run saves its whole session when a turn ends.
///
/// An OS lock on `<session file>.lock`, released when this is dropped or the process ends,
/// however it ends. The lock file stays: removing it while another run is opening it
/// could leave two runs holding locks on two different files.
#[derive(Debug)]
pub struct SessionLock {
    file: fs::File,
    path: PathBuf,
}

impl SessionLock {
    /// Locks the session file at `session_path` (which need not exist yet), or says
    /// that another run holds it.
    pub fn acquire(session_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let path = session_lock_path(session_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file, path }),
            Err(fs::TryLockError::WouldBlock) => Err(session_in_use(session_path).into()),
            Err(fs::TryLockError::Error(error)) => Err(error.into()),
        }
    }

    /// Lets go of the session and removes the lock file, for a session file that was
    /// removed too.
    pub fn remove(self) {
        let path = self.path.clone();
        drop(self);
        let _ = fs::remove_file(path);
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        // Closing the file unlocks it too, but Windows may take a moment to do so.
        let _ = self.file.unlock();
    }
}

/// The lock file of the session file at `session_path`: `<name>.jsonl.lock` beside it,
/// which session listings skip.
pub fn session_lock_path(session_path: &Path) -> PathBuf {
    let mut name = session_path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(".lock");
    session_path.with_file_name(name)
}

fn session_in_use(session_path: &Path) -> String {
    let id = session_path.file_stem().map_or_else(
        || session_path.display().to_string(),
        |stem| stem.to_string_lossy().into_owned(),
    );
    format!(
        "session {id} is in use by another neuron run (a terminal session, the full-screen UI \
or a script); one run at a time can continue a session, so wait for that run to end or start \
a new session"
    )
}

/// A session this run writes, held under its [`SessionLock`].
#[derive(Debug)]
pub struct OpenSession {
    pub handle: SessionHandle,
    pub session: Session,
    pub lock: SessionLock,
}

/// A new session for this run, locked, not saved yet.
pub fn new_open_session() -> Result<OpenSession, Box<dyn std::error::Error>> {
    let session = new_cli_session()?;
    let handle = create_managed_session_handle(&session.session_id)?;
    let lock = SessionLock::acquire(&handle.path)?;
    Ok(OpenSession {
        session: session.with_persistence_path(handle.path.clone()),
        handle,
        lock,
    })
}

/// The saved session `reference` names, opened for this run to continue. It is locked
/// before it is read, so what loads is what the last run saved, and no other run writes
/// it until the lock is dropped.
pub fn open_session_reference(reference: &str) -> Result<OpenSession, Box<dyn std::error::Error>> {
    let store = current_session_store()?;
    let resolved = resolve_session_reference_in_store(&store, reference)?;
    let lock = SessionLock::acquire(&resolved.path)?;
    let (handle, session) =
        load_session_reference_in_store(&store, &resolved.path.display().to_string())?;
    Ok(OpenSession {
        handle,
        session,
        lock,
    })
}

/// Whether `left` and `right` name the same file.
pub fn same_session_file(left: &Path, right: &Path) -> bool {
    left == right
        || matches!(
            (fs::canonicalize(left), fs::canonicalize(right)),
            (Ok(left), Ok(right)) if left == right
        )
}

pub fn delete_managed_session(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !path.exists() {
        return Err(format!("session file does not exist: {}", path.display()).into());
    }
    // Not while another run continues it: that run would save it again.
    let lock = SessionLock::acquire(path)?;
    fs::remove_file(path)?;
    lock.remove();
    Ok(())
}

pub fn confirm_session_deletion(session_id: &str) -> bool {
    print!("Delete session '{session_id}'? This cannot be undone. [y/N]: ");
    io::stdout().flush().unwrap_or(());
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim(), "y" | "Y" | "yes" | "Yes" | "YES")
}

pub fn render_session_list(active_session_id: &str) -> Result<String, Box<dyn std::error::Error>> {
    let sessions = list_managed_sessions()?;
    let w = 72;

    let mut lines = Vec::new();
    lines.push(box_top("Sessions", w));

    // Directory row
    let dir_display = sessions_dir()?.display().to_string();
    let dir_short = display_clean_path(&dir_display);
    lines.push(box_row(&format!("{DIM}Directory: {dir_short}{R}"), w));
    lines.push(box_separator(w));

    if sessions.is_empty() {
        lines.push(box_row(
            &format!("{DIM}No managed sessions saved yet.{R}"),
            w,
        ));
        lines.push(box_bottom(w));
        return Ok(lines.join("\n"));
    }

    // Header
    lines.push(box_row(
        &format!(
            "{BLUE}{BOLD}{id:<24} {status:<10} {msgs:<6} {modified}{R}",
            id = "SESSION",
            status = "STATUS",
            msgs = "MSGS",
            modified = "MODIFIED",
        ),
        w,
    ));
    lines.push(box_separator(w));

    // Session rows
    for session in &sessions {
        let (marker_icon, status_text) = if session.id == active_session_id {
            (ICON_ACTIVE, format!("{GREEN}current{R}"))
        } else {
            (ICON_INACTIVE, format!("{DIM}saved{R}"))
        };
        let id_display = if session.id.len() > 22 {
            format!("{}\u{2026}", &session.id[..21])
        } else {
            session.id.clone()
        };
        let modified = format_session_modified_age(session.modified_epoch_millis);
        let lineage = match (
            session.branch_name.as_deref(),
            session.parent_session_id.as_deref(),
        ) {
            (Some(branch), _) => format!(" {ORANGE}\u{23c7} {branch}{R}"),
            _ => String::new(),
        };

        let row_content = format!(
            "{marker_icon} {ORANGE}{id:<22}{R} {status:<10} {DIM}{msgs:<6}{R} {DIM}{modified}{R}{lineage}",
            id = id_display,
            status = status_text,
            msgs = session.message_count,
        );
        lines.push(box_row(&row_content, w));
    }

    lines.push(box_bottom(w));
    Ok(lines.join("\n"))
}

pub fn format_session_modified_age(modified_epoch_millis: u128) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map_or(modified_epoch_millis, |duration| duration.as_millis());
    let delta_seconds = now
        .saturating_sub(modified_epoch_millis)
        .checked_div(1_000)
        .unwrap_or_default();
    match delta_seconds {
        0..=4 => "just-now".to_string(),
        5..=59 => format!("{delta_seconds}s-ago"),
        60..=3_599 => format!("{}m-ago", delta_seconds / 60),
        3_600..=86_399 => format!("{}h-ago", delta_seconds / 3_600),
        _ => format!("{}d-ago", delta_seconds / 86_400),
    }
}

pub fn write_session_clear_backup(
    session: &Session,
    session_path: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let backup_path = session_clear_backup_path(session_path);
    session.save_to_path(&backup_path)?;
    Ok(backup_path)
}

pub fn session_clear_backup_path(session_path: &Path) -> PathBuf {
    let timestamp = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map_or(0, |duration| duration.as_millis());
    let file_name = session_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("session.jsonl");
    session_path.with_file_name(format!("{file_name}.before-clear-{timestamp}.bak"))
}

#[cfg(test)]
mod tests {
    use super::{same_session_file, session_lock_path, SessionLock};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_session(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        std::env::temp_dir()
            .join(format!("neuron-session-lock-{label}-{nanos}"))
            .join("session-1.jsonl")
    }

    #[test]
    fn a_session_is_held_by_one_run_at_a_time() {
        let session = temp_session("held");
        let held = SessionLock::acquire(&session).expect("the first run holds the session");

        let error = SessionLock::acquire(&session).expect_err("a second run is refused");
        assert!(
            error
                .to_string()
                .starts_with("session session-1 is in use by another neuron run"),
            "{error}"
        );
        drop(held);
        let again = SessionLock::acquire(&session).expect("free once the first run let go");

        // Beside the session file, with an extension session listings skip.
        let lock = session_lock_path(&session);
        assert_eq!(lock, session.with_file_name("session-1.jsonl.lock"));
        assert!(lock.is_file());
        again.remove();
        assert!(!lock.exists());
        let _ = fs::remove_dir_all(session.parent().expect("temp dir"));
    }

    #[test]
    fn the_same_session_file_is_recognised_by_any_path() {
        let session = temp_session("same");
        fs::create_dir_all(session.parent().expect("temp dir")).expect("temp dir");
        fs::write(&session, "").expect("session file");
        let roundabout = session
            .parent()
            .expect("temp dir")
            .join(".")
            .join("session-1.jsonl");

        assert!(same_session_file(&session, &roundabout));
        assert!(!same_session_file(
            &session,
            &session.with_file_name("session-2.jsonl")
        ));
        let _ = fs::remove_dir_all(session.parent().expect("temp dir"));
    }
}
