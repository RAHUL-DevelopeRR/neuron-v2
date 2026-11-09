//! Session management: CRUD operations for managed sessions.
//!
//! Extracted from the main module for maintainability.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use runtime::Session;
use sha2::{Digest, Sha256};

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
/// An OS lock on the session's lock file under `<NEURON_HOME>/locks` ([`session_lock_path`]),
/// released when this is dropped or the process ends, however it ends. Lock files are kept
/// there, not beside the session file, so `--resume <file>` leaves nothing next to a file
/// it was given. A lock file stays: removing it while another run is opening it could
/// leave two runs holding locks on two different files.
///
/// Releases before 7.0.0-alpha.4 locked `<session file>.lock` beside the file. For one
/// release that lock is honoured too: when it is there it is locked as well, so a session
/// a run of an older release is continuing is still refused. It is never created.
#[derive(Debug)]
pub struct SessionLock {
    file: fs::File,
    path: PathBuf,
    legacy: Option<(fs::File, PathBuf)>,
}

impl SessionLock {
    /// Locks the session file at `session_path` (which need not exist yet), or says
    /// that another run holds it.
    pub fn acquire(session_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let path = prepare_lock_path(session_path)?;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => return Err(session_in_use(session_path).into()),
            Err(fs::TryLockError::Error(error)) => return Err(error.into()),
        }
        let legacy = lock_legacy(session_path)?;
        Ok(Self { file, path, legacy })
    }

    /// Locks the file of a new session, which must not exist yet. `None` when its id is
    /// taken: the session file is there, or its lock file is (another run picked the same
    /// id and holds it, or held it). The lock file is created with `create_new`, so two
    /// runs that picked the same id can't both get it.
    pub fn claim_new(session_path: &Path) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        if session_path.exists() || legacy_session_lock_path(session_path).exists() {
            return Ok(None);
        }
        let path = prepare_lock_path(session_path)?;
        let file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        match file.try_lock() {
            Ok(()) => Ok(Some(Self {
                file,
                path,
                legacy: None,
            })),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(error)) => Err(error.into()),
        }
    }

    /// Lets go of the session and removes its lock files, for a session file that was
    /// removed too.
    pub fn remove(self) {
        let mut paths = vec![self.path.clone()];
        paths.extend(self.legacy.as_ref().map(|(_, path)| path.clone()));
        drop(self);
        for path in paths {
            let _ = fs::remove_file(path);
        }
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        // Closing the file unlocks it too, but Windows may take a moment to do so.
        let _ = self.file.unlock();
        if let Some((legacy, _)) = &self.legacy {
            let _ = legacy.unlock();
        }
    }
}

