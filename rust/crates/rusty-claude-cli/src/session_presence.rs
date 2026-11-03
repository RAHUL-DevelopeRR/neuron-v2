//! Saved chats as the session lists of the terminal UI and the desktop show them
//! ([`SessionSummary`]): which workspace each belongs to, whether another Neuron has it open
//! or runs a turn in it, how its last turn ended, and the git branch it ran on.
//!
//! - Presence: an engine holds an exclusive lock on `<sessions>/presence/<id>.lock` while it
//!   has the session open, and keeps `<id>.json` beside it (whether a turn runs, how the
//!   last one ended, the branch, which UI). The operating system releases the lock when the
//!   engine exits or crashes, so a stale file never shows a session as open.
//! - Projects: `projects.json` in Neuron's home lists the workspaces Neuron has kept chats
//!   in, newest first, for the list of every project's chats.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use neuron_tui::protocol::{SessionRunStatus, SessionScope, SessionSummary};
use serde::{Deserialize, Serialize};

const PRESENCE_DIR: &str = "presence";
const PROJECTS_FILE: &str = "projects.json";
/// How many workspaces `projects.json` remembers.
const MAX_PROJECTS: usize = 100;
/// How many chats a list shows: per workspace, and in all.
const PROJECT_SESSION_LIMIT: usize = 50;
const ALL_SESSION_LIMIT: usize = 200;

/// What an engine says about a session it has open, in `<id>.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct PresenceState {
    /// The UI that runs it, as it named itself.
    #[serde(default)]
    client: Option<String>,
    #[serde(default)]
    running: bool,
    /// The last turn failed.
    #[serde(default)]
    failed: bool,
    #[serde(default)]
    git_branch: Option<String>,
    #[serde(default)]
    updated_at_ms: u64,
}

/// An engine's hold on the session it runs; dropping it lets go.
pub(crate) struct SessionPresence {
    session_id: String,
    dir: PathBuf,
    /// The held lock; `None` when another Neuron holds it (both have the session open).
    _lock: Option<File>,
    state: PresenceState,
}

impl SessionPresence {
    /// Takes `session_id` in `sessions_dir` (the folder its file is in) as open.
    pub(crate) fn claim(sessions_dir: &Path, session_id: &str, client: Option<String>) -> Self {
        let dir = sessions_dir.join(PRESENCE_DIR);
        let previous = read_state(&dir, session_id).unwrap_or_default();
        let lock = fs::create_dir_all(&dir)
            .ok()
            .and_then(|()| hold(&lock_path(&dir, session_id)));
        let mut presence = Self {
            session_id: session_id.to_string(),
            dir,
            _lock: lock,
            state: PresenceState {
                client,
                running: false,
                ..previous
            },
        };
        presence.save();
        presence
    }

    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }

    /// A turn started, run by `client` (when the UI named itself by now).
    pub(crate) fn turn_started(&mut self, client: Option<String>) {
        if client.is_some() {
            self.state.client = client;
        }
        self.state.running = true;
        self.save();
    }

    /// The turn ended: how, and on which branch.
    pub(crate) fn turn_finished(&mut self, failed: bool, git_branch: Option<String>) {
        self.state.running = false;
        self.state.failed = failed;
        if git_branch.is_some() {
            self.state.git_branch = git_branch;
        }
        self.save();
    }

    /// Best effort: a session list misses a mark, nothing else.
    fn save(&mut self) {
        self.state.updated_at_ms = now_ms();
        let Ok(text) = serde_json::to_string(&self.state) else {
            return;
        };
        let path = state_path(&self.dir, &self.session_id);
        let partial = path.with_extension("json.partial");
        if fs::write(&partial, text).is_ok() && fs::rename(&partial, &path).is_err() {
            let _ = fs::remove_file(&partial);
        }
    }
}

impl Drop for SessionPresence {
    fn drop(&mut self) {
        if self.state.running {
            self.state.running = false;
            self.save();
        }
    }
}

fn lock_path(dir: &Path, session_id: &str) -> PathBuf {
    dir.join(format!("{session_id}.lock"))
}

fn state_path(dir: &Path, session_id: &str) -> PathBuf {
    dir.join(format!("{session_id}.json"))
}

