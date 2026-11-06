//! The chats the app has open, and the engines running them. A chat is a
//! folder (the project, or a worktree of it) plus a session; its engine comes
//! and goes: at most four run at once, the one least recently used is stopped
//! to make room, and one that has sat idle for a while is stopped too. A
//! stopped chat keeps its session, so it starts again where it left off.
//!
//! The registry has no Tauri types: engines come from a [`Spawner`] and events
//! go to a [`ChatEvents`], so tests drive it with fakes. Chat ids are minted
//! here and are only ever looked up here; the window cannot make one up.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::engine::{
    resolve_binary, status_session, EngineExit, EngineHandle, EngineProcess, Env, Launch,
};
use crate::worktree::{self, WorktreeRecord};

pub type ChatId = String;

/// Engines running at once.
pub const MAX_RUNNING: usize = 4;
/// Chats the registry holds, running or stopped.
pub const MAX_CHATS: usize = 32;
/// An engine with nothing going on for this long is stopped.
pub const IDLE_AFTER: Duration = Duration::from_secs(20 * 60);
/// The largest client message the window may send.
pub const MAX_LINE: usize = 4 * 1024 * 1024;
/// How long `halt` waits for an engine to save and exit before ending it.
const HALT_WAIT: Duration = if cfg!(test) {
    Duration::from_millis(200)
} else {
    Duration::from_secs(4)
};
/// How long the app's shutdown waits for engines to save before ending them.
const SHUTDOWN_WAIT: Duration = if cfg!(test) {
    Duration::from_millis(200)
} else {
    Duration::from_secs(3)
};

/// Why a chat's engine ended, as the window is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExitReason {
    Exited,
    Idle,
    Evicted,
    Discarded,
}

pub struct SpawnRequest<'a> {
    pub chat: &'a str,
    pub generation: u64,
    pub cwd: &'a Path,
    /// Everything the engine starts with, passed on whole (`--resume` included).
    pub launch: &'a Launch,
    pub trust_once: bool,
}

pub type LineFn = Box<dyn Fn(String) + Send>;
pub type ExitFn = Box<dyn FnOnce(EngineExit) + Send>;

/// Starts engines. The real one runs `neuron engine --stdio`.
pub trait Spawner: Send + Sync {
    fn spawn(
        &self,
        request: &SpawnRequest<'_>,
        on_line: LineFn,
        on_exit: ExitFn,
    ) -> Result<Arc<dyn EngineHandle>, String>;
}

/// Runs the real engine: `neuron engine --stdio` in the chat's folder.
#[derive(Default)]
pub struct ProcessSpawner {
    /// The binary to run; found at each start (`NEURON_BIN`, beside the app, the
    /// PATH) when `None`.
    pub binary: Option<PathBuf>,
    /// The engine's environment; this process's own when `None`.
    pub env: Option<Env>,
}

/// L5: the binary to run, or the coded `[engine:missing]` problem when there is none.
fn binary_or_missing(found: Option<PathBuf>) -> Result<PathBuf, String> {
    found.ok_or_else(|| crate::diagnostics::missing_problem().render())
}

impl Spawner for ProcessSpawner {
    fn spawn(
        &self,
        request: &SpawnRequest<'_>,
        on_line: LineFn,
        on_exit: ExitFn,
    ) -> Result<Arc<dyn EngineHandle>, String> {
        let binary = binary_or_missing(self.binary.clone().or_else(resolve_binary))?;
        let env = self.env.clone().unwrap_or_else(|| {
            // The home folder is trusted for this engine only, and never recorded.
            Env::Inherit(if request.trust_once {
                vec![("NEURON_TRUST_WORKSPACE".to_string(), "1".to_string())]
            } else {
                Vec::new()
            })
        });
        let process =
            EngineProcess::spawn(&binary, request.cwd, request.launch, &env, on_line, on_exit)?;
        Ok(Arc::new(process))
    }
}

/// Where a chat's lines and exits go: the window.
pub trait ChatEvents: Send + Sync {
    fn line(&self, chat: &str, generation: u64, line: String);
    fn exit(&self, chat: &str, generation: u64, exit: EngineExit, reason: ExitReason);
}

struct Chat {
    project_root: PathBuf,
    cwd: PathBuf,
    branch: Option<String>,
    worktree: Option<WorktreeRecord>,
    session: Option<String>,
    generation: u64,
    handle: Option<Arc<dyn EngineHandle>>,
    busy: bool,
    waiting: bool,
    last_activity: Instant,
}