/// The lock file of the session file at `session_path`, with its folders made: the
/// session's own folder first, so the path it is named by is the one it will have
/// once saved.
fn prepare_lock_path(session_path: &Path) -> io::Result<PathBuf> {
    if let Some(parent) = session_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let path = session_lock_path(session_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(path)
}

/// The lock of an older release beside the session file, locked when it is there.
fn lock_legacy(
    session_path: &Path,
) -> Result<Option<(fs::File, PathBuf)>, Box<dyn std::error::Error>> {
    let path = legacy_session_lock_path(session_path);
    // Not there (the usual case), or not ours to open: nothing to honour.
    let Ok(file) = fs::OpenOptions::new().write(true).open(&path) else {
        return Ok(None);
    };
    match file.try_lock() {
        Ok(()) => Ok(Some((file, path))),
        Err(fs::TryLockError::WouldBlock) => Err(session_in_use(session_path).into()),
        Err(fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

/// Where run locks live: `<NEURON_HOME>/locks`.
fn locks_dir() -> PathBuf {
    // Unit tests never write to the developer's own Neuron home, and a test that points
    // NEURON_HOME elsewhere must not move the lock of a session another test holds.
    if cfg!(test) {
        return env::temp_dir().join(format!("neuron-test-locks-{}", std::process::id()));
    }
    crate::credentials::neuron_home().join("locks")
}

/// The lock file of the session file at `session_path`:
/// `<NEURON_HOME>/locks/<sha256 of its canonical path>.lock`, the same file however the
/// session is named (an id, `latest`, a relative or absolute path).
pub fn session_lock_path(session_path: &Path) -> PathBuf {
    let canonical = canonical_session_path(session_path);
    let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
    locks_dir().join(format!("{}.lock", hex::encode(digest)))
}

/// `session_path` with links and `.` resolved. A new session's file does not exist yet,
/// so its folder is resolved and its name added: the path it will have once saved.
fn canonical_session_path(session_path: &Path) -> PathBuf {
    if let Ok(path) = fs::canonicalize(session_path) {
        return path;
    }
    let absolute = if session_path.is_absolute() {
        session_path.to_path_buf()
    } else {
        env::current_dir().map_or_else(|_| session_path.to_path_buf(), |cwd| cwd.join(session_path))
    };
    match (absolute.parent(), absolute.file_name()) {
        (Some(parent), Some(name)) => {
            fs::canonicalize(parent).map_or_else(|_| absolute.clone(), |parent| parent.join(name))
        }
        _ => absolute,
    }
}

/// Where releases before 7.0.0-alpha.4 kept the lock: `<name>.jsonl.lock` beside the
/// session file.
fn legacy_session_lock_path(session_path: &Path) -> PathBuf {
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

/// How many ids a new session tries before giving up. Ids differ by process and carry
/// random bits, so even a second try is rare.
const NEW_SESSION_ATTEMPTS: usize = 8;

/// A new session for this run, locked, not saved yet.
pub fn new_open_session() -> Result<OpenSession, Box<dyn std::error::Error>> {
    let store = current_session_store()?;
    crate::session_presence::record_project(store.workspace_root());
    open_new_session_in(&store, new_cli_session)
}

/// A new session in `store` with the first id from `fresh` that no other run has taken:
/// two runs started at once must each get a session of their own, not be refused as
/// sharing one.
fn open_new_session_in(
    store: &runtime::SessionStore,
    mut fresh: impl FnMut() -> Result<Session, Box<dyn std::error::Error>>,
) -> Result<OpenSession, Box<dyn std::error::Error>> {
    for _ in 0..NEW_SESSION_ATTEMPTS {
        let session = fresh()?;
        let handle = store.create_handle(&session.session_id);
        if let Some(lock) = SessionLock::claim_new(&handle.path)? {
            return Ok(OpenSession {
                session: session.with_persistence_path(handle.path.clone()),
                handle: SessionHandle {
                    id: handle.id,
                    path: handle.path,
                },
                lock,
            });
        }
    }
    Err(format!(
        "could not start a new session: {NEW_SESSION_ATTEMPTS} new session ids in a row were \
already taken in {}",
        store.sessions_dir().display()
    )
    .into())
}

/// The saved session `reference` names, opened for this run to continue. It is locked
/// before it is read, so what loads is what the last run saved, and no other run writes
/// it until the lock is dropped. A file outside the sessions folder (`--resume
/// ./README.md`) is read as a session before it is locked, so a file that is not one
/// gets no lock.
pub fn open_session_reference(reference: &str) -> Result<OpenSession, Box<dyn std::error::Error>> {
    open_session_reference_in_store(&current_session_store()?, reference)
}

fn open_session_reference_in_store(
    store: &runtime::SessionStore,
    reference: &str,
) -> Result<OpenSession, Box<dyn std::error::Error>> {
    let resolved = resolve_session_reference_in_store(store, reference)?;
    let path = resolved.path.display().to_string();
    if !resolved.path.starts_with(store.sessions_dir()) {
        load_session_reference_in_store(store, &path)?;
    }
    let lock = SessionLock::acquire(&resolved.path)?;
    let (handle, session) = load_session_reference_in_store(store, &path)?;
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
    use super::{
        locks_dir, open_new_session_in, open_session_reference_in_store, same_session_file,
        session_lock_path, SessionLock,
    };
    use sha2::{Digest, Sha256};
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

        // Under the locks folder, named by the session's canonical path; nothing beside
        // the session file.
        let lock = session_lock_path(&session);
        assert_eq!(lock.parent(), Some(locks_dir().as_path()));
        let canonical = fs::canonicalize(session.parent().expect("temp dir"))
            .expect("session folder")
            .join("session-1.jsonl");
        let name = format!(
            "{}.lock",
            hex::encode(Sha256::digest(canonical.to_string_lossy().as_bytes()))
        );
        assert_eq!(
            lock.file_name().and_then(|name| name.to_str()),
            Some(&*name)
        );
        assert!(lock.is_file());
        assert!(!session.with_file_name("session-1.jsonl.lock").exists());
        // The same lock however the session is named.
        let roundabout = session
            .parent()
            .expect("temp dir")
            .join(".")
            .join("session-1.jsonl");
        assert_eq!(session_lock_path(&roundabout), lock);
        again.remove();
        assert!(!lock.exists());
        let _ = fs::remove_dir_all(session.parent().expect("temp dir"));
    }

    /// A run of an older release locks `<session>.jsonl.lock` beside the file; while it
    /// holds it, the session is in use for this release too.
    #[test]
    fn the_lock_of_an_older_release_is_honoured() {
        let session = temp_session("legacy");
        fs::create_dir_all(session.parent().expect("temp dir")).expect("temp dir");
        let legacy_path = session.with_file_name("session-1.jsonl.lock");
        let legacy = fs::File::create(&legacy_path).expect("legacy lock");
        legacy.try_lock().expect("the older run holds the session");

        let error = SessionLock::acquire(&session).expect_err("the older run holds it");
        assert!(
            error
                .to_string()
                .contains("is in use by another neuron run"),
            "{error}"
        );
        assert!(SessionLock::claim_new(&session).expect("claim").is_none());

        // Once it is free, this release locks both, so the older release is refused.
        drop(legacy);
        let held = SessionLock::acquire(&session).expect("free once the older run let go");
        let older = fs::OpenOptions::new()
            .write(true)
            .open(&legacy_path)
            .expect("legacy lock");
        assert!(matches!(
            older.try_lock(),
            Err(fs::TryLockError::WouldBlock)
        ));
        drop(older);
        held.remove();
        assert!(!legacy_path.exists());
        let _ = fs::remove_dir_all(session.parent().expect("temp dir"));
    }

    /// `--resume README.md`: not a session, refused before any lock file is made.
    #[test]
    fn a_file_that_is_not_a_session_gets_no_lock() {
        let session = temp_session("not-a-session");
        let workspace = session.parent().expect("temp dir").to_path_buf();
        fs::create_dir_all(&workspace).expect("workspace");
        let store = runtime::SessionStore::from_cwd(&workspace).expect("store");
        let readme = workspace.join("README.md");
        fs::write(&readme, "# Notes\n\nNot a session.\n").expect("readme");

        let error = open_session_reference_in_store(&store, &readme.display().to_string())
            .expect_err("not a session");
        assert!(error.to_string().contains("no valid records"), "{error}");
        assert!(!session_lock_path(&readme).exists());
        assert!(!workspace.join("README.md.lock").exists());

        // A session saved outside the sessions folder is read, then locked.
        let exported = workspace.join("exported.jsonl");
        let mut saved = runtime::Session::new()
            .with_workspace_root(workspace.clone())
            .with_persistence_path(exported.clone());
        saved.push_user_text("hello").expect("saved session");
        let opened = open_session_reference_in_store(&store, &exported.display().to_string())
            .expect("a session");
        assert_eq!(opened.session.messages.len(), 1);
        assert!(session_lock_path(&exported).is_file());
        assert!(!workspace.join("exported.jsonl.lock").exists());
        opened.lock.remove();
        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn a_new_session_id_another_run_took_is_skipped() {
        let session = temp_session("claim");
        let workspace = session.parent().expect("temp dir").to_path_buf();
        fs::create_dir_all(&workspace).expect("workspace");
        let store = runtime::SessionStore::from_cwd(&workspace).expect("store");

        // Another run started in the same millisecond with the same id and holds it.
        let taken = store.create_handle("session-taken");
        let held = SessionLock::claim_new(&taken.path)
            .expect("claim")
            .expect("the first run gets the id");
        assert!(SessionLock::claim_new(&taken.path)
            .expect("claim")
            .is_none());
        // An id whose session file was saved is taken too.
        let saved = store.create_handle("session-saved");
        fs::create_dir_all(saved.path.parent().expect("sessions dir")).expect("sessions dir");
        fs::write(&saved.path, "").expect("session file");

        let mut ids = ["session-taken", "session-saved", "session-free"].into_iter();
        let opened = open_new_session_in(&store, || {
            let mut session = runtime::Session::new();
            session.session_id = ids.next().expect("an id").to_string();
            Ok(session)
        })
        .expect("a session of its own");
        assert_eq!(opened.handle.id, "session-free");
        assert_eq!(opened.session.session_id, "session-free");
        assert_eq!(opened.handle.path, store.create_handle("session-free").path);

        // Every id taken: an error, not a shared session.
        let error = open_new_session_in(&store, || {
            let mut session = runtime::Session::new();
            session.session_id = "session-taken".to_string();
            Ok(session)
        })
        .expect_err("no free id");
        assert!(
            error
                .to_string()
                .starts_with("could not start a new session"),
            "{error}"
        );
        drop(held);
        drop(opened);
        let _ = fs::remove_dir_all(&workspace);
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