fn read_state(dir: &Path, session_id: &str) -> Option<PresenceState> {
    let text = fs::read_to_string(state_path(dir, session_id)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Locks `path` for this process, trying briefly: another Neuron's session list may be
/// looking at it that moment.
fn hold(path: &Path) -> Option<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .ok()?;
    for attempt in 0..3 {
        match file.try_lock() {
            Ok(()) => return Some(file),
            Err(TryLockError::WouldBlock) if attempt < 2 => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
    None
}

/// Whether some Neuron holds the lock at `path`.
fn is_held(path: &Path) -> bool {
    let Ok(file) = OpenOptions::new().write(true).open(path) else {
        return false;
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(TryLockError::WouldBlock) => true,
        Err(TryLockError::Error(_)) => false,
    }
}

/// What other Neurons do with `session_id`, and what its last engine said about it.
fn observe(sessions_dir: &Path, session_id: &str) -> (SessionRunStatus, PresenceState) {
    let dir = sessions_dir.join(PRESENCE_DIR);
    let state = read_state(&dir, session_id).unwrap_or_default();
    let status = match (is_held(&lock_path(&dir, session_id)), state.running) {
        (false, _) => SessionRunStatus::Idle,
        (true, true) => SessionRunStatus::Running,
        (true, false) => SessionRunStatus::Open,
    };
    (status, state)
}

fn now_ms() -> u64 {
    crate::clock::now_unix().saturating_mul(1_000)
}

// ------------------------------------------------------------------- projects

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProjectsFile {
    #[serde(default)]
    projects: Vec<ProjectEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ProjectEntry {
    path: String,
    last_used_ms: u64,
}

fn projects_file() -> PathBuf {
    crate::credentials::neuron_home().join(PROJECTS_FILE)
}

/// Remembers that Neuron keeps chats in `root` (an absolute folder), as the newest.
pub(crate) fn record_project(root: &Path) {
    // Unit tests never write to the developer's own Neuron home.
    if cfg!(test) && std::env::var_os("NEURON_HOME").is_none() {
        return;
    }
    record_project_in(&projects_file(), root, now_ms());
}

fn record_project_in(file: &Path, root: &Path, now_ms: u64) {
    if !root.is_absolute() {
        return;
    }
    let path = root.display().to_string();
    let mut projects = read_projects(file);
    projects.projects.retain(|entry| entry.path != path);
    projects.projects.insert(
        0,
        ProjectEntry {
            path,
            last_used_ms: now_ms,
        },
    );
    projects.projects.truncate(MAX_PROJECTS);
    let Ok(text) = serde_json::to_string_pretty(&projects) else {
        return;
    };
    if let Some(parent) = file.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let partial = file.with_extension("json.partial");
    if fs::write(&partial, text).is_ok() && fs::rename(&partial, file).is_err() {
        let _ = fs::remove_file(&partial);
    }
}

fn read_projects(file: &Path) -> ProjectsFile {
    fs::read_to_string(file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// The workspaces Neuron has kept chats in, newest first.
fn known_projects() -> Vec<PathBuf> {
    read_projects(&projects_file())
        .projects
        .into_iter()
        .map(|entry| PathBuf::from(entry.path))
        .collect()
}

// ---------------------------------------------------------------- session lists

/// The saved chats in `scope`, newest first: this workspace's (`cwd`), or every known
/// workspace's. `current` is the session the engine runs (empty for none).
pub(crate) fn list(scope: SessionScope, current: &str, cwd: &Path) -> Vec<SessionSummary> {
    let projects = match scope {
        SessionScope::Project => Vec::new(),
        SessionScope::All => known_projects(),
    };
    list_with(scope, current, cwd, &projects)
}

fn list_with(
    scope: SessionScope,
    current: &str,
    cwd: &Path,
    projects: &[PathBuf],
) -> Vec<SessionSummary> {
    let mut sessions = Vec::new();
    sessions_of(cwd, current, &mut sessions);
    if scope == SessionScope::Project {
        return sessions;
    }
    let mut seen = vec![same_folder_key(cwd)];
    for root in projects {
        let key = same_folder_key(root);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        sessions_of(root, current, &mut sessions);
    }
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at_ms));
    sessions.truncate(ALL_SESSION_LIMIT);
    sessions
}

/// A folder as compared for "the same project": canonical when it exists.
fn same_folder_key(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The newest chats of the workspace at `root`, read without creating anything there.
fn sessions_of(root: &Path, current: &str, out: &mut Vec<SessionSummary>) {
    let Some(store) = runtime::SessionStore::existing(root) else {
        return;
    };
    let Ok(sessions) = store.list_sessions() else {
        return;
    };
    let project = root.display().to_string();
    for session in sessions.into_iter().take(PROJECT_SESSION_LIMIT) {
        let is_current = !current.is_empty() && session.id == current;
        let dir = session
            .path
            .parent()
            .unwrap_or_else(|| store.sessions_dir())
            .to_path_buf();
        let (run_status, state) = if is_current {
            // This engine's own state is in `status`.
            let state = read_state(&dir.join(PRESENCE_DIR), &session.id).unwrap_or_default();
            (SessionRunStatus::Idle, state)
        } else {
            observe(&dir, &session.id)
        };
        out.push(SessionSummary {
            current: is_current,
            id: session.id,
            title: session.title,
            model: session.model,
            message_count: session.message_count,
            updated_at_ms: session.updated_at_ms,
            branch: session.branch_name,
            project: Some(project.clone()),
            git_branch: state.git_branch,
            run_status,
            failed: state.failed,
            client: state.client,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::{ConversationMessage, Session};

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "neuron-presence-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// A workspace with one saved chat; returns the chat's id.
    fn workspace_with_chat(root: &Path, prompt: &str) -> String {
        let store = runtime::SessionStore::from_cwd(root).expect("a store");
        let mut session = Session::new().with_workspace_root(root.to_path_buf());
        session
            .messages
            .push(ConversationMessage::user_text(prompt));
        let handle = store.create_handle(&session.session_id);
        session.save_to_path(&handle.path).expect("saved");
        handle.id
    }

    #[test]
    fn an_open_session_shows_as_open_then_running_then_idle() {
        let root = temp_dir("states");
        let id = workspace_with_chat(&root, "fix the build");
        let dir = runtime::SessionStore::existing(&root)
            .expect("a store")
            .sessions_dir()
            .to_path_buf();
        assert_eq!(observe(&dir, &id).0, SessionRunStatus::Idle);

        let mut presence = SessionPresence::claim(&dir, &id, Some("terminal".into()));
        assert_eq!(presence.session_id(), id);
        let (status, state) = observe(&dir, &id);
        assert_eq!(status, SessionRunStatus::Open);
        assert_eq!(state.client.as_deref(), Some("terminal"));

        presence.turn_started(None);
        assert_eq!(observe(&dir, &id).0, SessionRunStatus::Running);
        presence.turn_finished(true, Some("main".into()));
        let (status, state) = observe(&dir, &id);
        assert_eq!(status, SessionRunStatus::Open);
        assert!(state.failed);
        assert_eq!(state.git_branch.as_deref(), Some("main"));

        drop(presence);
        let (status, state) = observe(&dir, &id);
        assert_eq!(status, SessionRunStatus::Idle, "the lock went with it");
        assert!(state.failed, "how the last turn ended stays");

        // The next engine to open it starts from what was said, not running.
        let again = SessionPresence::claim(&dir, &id, None);
        assert!(again.state.failed);
        assert!(!again.state.running);
        drop(again);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn projects_are_remembered_newest_first_without_repeats() {
        let home = temp_dir("projects");
        let file = home.join(PROJECTS_FILE);
        let first = home.join("first");
        let second = home.join("second");
        record_project_in(&file, &first, 1);
        record_project_in(&file, &second, 2);
        record_project_in(&file, &first, 3);
        record_project_in(&file, Path::new("relative"), 4);
        let paths: Vec<String> = read_projects(&file)
            .projects
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        assert_eq!(
            paths,
            vec![first.display().to_string(), second.display().to_string()]
        );
        fs::remove_dir_all(home).expect("cleanup");
    }

    #[test]
    fn lists_this_workspace_or_every_known_one() {
        let base = temp_dir("lists");
        let here = base.join("here");
        let there = base.join("there");
        let empty = base.join("empty");
        for dir in [&here, &there, &empty] {
            fs::create_dir_all(dir).expect("workspace");
        }
        let current = workspace_with_chat(&here, "fix the build");
        let other = workspace_with_chat(&there, "write the docs");
        let projects = vec![there.clone(), here.clone(), empty.clone()];

        let project = list_with(SessionScope::Project, &current, &here, &projects);
        assert_eq!(project.len(), 1);
        assert!(project[0].current);
        assert_eq!(project[0].title.as_deref(), Some("fix the build"));
        assert_eq!(
            project[0].project.as_deref(),
            Some(here.display().to_string().as_str())
        );

        let all = list_with(SessionScope::All, &current, &here, &projects);
        assert_eq!(all.len(), 2, "each chat once: {all:?}");
        let theirs = all
            .iter()
            .find(|session| session.id == other)
            .expect("the other workspace's chat");
        assert!(!theirs.current);
        assert_eq!(
            theirs.project.as_deref(),
            Some(there.display().to_string().as_str())
        );
        assert_eq!(theirs.run_status, SessionRunStatus::Idle);
        assert!(
            !empty.join(".neuron").exists(),
            "listing creates nothing in other workspaces"
        );
        fs::remove_dir_all(base).expect("cleanup");
    }
}