impl Chat {
    fn running(&self) -> bool {
        self.handle.as_ref().is_some_and(|handle| handle.running())
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProjectView {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WorktreeView {
    pub path: String,
    pub branch: String,
    pub base: String,
}

/// What the window is told about a chat.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChatSummary {
    pub chat: ChatId,
    pub generation: u64,
    pub project: ProjectView,
    pub cwd: String,
    pub branch: Option<String>,
    pub worktree: Option<WorktreeView>,
    pub running: bool,
    pub session: Option<String>,
}

pub struct OpenRequest {
    pub path: PathBuf,
    pub resume: Option<String>,
    pub worktree: Option<WorktreeRecord>,
}

pub struct EngineRegistry {
    chats: Mutex<HashMap<ChatId, Chat>>,
    active: Mutex<Option<ChatId>>,
    spawner: Arc<dyn Spawner>,
    events: Arc<dyn ChatEvents>,
    cap: usize,
    max_chats: usize,
    idle_after: Duration,
    next_generation: AtomicU64,
    next_chat: AtomicU64,
}

/// A chat id the registry could have minted: `c` and 4 to 24 of `[0-9a-z]`.
pub fn valid_chat_id(id: &str) -> bool {
    let rest = id.strip_prefix('c').unwrap_or("");
    (4..=24).contains(&rest.len())
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
}

/// A session id for `--resume`: `[A-Za-z0-9._-]{1,128}`, never a flag.
pub fn valid_session(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn display_path(path: &Path) -> String {
    worktree::plain(path).to_string_lossy().into_owned()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl EngineRegistry {
    pub fn new(spawner: Arc<dyn Spawner>, events: Arc<dyn ChatEvents>) -> Arc<Self> {
        Self::with_limits(spawner, events, MAX_RUNNING, MAX_CHATS, IDLE_AFTER)
    }

    pub fn with_limits(
        spawner: Arc<dyn Spawner>,
        events: Arc<dyn ChatEvents>,
        cap: usize,
        max_chats: usize,
        idle_after: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            chats: Mutex::new(HashMap::new()),
            active: Mutex::new(None),
            spawner,
            events,
            cap,
            max_chats,
            idle_after,
            next_generation: AtomicU64::new(0),
            next_chat: AtomicU64::new(0),
        })
    }

    /// Makes a chat in `request.path` (a folder that exists) and starts its engine.
    pub fn open(
        self: &Arc<Self>,
        request: OpenRequest,
        launch: &Launch,
    ) -> Result<ChatSummary, String> {
        let session = match request.resume.as_deref().filter(|id| !id.is_empty()) {
            Some(id) if valid_session(id) => Some(id.to_string()),
            Some(_) => return Err("That session id is not valid.".into()),
            None => None,
        };
        let project_root = std::fs::canonicalize(&request.path)
            .ok()
            .filter(|path| path.is_dir())
            .ok_or_else(|| format!("{} is not a folder.", request.path.display()))?;
        let cwd = request
            .worktree
            .as_ref()
            .map_or_else(|| project_root.clone(), WorktreeRecord::cwd);
        let branch = request
            .worktree
            .as_ref()
            .map(|record| record.branch.clone())
            .or_else(|| worktree::current_branch(&cwd));

        let id = self.mint();
        {
            let mut chats = lock(&self.chats);
            if chats.len() >= self.max_chats {
                return Err("too_many_chats".into());
            }
            chats.insert(
                id.clone(),
                Chat {
                    project_root,
                    cwd,
                    branch,
                    worktree: request.worktree,
                    session,
                    generation: 0,
                    handle: None,
                    busy: false,
                    waiting: false,
                    last_activity: Instant::now(),
                },
            );
        }
        if let Err(error) = self.start(&id, launch, None, false, false) {
            lock(&self.chats).remove(&id);
            return Err(error);
        }
        self.summary(&id)
    }

    /// Starts the chat's engine if none runs (continuing its session). At the
    /// cap, the least recently active chat that is neither working, waiting for
    /// the user, nor the one in view is stopped; with none to stop, `cap_reached`.
    pub fn ensure_running(
        self: &Arc<Self>,
        id: &str,
        launch: &Launch,
    ) -> Result<ChatSummary, String> {
        self.start(id, launch, None, false, false)?;
        self.summary(id)
    }

    /// A new engine for the chat, replacing the one that runs. `resume`: `None`
    /// continues the chat's own session, `Some("")` starts a fresh one,
    /// `Some(id)` continues that one.
    pub fn restart(
        self: &Arc<Self>,
        id: &str,
        launch: &Launch,
        resume: Option<&str>,
        trust_once: bool,
    ) -> Result<ChatSummary, String> {
        self.start(id, launch, resume, trust_once, true)?;
        self.summary(id)
    }

    fn start(
        self: &Arc<Self>,
        id: &str,
        launch: &Launch,
        resume: Option<&str>,
        trust_once: bool,
        replace: bool,
    ) -> Result<u64, String> {
        if let Some(resume) = resume.filter(|resume| !resume.is_empty()) {
            if !valid_session(resume) {
                return Err("That session id is not valid.".into());
            }
        }
        let mut evicted = None;
        let started = self.start_locked(id, launch, resume, trust_once, replace, &mut evicted);
        if let Some((victim, generation)) = evicted {
            self.events.exit(
                &victim,
                generation,
                EngineExit {
                    code: None,
                    error: None,
                },
                ExitReason::Evicted,
            );
        }
        started
    }

    fn start_locked(
        self: &Arc<Self>,
        id: &str,
        launch: &Launch,
        resume: Option<&str>,
        trust_once: bool,
        replace: bool,
        evicted: &mut Option<(ChatId, u64)>,
    ) -> Result<u64, String> {
        let mut chats = lock(&self.chats);
        if !chats.contains_key(id) {
            return Err("No such chat.".into());
        }
        if chats[id].running() && !replace {
            return Ok(chats[id].generation);
        }
        // The engine this one replaces; its exit is not news to the window.
        if let Some(old) = chats.get_mut(id).and_then(|chat| chat.handle.take()) {
            old.stop();
        }
        let active = lock(&self.active).clone();
        let running = chats.values().filter(|chat| chat.running()).count();
        if running >= self.cap {
            let victim = chats
                .iter()
                .filter(|(other, chat)| {
                    other.as_str() != id
                        && chat.running()
                        && !chat.busy
                        && !chat.waiting
                        && active.as_deref() != Some(other.as_str())
                })
                .min_by_key(|(_, chat)| chat.last_activity)
                .map(|(other, _)| other.clone());
            let Some(victim) = victim else {
                return Err("cap_reached".into());
            };
            let chat = chats.get_mut(&victim).expect("the victim is listed");
            if let Some(handle) = chat.handle.take() {
                handle.stop();
            }
            *evicted = Some((victim, chat.generation));
        }

        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let chat = chats.get_mut(id).expect("checked above");
        if let Some(resume) = resume {
            chat.session = Some(resume.to_string()).filter(|resume| !resume.is_empty());
        }
        let launch = Launch {
            resume: chat.session.clone(),
            ..launch.clone()
        };
        let request = SpawnRequest {
            chat: id,
            generation,
            cwd: &chat.cwd,
            launch: &launch,
            trust_once,
        };
        let lines = Arc::clone(self);
        let exits = Arc::clone(self);
        let (line_chat, exit_chat) = (id.to_string(), id.to_string());
        let handle = self.spawner.spawn(
            &request,
            Box::new(move |line| lines.on_line(&line_chat, generation, line)),
            Box::new(move |exit| exits.on_exit(&exit_chat, generation, exit)),
        )?;
        chat.generation = generation;
        chat.handle = Some(handle);
        chat.busy = false;
        chat.waiting = false;
        chat.last_activity = Instant::now();
        Ok(generation)
    }

    fn on_line(&self, id: &str, generation: u64, line: String) {
        {
            let mut chats = lock(&self.chats);
            let Some(chat) = chats
                .get_mut(id)
                .filter(|chat| chat.generation == generation)
            else {
                return;
            };
            chat.last_activity = Instant::now();
            if let Some(session) = status_session(&line) {
                chat.session = Some(session);
            }
        }
        self.events.line(id, generation, line);
    }

    fn on_exit(&self, id: &str, generation: u64, exit: EngineExit) {
        {
            let mut chats = lock(&self.chats);
            let Some(chat) = chats.get_mut(id) else {
                return;
            };
            // An engine the registry already replaced or stopped said why then.
            if chat.generation != generation || chat.handle.is_none() {
                return;
            }
            chat.handle = None;
            chat.busy = false;
            chat.waiting = false;
        }
        self.events.exit(id, generation, exit, ExitReason::Exited);
    }

    /// Writes one client message to the chat's engine.
    pub fn send(&self, id: &str, line: &str) -> Result<(), String> {
        if line.len() > MAX_LINE {
            return Err("That message is too large to send.".into());
        }
        if line.contains(['\n', '\r']) {
            return Err("a client message is one line".into());
        }
        let handle = {
            let mut chats = lock(&self.chats);
            let chat = chats.get_mut(id).ok_or("No such chat.")?;
            chat.last_activity = Instant::now();
            chat.handle.clone()
        };
        handle
            .ok_or("The Neuron engine is not running.")?
            .send(line)
    }

    /// Asks the chat's engine to save and stop. The chat stays; its exit comes
    /// as the engine ends.
    pub fn stop(&self, id: &str) -> Result<(), String> {
        let handle = lock(&self.chats)
            .get(id)
            .ok_or("No such chat.")?
            .handle
            .clone();
        if let Some(handle) = handle {
            handle.stop();
        }
        Ok(())
    }

    /// Forgets the chat, stopping its engine.
    pub fn close(&self, id: &str) -> Result<(), String> {
        let chat = lock(&self.chats).remove(id).ok_or("No such chat.")?;
        if let Some(handle) = chat.handle {
            handle.stop();
        }
        let mut active = lock(&self.active);
        if active.as_deref() == Some(id) {
            *active = None;
        }
        Ok(())
    }

    /// Stops the chat's engine and waits for it to be gone (ending it after a
    /// few seconds), so its folder can be removed.
    pub fn halt(&self, id: &str, reason: ExitReason) -> Result<(), String> {
        let (handle, generation) = {
            let mut chats = lock(&self.chats);
            let chat = chats.get_mut(id).ok_or("No such chat.")?;
            chat.busy = false;
            chat.waiting = false;
            (chat.handle.take(), chat.generation)
        };
        let Some(handle) = handle else {
            return Ok(());
        };
        handle.stop();
        let deadline = Instant::now() + HALT_WAIT;
        while handle.running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        handle.kill_tree();
        self.events.exit(
            id,
            generation,
            EngineExit {
                code: None,
                error: None,
            },
            reason,
        );
        Ok(())
    }

    /// The chat in view; it is never stopped for being idle or to make room.
    pub fn activate(&self, id: Option<&str>) -> Result<(), String> {
        if let Some(id) = id {
            if !lock(&self.chats).contains_key(id) {
                return Err("No such chat.".into());
            }
        }
        *lock(&self.active) = id.map(str::to_string);
        Ok(())
    }

    /// What the window's own state says: working, or waiting for an answer.
    pub fn activity(&self, id: &str, busy: bool, waiting: bool) -> Result<(), String> {
        let mut chats = lock(&self.chats);
        let chat = chats.get_mut(id).ok_or("No such chat.")?;
        chat.busy = busy;
        chat.waiting = waiting;
        chat.last_activity = Instant::now();
        Ok(())
    }

    /// Stops engines that have done nothing for `idle_after`: not working, not
    /// waiting, not in view. Returns the chats stopped.
    pub fn reap(&self, now: Instant) -> Vec<ChatId> {
        let active = lock(&self.active).clone();
        let mut reaped = Vec::new();
        {
            let mut chats = lock(&self.chats);
            for (id, chat) in chats.iter_mut() {
                if chat.running()
                    && !chat.busy
                    && !chat.waiting
                    && active.as_deref() != Some(id.as_str())
                    && now.saturating_duration_since(chat.last_activity) > self.idle_after
                {
                    if let Some(handle) = chat.handle.take() {
                        handle.stop();
                    }
                    reaped.push((id.clone(), chat.generation));
                }
            }
        }
        for (id, generation) in &reaped {
            self.events.exit(
                id,
                *generation,
                EngineExit {
                    code: None,
                    error: None,
                },
                ExitReason::Idle,
            );
        }
        reaped.into_iter().map(|(id, _)| id).collect()
    }

    /// The app is closing: every engine saves and stops, and whatever is left
    /// after a short wait is ended with its children.
    pub fn shutdown_all(&self) {
        let handles: Vec<_> = lock(&self.chats)
            .values_mut()
            .filter_map(|chat| chat.handle.take())
            .collect();
        for handle in &handles {
            handle.stop();
        }
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while handles.iter().any(|handle| handle.running()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        for handle in &handles {
            handle.kill_tree();
        }
    }

    pub fn summary(&self, id: &str) -> Result<ChatSummary, String> {
        let chats = lock(&self.chats);
        let chat = chats.get(id).ok_or("No such chat.")?;
        Ok(Self::summarize(id, chat))
    }

    pub fn list(&self) -> Vec<ChatSummary> {
        let chats = lock(&self.chats);
        let mut all: Vec<_> = chats.iter().collect();
        all.sort_by_key(|(_, chat)| chat.generation);
        all.into_iter()
            .map(|(id, chat)| Self::summarize(id, chat))
            .collect()
    }

    fn summarize(id: &str, chat: &Chat) -> ChatSummary {
        ChatSummary {
            chat: id.to_string(),
            generation: chat.generation,
            project: ProjectView {
                name: chat.project_root.file_name().map_or_else(
                    || display_path(&chat.project_root),
                    |name| name.to_string_lossy().into_owned(),
                ),
                path: display_path(&chat.project_root),
            },
            cwd: display_path(&chat.cwd),
            branch: chat.branch.clone(),
            worktree: chat.worktree.as_ref().map(|record| WorktreeView {
                path: display_path(&record.path),
                branch: record.branch.clone(),
                base: record.base.clone(),
            }),
            running: chat.running(),
            session: chat.session.clone(),
        }
    }

    /// The folder the chat works in.
    pub fn cwd(&self, id: &str) -> Result<PathBuf, String> {
        lock(&self.chats)
            .get(id)
            .map(|chat| chat.cwd.clone())
            .ok_or_else(|| "No such chat.".into())
    }

    /// The chat's worktree record; an error when the chat has none.
    pub fn worktree(&self, id: &str) -> Result<WorktreeRecord, String> {
        lock(&self.chats)
            .get(id)
            .ok_or("No such chat.")?
            .worktree
            .clone()
            .ok_or_else(|| "This chat has no copy of its own.".into())
    }

    /// The chats whose engine runs now.
    pub fn running_count(&self) -> usize {
        lock(&self.chats)
            .values()
            .filter(|chat| chat.running())
            .count()
    }

    fn mint(&self) -> ChatId {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        let counter = self.next_chat.fetch_add(1, Ordering::SeqCst) + 1;
        format!("c{counter}{:06x}", nanos & 0x00ff_ffff)
    }

    #[cfg(test)]
    fn set_last_activity(&self, id: &str, at: Instant) {
        lock(&self.chats).get_mut(id).unwrap().last_activity = at;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Env;
    use std::sync::atomic::AtomicBool;

    struct FakeEngine {
        sent: Mutex<Vec<String>>,
        stops: AtomicU64,
        kills: AtomicU64,
        alive: AtomicBool,
        /// A stop ends the engine at once, like a well-behaved one.
        obeys: bool,
    }

    impl EngineHandle for FakeEngine {
        fn send(&self, line: &str) -> Result<(), String> {
            lock(&self.sent).push(line.to_string());
            Ok(())
        }
        fn stop(&self) {
            self.stops.fetch_add(1, Ordering::SeqCst);
            if self.obeys {
                self.alive.store(false, Ordering::SeqCst);
            }
        }
        fn kill_tree(&self) {
            self.kills.fetch_add(1, Ordering::SeqCst);
            self.alive.store(false, Ordering::SeqCst);
        }
        fn running(&self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }
    }

    struct Spawned {
        chat: String,
        generation: u64,
        cwd: PathBuf,
        args: Vec<String>,
        trust_once: bool,
        engine: Arc<FakeEngine>,
        line: Arc<dyn Fn(String) + Send + Sync>,
        exit: Mutex<Option<ExitFn>>,
    }

    #[derive(Default)]
    struct FakeSpawner {
        spawned: Mutex<Vec<Arc<Spawned>>>,
        refuse: Mutex<Option<String>>,
        obeys: bool,
    }

    impl FakeSpawner {
        fn all(&self) -> Vec<Arc<Spawned>> {
            lock(&self.spawned).clone()
        }
        fn last(&self) -> Arc<Spawned> {
            self.all().last().cloned().expect("an engine was started")
        }
    }

    impl Spawner for FakeSpawner {
        fn spawn(
            &self,
            request: &SpawnRequest<'_>,
            on_line: LineFn,
            on_exit: ExitFn,
        ) -> Result<Arc<dyn EngineHandle>, String> {
            if let Some(error) = lock(&self.refuse).clone() {
                return Err(error);
            }
            let engine = Arc::new(FakeEngine {
                sent: Mutex::new(Vec::new()),
                stops: AtomicU64::new(0),
                kills: AtomicU64::new(0),
                alive: AtomicBool::new(true),
                obeys: self.obeys,
            });
            let on_line = Mutex::new(on_line);
            lock(&self.spawned).push(Arc::new(Spawned {
                chat: request.chat.to_string(),
                generation: request.generation,
                cwd: request.cwd.to_path_buf(),
                args: crate::engine::engine_args(request.launch),
                trust_once: request.trust_once,
                engine: Arc::clone(&engine),
                line: Arc::new(move |line| {
                    let call = lock(&on_line);
                    (*call)(line);
                }),
                exit: Mutex::new(Some(on_exit)),
            }));
            Ok(engine)
        }
    }

    #[derive(Default)]
    struct Recorder {
        lines: Mutex<Vec<(String, u64, String)>>,
        exits: Mutex<Vec<(String, u64, ExitReason)>>,
        /// L5: the error each exit carried, as the window gets it.
        errors: Mutex<Vec<Option<String>>>,
    }

    impl ChatEvents for Recorder {
        fn line(&self, chat: &str, generation: u64, line: String) {
            lock(&self.lines).push((chat.to_string(), generation, line));
        }
        fn exit(&self, chat: &str, generation: u64, exit: EngineExit, reason: ExitReason) {
            lock(&self.exits).push((chat.to_string(), generation, reason));
            lock(&self.errors).push(exit.error);
        }
    }

    struct Fixture {
        registry: Arc<EngineRegistry>,
        spawner: Arc<FakeSpawner>,
        events: Arc<Recorder>,
        dir: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            Self::with(name, false, MAX_RUNNING, MAX_CHATS)
        }
        fn with(name: &str, obeys: bool, cap: usize, max_chats: usize) -> Self {
            let dir =
                std::env::temp_dir().join(format!("neuron-reg-{}-{name}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let spawner = Arc::new(FakeSpawner {
                obeys,
                ..FakeSpawner::default()
            });
            let events = Arc::new(Recorder::default());
            let registry = EngineRegistry::with_limits(
                spawner.clone(),
                events.clone(),
                cap,
                max_chats,
                IDLE_AFTER,
            );
            Self {
                registry,
                spawner,
                events,
                dir,
            }
        }
        fn open(&self) -> ChatSummary {
            self.registry
                .open(
                    OpenRequest {
                        path: self.dir.clone(),
                        resume: None,
                        worktree: None,
                    },
                    &Launch::default(),
                )
                .expect("a chat opens")
        }
        fn exits(&self) -> Vec<(String, u64, ExitReason)> {
            lock(&self.events.exits).clone()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_chat_opens_with_a_minted_id_and_a_running_engine() {
        let fixture = Fixture::new("open");
        let chat = fixture.open();
        assert!(valid_chat_id(&chat.chat), "{}", chat.chat);
        assert!(chat.running);
        assert_eq!(chat.generation, 1);
        let spawned = fixture.spawner.last();
        assert_eq!(spawned.chat, chat.chat);
        assert_eq!(spawned.args, ["engine", "--stdio"]);
        assert_eq!(spawned.cwd, std::fs::canonicalize(&fixture.dir).unwrap());
        let second = fixture.open();
        assert_ne!(second.chat, chat.chat);
    }

    #[test]
    fn a_spawn_error_reaches_the_caller_unchanged_and_leaves_no_chat() {
        let fixture = Fixture::new("spawn-error");
        *lock(&fixture.spawner.refuse) = Some("[engine:quarantined] Could not start x".into());
        let error = fixture
            .registry
            .open(
                OpenRequest {
                    path: fixture.dir.clone(),
                    resume: None,
                    worktree: None,
                },
                &Launch::default(),
            )
            .unwrap_err();
        assert_eq!(error, "[engine:quarantined] Could not start x");
        assert!(fixture.registry.list().is_empty());
    }

    #[test]
    fn the_fifth_engine_stops_the_least_recently_active_idle_chat() {
        let fixture = Fixture::new("evict");
        let base = Instant::now();
        let chats: Vec<_> = (0..4).map(|_| fixture.open()).collect();
        // Chat 1 is the oldest, but working; chat 2 is next-oldest, and idle.
        fixture.registry.set_last_activity(&chats[0].chat, base);
        fixture
            .registry
            .set_last_activity(&chats[1].chat, base + Duration::from_secs(1));
        fixture
            .registry
            .set_last_activity(&chats[2].chat, base + Duration::from_secs(2));
        fixture
            .registry
            .set_last_activity(&chats[3].chat, base + Duration::from_secs(3));
        fixture
            .registry
            .activity(&chats[0].chat, true, false)
            .unwrap();
        fixture.registry.set_last_activity(&chats[0].chat, base);
        let fifth = fixture.open();
        assert!(fifth.running);

        let exits = fixture.exits();
        assert_eq!(exits.len(), 1, "{exits:?}");
        assert_eq!(exits[0].0, chats[1].chat);
        assert_eq!(exits[0].2, ExitReason::Evicted);
        assert!(!fixture.registry.summary(&chats[1].chat).unwrap().running);
        assert!(fixture.registry.summary(&chats[0].chat).unwrap().running);
        assert_eq!(fixture.registry.running_count(), 4);
    }

    #[test]
    fn busy_waiting_and_active_chats_are_never_evicted() {
        let fixture = Fixture::new("keep");
        let chats: Vec<_> = (0..4).map(|_| fixture.open()).collect();
        fixture
            .registry
            .activity(&chats[0].chat, true, false)
            .unwrap();
        fixture
            .registry
            .activity(&chats[1].chat, false, true)
            .unwrap();
        fixture.registry.activate(Some(&chats[2].chat)).unwrap();
        // Chat 3 is the only one that can go.
        let fifth = fixture.open();
        assert_eq!(fixture.exits()[0].0, chats[3].chat);

        // Now nothing can: 0 works, 1 waits, 2 is in view, the fifth is new but idle.
        fixture.registry.activity(&fifth.chat, true, false).unwrap();
        let sixth = fixture.open_error();
        assert_eq!(sixth, "cap_reached");
        // The refused chat does not linger.
        assert_eq!(fixture.registry.list().len(), 5);
    }

    impl Fixture {
        fn open_error(&self) -> String {
            self.registry
                .open(
                    OpenRequest {
                        path: self.dir.clone(),
                        resume: None,
                        worktree: None,
                    },
                    &Launch::default(),
                )
                .unwrap_err()
        }
    }

    #[test]
    fn reap_stops_only_engines_past_the_idle_time() {
        let fixture = Fixture::new("reap");
        let (old, fresh, active) = (fixture.open(), fixture.open(), fixture.open());
        let base = Instant::now();
        fixture.registry.set_last_activity(&old.chat, base);
        fixture
            .registry
            .set_last_activity(&fresh.chat, base + Duration::from_secs(15 * 60));
        fixture.registry.set_last_activity(&active.chat, base);
        fixture.registry.activate(Some(&active.chat)).unwrap();

        let reaped = fixture.registry.reap(base + Duration::from_secs(21 * 60));
        assert_eq!(reaped, std::slice::from_ref(&old.chat));
        assert_eq!(fixture.exits(), [(old.chat.clone(), 1, ExitReason::Idle)]);
        assert!(fixture.registry.summary(&fresh.chat).unwrap().running);
        assert!(fixture.registry.summary(&active.chat).unwrap().running);
    }

    #[test]
    fn working_and_waiting_chats_are_not_reaped() {
        let fixture = Fixture::new("reap-busy");
        let (busy, waiting) = (fixture.open(), fixture.open());
        let base = Instant::now();
        fixture.registry.activity(&busy.chat, true, false).unwrap();
        fixture
            .registry
            .activity(&waiting.chat, false, true)
            .unwrap();
        fixture.registry.set_last_activity(&busy.chat, base);
        fixture.registry.set_last_activity(&waiting.chat, base);
        assert!(fixture
            .registry
            .reap(base + Duration::from_secs(3600))
            .is_empty());
    }

    #[test]
    fn a_line_or_a_send_restarts_the_idle_clock() {
        let fixture = Fixture::new("clock");
        let (talking, sending) = (fixture.open(), fixture.open());
        let long_ago = Instant::now() - Duration::from_secs(3600);
        fixture.registry.set_last_activity(&talking.chat, long_ago);
        fixture.registry.set_last_activity(&sending.chat, long_ago);

        let spawned = fixture.spawner.all();
        (spawned[0].line)(r#"{"type":"text_delta","text":"hi"}"#.into());
        fixture
            .registry
            .send(&sending.chat, r#"{"kind":"command"}"#)
            .unwrap();

        assert!(fixture.registry.reap(Instant::now()).is_empty());
    }

    #[test]
    fn a_stopped_chat_starts_again_with_its_session() {
        let fixture = Fixture::with("resume", true, MAX_RUNNING, MAX_CHATS);
        let chat = fixture.open();
        let first = fixture.spawner.last();
        (first.line)(r#"{"type":"status","session_id":"session-9","model":"m"}"#.into());
        fixture.registry.stop(&chat.chat).unwrap();
        (first.exit.lock().unwrap().take().unwrap())(EngineExit {
            code: Some(0),
            error: None,
        });
        assert!(!fixture.registry.summary(&chat.chat).unwrap().running);
        assert_eq!(
            fixture.exits(),
            [(chat.chat.clone(), 1, ExitReason::Exited)]
        );

        let again = fixture
            .registry
            .ensure_running(&chat.chat, &Launch::default())
            .unwrap();
        assert!(again.running);
        assert_eq!(again.generation, 2);
        assert_eq!(
            fixture.spawner.last().args,
            ["engine", "--stdio", "--resume", "session-9"]
        );
        // Already running: nothing more starts.
        fixture
            .registry
            .ensure_running(&chat.chat, &Launch::default())
            .unwrap();
        assert_eq!(fixture.spawner.all().len(), 2);
    }

    #[test]
    fn launch_options_reach_every_engine() {
        let fixture = Fixture::new("launch");
        let launch = Launch {
            model: Some("sonnet".into()),
            permission_mode: Some("read-only".into()),
            resume: None,
            ..Launch::default()
        };
        let chat = fixture
            .registry
            .open(
                OpenRequest {
                    path: fixture.dir.clone(),
                    resume: Some("old-session".into()),
                    worktree: None,
                },
                &launch,
            )
            .unwrap();
        let expected = [
            "engine",
            "--stdio",
            "--model",
            "sonnet",
            "--permission-mode",
            "read-only",
            "--resume",
            "old-session",
        ];
        assert_eq!(fixture.spawner.last().args, expected);
        fixture
            .registry
            .restart(&chat.chat, &launch, None, true)
            .unwrap();
        let restarted = fixture.spawner.last();
        assert_eq!(restarted.args, expected);
        assert!(restarted.trust_once);
        // A fresh start drops the session.
        fixture
            .registry
            .restart(&chat.chat, &launch, Some(""), false)
            .unwrap();
        assert_eq!(
            fixture.spawner.last().args,
            [
                "engine",
                "--stdio",
                "--model",
                "sonnet",
                "--permission-mode",
                "read-only"
            ]
        );
    }

    /// L3 + L4: the reasoning effort the window chose reaches every engine the
    /// registry starts: a new chat, a restart, a fresh session, and a stopped
    /// chat that starts again.
    #[test]
    fn the_reasoning_effort_reaches_every_engine() {
        let fixture = Fixture::with("effort", true, MAX_RUNNING, MAX_CHATS);
        let launch = Launch {
            reasoning_effort: Some("high".into()),
            ..Launch::default()
        };
        let effort = |args: &[String]| {
            args.windows(2)
                .any(|pair| pair[0] == "--reasoning-effort" && pair[1] == "high")
        };
        let chat = fixture
            .registry
            .open(
                OpenRequest {
                    path: fixture.dir.clone(),
                    resume: None,
                    worktree: None,
                },
                &launch,
            )
            .unwrap();
        assert!(effort(&fixture.spawner.last().args));
        fixture
            .registry
            .restart(&chat.chat, &launch, None, false)
            .unwrap();
        assert!(effort(&fixture.spawner.last().args));
        fixture
            .registry
            .restart(&chat.chat, &launch, Some(""), false)
            .unwrap();
        assert!(effort(&fixture.spawner.last().args));

        // Stopped, then started again where it left off: the effort comes along.
        let running = fixture.spawner.last();
        (running.line)(r#"{"type":"status","session_id":"s-effort"}"#.into());
        fixture.registry.stop(&chat.chat).unwrap();
        (running.exit.lock().unwrap().take().unwrap())(EngineExit {
            code: Some(0),
            error: None,
        });
        fixture
            .registry
            .ensure_running(&chat.chat, &launch)
            .unwrap();
        let again = fixture.spawner.last();
        assert!(effort(&again.args), "{:?}", again.args);
        assert!(again.args.ends_with(&[
            "--resume".to_string(),
            "s-effort".to_string(),
            "--reasoning-effort".to_string(),
            "high".to_string()
        ]));
    }

    /// L5: an engine's coded problem (`[engine:<code>] ...`) reaches the window
    /// unchanged: from a restart that cannot spawn, and from an engine that ends.
    #[test]
    fn engine_problems_reach_the_window_unchanged() {
        let fixture = Fixture::new("problems");
        let chat = fixture.open();
        let crashed = "[engine:crashed] The Neuron engine stopped unexpectedly (exit code 3). Last message: boom";
        (fixture.spawner.last().exit.lock().unwrap().take().unwrap())(EngineExit {
            code: Some(3),
            error: Some(crashed.into()),
        });
        assert_eq!(
            fixture.exits(),
            [(chat.chat.clone(), 1, ExitReason::Exited)]
        );
        assert_eq!(
            lock(&fixture.events.errors).clone(),
            [Some(crashed.to_string())]
        );

        *lock(&fixture.spawner.refuse) =
            Some("[engine:denied] The system would not let Neuron run the engine.".into());
        let error = fixture
            .registry
            .restart(&chat.chat, &Launch::default(), None, false)
            .unwrap_err();
        assert_eq!(
            error,
            "[engine:denied] The system would not let Neuron run the engine."
        );
        let error = fixture
            .registry
            .ensure_running(&chat.chat, &Launch::default())
            .unwrap_err();
        assert!(error.starts_with("[engine:denied] "), "{error}");
    }

    /// L5's launch test, through the registry: a binary that is not there is
    /// `[engine:missing]`, and no chat or exit is left behind.
    #[test]
    fn a_failed_launch_reports_why() {
        let fixture = Fixture::new("missing-binary");
        let events = Arc::new(Recorder::default());
        let registry = EngineRegistry::new(
            Arc::new(ProcessSpawner {
                binary: Some(PathBuf::from("definitely-not-a-neuron-binary")),
                env: None,
            }),
            events.clone(),
        );
        let error = registry
            .open(
                OpenRequest {
                    path: fixture.dir.clone(),
                    resume: None,
                    worktree: None,
                },
                &Launch::default(),
            )
            .unwrap_err();
        assert!(error.starts_with("[engine:missing] "), "{error}");
        assert!(registry.list().is_empty());
        assert!(lock(&events.exits).is_empty());
    }

    /// L5: no engine anywhere (no `NEURON_BIN`, none bundled, none on the PATH)
    /// is the coded `[engine:missing]` problem too.
    #[test]
    fn no_engine_anywhere_is_the_missing_problem() {
        let error = binary_or_missing(None).unwrap_err();
        assert_eq!(error, crate::diagnostics::missing_problem().render());
        assert!(error.starts_with("[engine:missing] "));
        assert_eq!(
            binary_or_missing(Some(PathBuf::from("/opt/neuron"))).unwrap(),
            PathBuf::from("/opt/neuron")
        );
    }

    #[test]
    fn a_replaced_engine_does_not_report_its_exit() {
        let fixture = Fixture::new("replaced");
        let chat = fixture.open();
        let old = fixture.spawner.last();
        fixture
            .registry
            .restart(&chat.chat, &Launch::default(), None, false)
            .unwrap();
        assert_eq!(old.engine.stops.load(Ordering::SeqCst), 1);
        (old.exit.lock().unwrap().take().unwrap())(EngineExit {
            code: Some(0),
            error: None,
        });
        (old.line)("late line".into());
        assert!(fixture.exits().is_empty());
        assert!(lock(&fixture.events.lines).is_empty());
        assert!(fixture.registry.summary(&chat.chat).unwrap().running);
        assert_eq!(old.generation, 1);
        assert_eq!(fixture.spawner.last().generation, 2);
    }

    #[test]
    fn lines_reach_the_window_with_their_chat_and_generation() {
        let fixture = Fixture::new("lines");
        let chat = fixture.open();
        (fixture.spawner.last().line)(r#"{"type":"ready"}"#.into());
        assert_eq!(
            lock(&fixture.events.lines).clone(),
            [(chat.chat, 1, r#"{"type":"ready"}"#.to_string())]
        );
    }

    #[test]
    fn malformed_and_unknown_ids_are_refused() {
        let fixture = Fixture::new("ids");
        let registry = &fixture.registry;
        for id in [
            "",
            "c",
            "x1234",
            "C1234",
            "c12",
            "c../etc",
            "c1234 5",
            &format!("c{}", "a".repeat(40)),
        ] {
            assert!(!valid_chat_id(id), "{id:?}");
            assert!(registry.send(id, "{}").is_err());
            assert!(registry.stop(id).is_err());
            assert!(registry.close(id).is_err());
            assert!(registry.activate(Some(id)).is_err());
            assert!(registry.activity(id, true, true).is_err());
            assert!(registry.summary(id).is_err());
            assert!(registry.worktree(id).is_err());
            assert!(registry.cwd(id).is_err());
            assert!(registry.ensure_running(id, &Launch::default()).is_err());
        }
        // Well-formed, but never minted.
        assert!(valid_chat_id("c1abcdef"));
        assert!(registry.send("c1abcdef", "{}").is_err());
        assert_eq!(fixture.spawner.all().len(), 0);
    }

    #[test]
    fn resume_ids_and_paths_are_checked() {
        let fixture = Fixture::new("inputs");
        for resume in ["-rf", "--model", "a b", "a/b", "", &"a".repeat(129)] {
            let result = fixture.registry.open(
                OpenRequest {
                    path: fixture.dir.clone(),
                    resume: Some(resume.to_string()),
                    worktree: None,
                },
                &Launch::default(),
            );
            // An empty one means "no session", the rest are refused.
            assert_eq!(result.is_ok(), resume.is_empty(), "{resume:?}");
        }
        assert!(valid_session("session-1700000000-0.a_b"));
        assert!(!valid_session("-x"));

        let file = fixture.dir.join("a-file.txt");
        std::fs::write(&file, "x").unwrap();
        for path in [file, fixture.dir.join("missing"), PathBuf::new()] {
            let error = fixture
                .registry
                .open(
                    OpenRequest {
                        path,
                        resume: None,
                        worktree: None,
                    },
                    &Launch::default(),
                )
                .unwrap_err();
            assert!(error.contains("is not a folder"), "{error}");
        }
        assert_eq!(fixture.registry.list().len(), 1);
    }

    #[test]
    fn the_thirty_third_chat_is_refused() {
        let fixture = Fixture::with("many", true, 100, 32);
        for _ in 0..32 {
            fixture.open();
        }
        assert_eq!(fixture.open_error(), "too_many_chats");
        assert_eq!(fixture.registry.list().len(), 32);
    }

    #[test]
    fn messages_are_one_line_and_capped() {
        let fixture = Fixture::new("send");
        let chat = fixture.open();
        let registry = &fixture.registry;
        assert!(registry.send(&chat.chat, "{\"a\":1}\n{\"b\":2}").is_err());
        assert!(registry
            .send(&chat.chat, &"x".repeat(MAX_LINE + 1))
            .is_err());
        registry.send(&chat.chat, "{\"a\":1}").unwrap();
        assert_eq!(*lock(&fixture.spawner.last().engine.sent), ["{\"a\":1}"]);
        // A stopped chat has no engine to send to.
        fixture
            .registry
            .halt(&chat.chat, ExitReason::Discarded)
            .unwrap();
        assert!(registry.send(&chat.chat, "{}").is_err());
    }

    #[test]
    fn closing_forgets_the_chat_and_stops_its_engine() {
        let fixture = Fixture::new("close");
        let chat = fixture.open();
        fixture.registry.activate(Some(&chat.chat)).unwrap();
        fixture.registry.close(&chat.chat).unwrap();
        assert_eq!(
            fixture.spawner.last().engine.stops.load(Ordering::SeqCst),
            1
        );
        assert!(fixture.registry.list().is_empty());
        assert!(fixture.registry.send(&chat.chat, "{}").is_err());
    }

    #[test]
    fn halting_waits_for_the_engine_and_ends_what_is_left() {
        let fixture = Fixture::with("halt", false, MAX_RUNNING, MAX_CHATS);
        let chat = fixture.open();
        // This engine ignores stop, so halt has to end it itself.
        let started = Instant::now();
        fixture
            .registry
            .halt(&chat.chat, ExitReason::Discarded)
            .unwrap();
        let engine = fixture.spawner.last().engine.clone();
        assert!(!engine.running());
        assert_eq!(engine.kills.load(Ordering::SeqCst), 1);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(fixture.exits(), [(chat.chat, 1, ExitReason::Discarded)]);
    }

    #[test]
    fn shutting_down_ends_every_engine_and_its_children() {
        let fixture = Fixture::with("shutdown", false, MAX_RUNNING, MAX_CHATS);
        fixture.open();
        fixture.open();
        fixture.registry.shutdown_all();
        for spawned in fixture.spawner.all() {
            assert_eq!(spawned.engine.stops.load(Ordering::SeqCst), 1);
            assert_eq!(spawned.engine.kills.load(Ordering::SeqCst), 1);
            assert!(!spawned.engine.running());
        }
    }

    #[test]
    fn the_summary_names_the_project_and_the_session() {
        let fixture = Fixture::new("summary");
        let chat = fixture.open();
        (fixture.spawner.last().line)(r#"{"type":"status","session_id":"s-2"}"#.into());
        let listed = fixture.registry.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].session.as_deref(), Some("s-2"));
        assert_eq!(listed[0].chat, chat.chat);
        assert!(listed[0].worktree.is_none());
        assert_eq!(
            listed[0].project.name,
            fixture.dir.file_name().unwrap().to_string_lossy()
        );
    }

    /// Sends lines and exits to channels, for the real-engine test.
    struct Channels {
        lines: Mutex<std::sync::mpsc::Sender<String>>,
        exits: Mutex<std::sync::mpsc::Sender<(EngineExit, ExitReason)>>,
    }

    impl ChatEvents for Channels {
        fn line(&self, _chat: &str, _generation: u64, line: String) {
            let _ = lock(&self.lines).send(line);
        }
        fn exit(&self, _chat: &str, _generation: u64, exit: EngineExit, reason: ExitReason) {
            let _ = lock(&self.exits).send((exit, reason));
        }
    }

    fn debug_binary(name: &str) -> PathBuf {
        let exe = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_string()
        };
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../rust/target/debug")
            .join(exe)
    }

    /// The real engine against the mock service, through a one-chat registry: a
    /// turn whose bash call needs approval. Skipped unless `cargo build -p
    /// rusty-claude-cli -p mock-anthropic-service` ran in rust/.
    #[test]
    fn runs_a_turn_with_an_approval_against_the_mock_service() {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};
        use std::sync::mpsc;

        let (neuron, mock) = (
            debug_binary("neuron"),
            debug_binary("mock-anthropic-service"),
        );
        if !neuron.is_file() || !mock.is_file() {
            eprintln!("skipped: build the engine and the mock service in rust/ first");
            return;
        }
        let mut service = Command::new(&mock)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("mock");
        let mut first = String::new();
        let mut out = BufReader::new(service.stdout.take().unwrap());
        while !first.contains("MOCK_ANTHROPIC_BASE_URL=") {
            first.clear();
            assert!(out.read_line(&mut first).unwrap() > 0, "the mock exited");
        }
        let base_url = first.trim().split_once('=').unwrap().1.to_string();

        let home =
            std::env::temp_dir().join(format!("neuron-desktop-bridge-{}", std::process::id()));
        let workspace = home.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(home.join("config")).unwrap();
        // npx would download the bundled MCP servers into this fresh home.
        std::fs::write(
            home.join("config/settings.json"),
            r#"{"defaultMcpServers":false}"#,
        )
        .unwrap();
        let home_str = home.to_string_lossy().to_string();
        let mut vars: Vec<(String, String)> = [
            "PATH",
            "SystemRoot",
            "SystemDrive",
            "TEMP",
            "TMP",
            "PATHEXT",
            "ComSpec",
        ]
        .iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| (name.to_string(), value))
        })
        .collect();
        vars.extend([
            ("HOME".into(), home_str.clone()),
            ("USERPROFILE".into(), home_str.clone()),
            ("NEURON_CONFIG_HOME".into(), format!("{home_str}/config")),
            ("NEURON_HOME".into(), format!("{home_str}/neuron")),
            ("NEURON_CREDENTIAL_STORE".into(), "file".into()),
            ("NEURON_NO_BROWSER".into(), "1".into()),
            ("ANTHROPIC_API_KEY".into(), "mock-service-key".into()),
            ("ANTHROPIC_BASE_URL".into(), base_url),
        ]);

        let (lines_tx, lines) = mpsc::channel::<String>();
        let (exit_tx, exit) = mpsc::channel();
        let registry = EngineRegistry::new(
            Arc::new(ProcessSpawner {
                binary: Some(neuron),
                env: Some(Env::Only(vars)),
            }),
            Arc::new(Channels {
                lines: Mutex::new(lines_tx),
                exits: Mutex::new(exit_tx),
            }),
        );
        let launch = Launch {
            model: Some("sonnet".into()),
            permission_mode: Some("workspace-write".into()),
            resume: None,
            ..Launch::default()
        };
        let chat = registry
            .open(
                OpenRequest {
                    path: workspace.clone(),
                    resume: None,
                    worktree: None,
                },
                &launch,
            )
            .expect("open");

        let next = || {
            let line = lines
                .recv_timeout(Duration::from_secs(60))
                .expect("an event");
            serde_json::from_str::<serde_json::Value>(&line).expect("a JSON line")
        };
        assert_eq!(next()["type"], "ready");
        registry
            .send(
                &chat.chat,
                r#"{"kind":"command","type":"submit","text":"PARITY_SCENARIO:bash_permission_prompt_approved","attachments":[]}"#,
            )
            .unwrap();
        let mut seen = Vec::new();
        loop {
            let event = next();
            let kind = event["type"].as_str().unwrap().to_string();
            if kind == "permission_requested" {
                let reply = format!(
                    r#"{{"kind":"permission_reply","id":{},"decision":{{"kind":"allow_once"}}}}"#,
                    event["id"]
                );
                registry.send(&chat.chat, &reply).unwrap();
            }
            if kind == "tool_result" {
                assert_eq!(event["is_error"], false);
            }
            seen.push(kind.clone());
            if kind == "turn_finished" {
                break;
            }
        }
        assert!(
            seen.contains(&"permission_requested".to_string()),
            "{seen:?}"
        );
        assert!(seen.contains(&"tool_result".to_string()), "{seen:?}");
        assert_eq!(
            std::fs::read_to_string(workspace.join("approved.txt")).unwrap(),
            "approved via prompt"
        );

        registry.stop(&chat.chat).unwrap();
        let (end, reason) = exit
            .recv_timeout(Duration::from_secs(30))
            .expect("the engine exits");
        assert_eq!(reason, ExitReason::Exited);
        assert_eq!(
            end,
            EngineExit {
                code: Some(0),
                error: None
            }
        );
        let _ = service.kill();
        let _ = service.wait();
        let _ = std::fs::remove_dir_all(&home);
    }
}
