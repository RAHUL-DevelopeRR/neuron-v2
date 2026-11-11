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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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
/// How long a same-chat restart (or a start that needs the room) waits for a
/// stopping engine to exit before ending it: the engine's own shutdown grace.
const STOP_WAIT: Duration = if cfg!(test) {
    Duration::from_millis(200)
} else {
    Duration::from_secs(5)
};
/// How long the app's shutdown waits for engines to save before ending them.
const SHUTDOWN_WAIT: Duration = if cfg!(test) {
    Duration::from_millis(200)
} else {
    Duration::from_secs(3)
};
/// How long the app's shutdown then waits for starts under way to give up. The
/// wait ends as soon as they have, so tests keep it as long. A spawn slower
/// than this is not waited for: the engine's input ends when the app exits and
/// the engine exits by itself, and on Windows the Job Object ends it with the
/// app.
const STARTS_WAIT: Duration = Duration::from_secs(3);

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
    /// An engine is being started: the slot and the generation are reserved,
    /// but the handle is not there yet.
    starting: bool,
    /// The engine ended while it was still starting; reported once it is stored.
    pending_exit: Option<EngineExit>,
    /// The copy is being removed: nothing may start or send meanwhile.
    discarding: bool,
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
    /// False until the copy has a name of its own (the first prompt gives one).
    pub named: bool,
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
    /// Engines told to stop that have not exited yet. They still count toward
    /// the cap. Lock order: `chats`, then this.
    lingering: Mutex<Vec<Arc<dyn EngineHandle>>>,
    active: Mutex<Option<ChatId>>,
    spawner: Arc<dyn Spawner>,
    events: Arc<dyn ChatEvents>,
    cap: usize,
    max_chats: usize,
    idle_after: Duration,
    next_generation: AtomicU64,
    next_chat: AtomicU64,
    /// The app is closing: no engine starts any more. Set under the `chats` lock.
    closing: AtomicBool,
    /// Starts from `reserve` to their end, whether their chat is still there
    /// or not; the app's shutdown waits for them. Counted up under the `chats`
    /// lock.
    starts: AtomicUsize,
}

/// One start under way, counted until it ends by whatever path.
struct StartUnderWay<'a>(&'a AtomicUsize);

impl Drop for StartUnderWay<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
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

/// What the window is told when an engine was ended by the registry.
fn ended() -> EngineExit {
    EngineExit {
        code: None,
        error: None,
    }
}

/// Waits until none of `handles` runs, or `limit` passes; then ends what is left.
fn wait_for_exit(handles: &[Arc<dyn EngineHandle>], limit: Duration) {
    let deadline = Instant::now() + limit;
    while handles.iter().any(|handle| handle.running()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    for handle in handles.iter().filter(|handle| handle.running()) {
        handle.kill_tree();
    }
}

/// What `reserve` decided.
enum Reserved {
    /// An engine is running (or starting); nothing to do.
    Already(u64),
    Go(Box<Plan>),
}

struct Plan {
    generation: u64,
    previous_generation: u64,
    cwd: PathBuf,
    launch: Launch,
    /// Engines to ask to stop.
    stop: Vec<Arc<dyn EngineHandle>>,
    /// Engines to wait for before the new one starts.
    wait: Vec<Arc<dyn EngineHandle>>,
    evicted: Option<(ChatId, u64)>,
    /// The chat's own engine was taken out to be replaced. Its exit is not
    /// told when a new engine takes its place; when none does, it is.
    replaced: bool,
}

/// Why nothing starts once the app is closing.
const CLOSING: &str = "Neuron is closing.";

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
            lingering: Mutex::new(Vec::new()),
            active: Mutex::new(None),
            spawner,
            events,
            cap,
            max_chats,
            idle_after,
            next_generation: AtomicU64::new(0),
            next_chat: AtomicU64::new(0),
            closing: AtomicBool::new(false),
            starts: AtomicUsize::new(0),
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
                    starting: false,
                    pending_exit: None,
                    discarding: false,
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
    /// `Some(id)` continues that one. When the new engine cannot start, the old
    /// one is gone all the same: the window gets its `exited` exit, carrying
    /// the reason nothing replaced it.
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
        // Under the lock: only bookkeeping. The slot and the generation are
        // reserved, and the engines to stop are taken out.
        let plan = match self.reserve(id, launch, resume, replace)? {
            Reserved::Already(generation) => return Ok(generation),
            Reserved::Go(plan) => plan,
        };
        // `reserve` counted this start; it stops counting once the engine is
        // stored or ended.
        let _under_way = StartUnderWay(&self.starts);
        // Without the lock: stop, wait, spawn.
        for handle in &plan.stop {
            handle.stop();
        }
        wait_for_exit(&plan.wait, STOP_WAIT);
        if let Some((victim, generation)) = &plan.evicted {
            self.events
                .exit(victim, *generation, ended(), ExitReason::Evicted);
        }
        // The app began closing meanwhile: nothing more starts.
        if self.closing.load(Ordering::SeqCst) {
            return Err(self.abandon(id, &plan, CLOSING.into()));
        }
        let request = SpawnRequest {
            chat: id,
            generation: plan.generation,
            cwd: &plan.cwd,
            launch: &plan.launch,
            trust_once,
        };
        let generation = plan.generation;
        let lines = Arc::clone(self);
        let exits = Arc::clone(self);
        let (line_chat, exit_chat) = (id.to_string(), id.to_string());
        let spawned = self.spawner.spawn(
            &request,
            Box::new(move |line| lines.on_line(&line_chat, generation, line)),
            Box::new(move |exit| exits.on_exit(&exit_chat, generation, exit)),
        );
        let handle = match spawned {
            Ok(handle) => handle,
            Err(error) => return Err(self.abandon(id, &plan, error)),
        };
        let mut chats = lock(&self.chats);
        if self.closing.load(Ordering::SeqCst) {
            // Ended while the chat still counts as starting: the app's shutdown
            // waits for that, so this engine is gone before the app is.
            drop(chats);
            handle.kill_tree();
            return Err(self.abandon(id, &plan, CLOSING.into()));
        }
        let Some(chat) = chats.get_mut(id) else {
            // Closed while it was starting.
            drop(chats);
            handle.kill_tree();
            return Err("No such chat.".into());
        };
        chat.handle = Some(handle);
        chat.starting = false;
        chat.last_activity = Instant::now();
        let early_exit = chat.pending_exit.take();
        if early_exit.is_some() {
            chat.handle = None;
        }
        drop(chats);
        if let Some(exit) = early_exit {
            self.events.exit(id, generation, exit, ExitReason::Exited);
        }
        Ok(generation)
    }

    /// A start that will not finish: the chat is as it was, without an engine.
    /// If the start had taken the chat's own engine out to replace it, the
    /// window is told that engine is gone, with `error` as the reason (not
    /// while the app is closing: there is no window to tell). Returns `error`.
    fn abandon(&self, id: &str, plan: &Plan, error: String) -> String {
        let replaced = {
            let mut chats = lock(&self.chats);
            if let Some(chat) = chats.get_mut(id) {
                chat.starting = false;
                chat.pending_exit = None;
                chat.generation = plan.previous_generation;
                plan.replaced
            } else {
                false
            }
        };
        if replaced && !self.closing.load(Ordering::SeqCst) {
            self.events.exit(
                id,
                plan.previous_generation,
                EngineExit {
                    code: None,
                    error: Some(error.clone()),
                },
                ExitReason::Exited,
            );
        }
        error
    }

    /// The part of a start that holds the lock: no stopping, no spawning.
    fn reserve(
        &self,
        id: &str,
        launch: &Launch,
        resume: Option<&str>,
        replace: bool,
    ) -> Result<Reserved, String> {
        let mut chats = lock(&self.chats);
        if self.closing.load(Ordering::SeqCst) {
            return Err(CLOSING.into());
        }
        let chat = chats.get(id).ok_or("No such chat.")?;
        if chat.discarding {
            return Err("discarding".into());
        }
        if chat.starting {
            return if replace {
                Err("starting".into())
            } else {
                Ok(Reserved::Already(chat.generation))
            };
        }
        if chat.running() && !replace {
            return Ok(Reserved::Already(chat.generation));
        }
        let active = lock(&self.active).clone();
        // The engines other chats hold (this chat's own is being replaced).
        let others = chats
            .iter()
            .filter(|(other, chat)| other.as_str() != id && (chat.running() || chat.starting))
            .count();
        let mut victim = None;
        if others >= self.cap {
            victim = chats
                .iter()
                .filter(|(other, chat)| {
                    other.as_str() != id
                        && chat.running()
                        && !chat.busy
                        && !chat.waiting
                        && !chat.discarding
                        && active.as_deref() != Some(other.as_str())
                })
                .min_by_key(|(_, chat)| chat.last_activity)
                .map(|(other, _)| other.clone());
            if victim.is_none() {
                return Err("cap_reached".into());
            }
        }

        let mut stop = Vec::new();
        let mut evicted = None;
        let mut lingering = lock(&self.lingering);
        lingering.retain(|handle| handle.running());
        // The engine this one replaces; its exit is not news to the window
        // unless nothing takes its place (`abandon`). One that already ended
        // on its own is not waited for, but its exit was not told either.
        let taken = chats.get_mut(id).and_then(|chat| chat.handle.take());
        let replaced = taken.is_some();
        let old = taken.filter(|handle| handle.running());
        if let Some(old) = &old {
            lingering.push(Arc::clone(old));
            stop.push(Arc::clone(old));
        }
        let mut others_after = others;
        if let Some(victim) = victim {
            let chat = chats.get_mut(&victim).expect("the victim is listed");
            if let Some(handle) = chat.handle.take() {
                lingering.push(Arc::clone(&handle));
                stop.push(handle);
            }
            evicted = Some((victim, chat.generation));
            others_after -= 1;
        }
        // Over the cap while stopping engines still run: wait for them.
        let wait = if others_after + 1 + lingering.len() > self.cap {
            lingering.clone()
        } else {
            old.into_iter().collect()
        };
        drop(lingering);

        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let chat = chats.get_mut(id).expect("checked above");
        if let Some(resume) = resume {
            chat.session = Some(resume.to_string()).filter(|resume| !resume.is_empty());
        }
        let previous_generation = chat.generation;
        chat.generation = generation;
        chat.starting = true;
        // Under the lock with the `closing` check above: a shutdown either
        // stopped this start or waits for it.
        self.starts.fetch_add(1, Ordering::SeqCst);
        chat.pending_exit = None;
        chat.busy = false;
        chat.waiting = false;
        chat.last_activity = Instant::now();
        Ok(Reserved::Go(Box::new(Plan {
            generation,
            previous_generation,
            cwd: chat.cwd.clone(),
            launch: Launch {
                resume: chat.session.clone(),
                ..launch.clone()
            },
            stop,
            wait,
            evicted,
            replaced,
        })))
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
            // A session id is kept only if it could be passed to `--resume`.
            if let Some(session) = status_session(&line).filter(|session| valid_session(session)) {
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
            if chat.generation != generation {
                return;
            }
            // Ended before the registry stored it: reported when it is.
            if chat.starting {
                chat.pending_exit = Some(exit);
                return;
            }
            // An engine the registry already replaced or stopped said why then.
            if chat.handle.is_none() {
                return;
            }
            chat.handle = None;
            chat.busy = false;
            chat.waiting = false;
        }
        self.events.exit(id, generation, exit, ExitReason::Exited);
    }

    /// Writes one client message to the chat's engine. It only queues the
    /// message (the engine's writer thread does the writing), so it never waits.
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
            if chat.discarding {
                return Err("discarding".into());
            }
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

    /// Forgets the chat, stopping its engine. A chat with a copy of its own
    /// (a worktree) is not closed: `has_worktree`, and nothing changes. The
    /// discard flow removes the copy and the chat together.
    pub fn close(&self, id: &str) -> Result<(), String> {
        let handle = {
            let mut chats = lock(&self.chats);
            let chat = chats.get(id).ok_or("No such chat.")?;
            if chat.worktree.is_some() {
                return Err("has_worktree".into());
            }
            let chat = chats.remove(id).expect("checked above");
            if let Some(handle) = &chat.handle {
                lock(&self.lingering).push(Arc::clone(handle));
            }
            chat.handle
        };
        if let Some(handle) = handle {
            handle.stop();
        }
        let mut active = lock(&self.active);
        if active.as_deref() == Some(id) {
            *active = None;
        }
        Ok(())
    }

    /// Takes the chat's engine, stops it and waits for it to be gone (ending it
    /// after a few seconds), so its folder can be removed. Says nothing to the
    /// window; returns the generation of the engine that was there.
    fn halt_engine(&self, id: &str) -> Result<Option<u64>, String> {
        let (handle, generation) = {
            let mut chats = lock(&self.chats);
            let chat = chats.get_mut(id).ok_or("No such chat.")?;
            chat.busy = false;
            chat.waiting = false;
            let handle = chat.handle.take();
            if let Some(handle) = &handle {
                lock(&self.lingering).push(Arc::clone(handle));
            }
            (handle, chat.generation)
        };
        let Some(handle) = handle else {
            return Ok(None);
        };
        handle.stop();
        wait_for_exit(std::slice::from_ref(&handle), HALT_WAIT);
        Ok(Some(generation))
    }

    /// Stops the chat's engine and waits for it to be gone, then tells the
    /// window why it ended.
    pub fn halt(&self, id: &str, reason: ExitReason) -> Result<(), String> {
        if let Some(generation) = self.halt_engine(id)? {
            self.events.exit(id, generation, ended(), reason);
        }
        Ok(())
    }

    /// Removes a chat's copy. While it runs, the chat refuses to start or take
    /// messages. `check` runs first (nothing is stopped if it refuses), then the
    /// engine is halted, `before_remove` runs (close terminals), and `remove`
    /// removes the copy. Only when the removal worked is the chat forgotten and
    /// the `discarded` exit sent; if it failed the chat is usable again (its
    /// engine, already stopped, is reported as exited).
    pub fn discard_worktree(
        &self,
        id: &str,
        check: impl FnOnce(&WorktreeRecord) -> Result<(), String>,
        before_remove: impl FnOnce(&WorktreeRecord),
        remove: impl FnOnce(&WorktreeRecord) -> Result<(), String>,
    ) -> Result<(), String> {
        let record = {
            let mut chats = lock(&self.chats);
            let chat = chats.get_mut(id).ok_or("No such chat.")?;
            let record = chat
                .worktree
                .clone()
                .ok_or("This chat has no copy of its own.")?;
            if chat.discarding {
                return Err("discarding".into());
            }
            if chat.starting {
                return Err("starting".into());
            }
            chat.discarding = true;
            record
        };
        let mut halted = None;
        let result = check(&record).and_then(|()| {
            halted = self.halt_engine(id)?;
            before_remove(&record);
            remove(&record)
        });
        match result {
            Ok(()) => {
                lock(&self.chats).remove(id);
                let mut active = lock(&self.active);
                if active.as_deref() == Some(id) {
                    *active = None;
                }
                drop(active);
                if let Some(generation) = halted {
                    self.events
                        .exit(id, generation, ended(), ExitReason::Discarded);
                }
                Ok(())
            }
            Err(error) => {
                if let Some(chat) = lock(&self.chats).get_mut(id) {
                    chat.discarding = false;
                }
                if let Some(generation) = halted {
                    self.events
                        .exit(id, generation, ended(), ExitReason::Exited);
                }
                Err(error)
            }
        }
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
        let mut handles = Vec::new();
        {
            let mut chats = lock(&self.chats);
            for (id, chat) in chats.iter_mut() {
                if chat.running()
                    && !chat.busy
                    && !chat.waiting
                    && !chat.discarding
                    && active.as_deref() != Some(id.as_str())
                    && now.saturating_duration_since(chat.last_activity) > self.idle_after
                {
                    if let Some(handle) = chat.handle.take() {
                        lock(&self.lingering).push(Arc::clone(&handle));
                        handles.push(handle);
                    }
                    reaped.push((id.clone(), chat.generation));
                }
            }
        }
        // Stopped with the lock released.
        for handle in &handles {
            handle.stop();
        }
        for (id, generation) in &reaped {
            self.events.exit(id, *generation, ended(), ExitReason::Idle);
        }
        reaped.into_iter().map(|(id, _)| id).collect()
    }

    /// The app is closing: no engine starts any more, every engine saves and
    /// stops (one already told to stop, too), and whatever is left after a
    /// short wait is ended with its children. A chat still starting is waited
    /// for, up to `STARTS_WAIT`: its start gives up and ends any engine it
    /// brought up. A start slower than that is left to the engine itself, which
    /// exits when its input ends with the app.
    pub fn shutdown_all(&self) {
        let handles: Vec<_> = {
            let mut chats = lock(&self.chats);
            // Under the lock: a start reserved after this sees it, and one
            // reserved before counts as starting below.
            self.closing.store(true, Ordering::SeqCst);
            let mut handles: Vec<_> = chats
                .values_mut()
                .filter_map(|chat| chat.handle.take())
                .collect();
            // Told to stop, not exited yet: a closed chat's engine, one stopped
            // to make room or for being idle, one being replaced.
            handles.append(&mut lock(&self.lingering));
            handles
        };
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
        // A start that waited for one of those engines goes on now and gives
        // up; one that was spawning ends what it spawned. A start counts even
        // when its chat was closed meanwhile.
        let deadline = Instant::now() + STARTS_WAIT;
        while self.starts.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
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
                named: record.named,
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
    /// How many chats are working, and how many wait for the user: what a
    /// quit asks about first.
    pub fn working(&self) -> (usize, usize) {
        let chats = lock(&self.chats);
        let waiting = chats.values().filter(|chat| chat.waiting).count();
        let working = chats
            .values()
            .filter(|chat| chat.busy && !chat.waiting)
            .count();
        (working, waiting)
    }

    /// Names the chat's copy with `rename` (git runs with the lock released).
    /// Nothing changes when the copy has a name already; an error when it is
    /// being discarded or was replaced meanwhile.
    pub fn rename_worktree(
        &self,
        id: &str,
        rename: impl FnOnce(&WorktreeRecord) -> Result<WorktreeRecord, String>,
    ) -> Result<ChatSummary, String> {
        let record = {
            let chats = lock(&self.chats);
            let chat = chats.get(id).ok_or("No such chat.")?;
            if chat.discarding {
                return Err("discarding".into());
            }
            chat.worktree
                .clone()
                .ok_or("This chat has no copy of its own.")?
        };
        if record.named {
            return self.summary(id);
        }
        let renamed = rename(&record)?;
        let mut chats = lock(&self.chats);
        let chat = chats.get_mut(id).ok_or("No such chat.")?;
        if chat.discarding || chat.worktree.as_ref() != Some(&record) {
            return Err("This chat's copy changed meanwhile.".into());
        }
        chat.branch = Some(renamed.branch.clone());
        chat.worktree = Some(renamed);
        Ok(Self::summarize(id, chat))
    }

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
        /// `send` waits while this is set, like a pipe nobody reads.
        stalled: AtomicBool,
    }

    impl EngineHandle for FakeEngine {
        fn send(&self, line: &str) -> Result<(), String> {
            while self.stalled.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
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
        /// Engines started before this one that still ran when it started.
        alive_at_spawn: usize,
        engine: Arc<FakeEngine>,
        line: Arc<dyn Fn(String) + Send + Sync>,
        exit: Mutex<Option<ExitFn>>,
    }

    #[derive(Default)]
    struct FakeSpawner {
        spawned: Mutex<Vec<Arc<Spawned>>>,
        refuse: Mutex<Option<String>>,
        obeys: bool,
        /// The engine ends before `spawn` has returned.
        exits_while_starting: AtomicBool,
        /// `spawn` waits while this is set, like an engine slow to come up.
        held: AtomicBool,
        /// Starts that reached `spawn` (held there or not).
        entered: AtomicU64,
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
            self.entered.fetch_add(1, Ordering::SeqCst);
            while self.held.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            if let Some(error) = lock(&self.refuse).clone() {
                return Err(error);
            }
            let engine = Arc::new(FakeEngine {
                sent: Mutex::new(Vec::new()),
                stops: AtomicU64::new(0),
                kills: AtomicU64::new(0),
                alive: AtomicBool::new(true),
                obeys: self.obeys,
                stalled: AtomicBool::new(false),
            });
            let alive_at_spawn = lock(&self.spawned)
                .iter()
                .filter(|spawned| spawned.engine.running())
                .count();
            if self.exits_while_starting.load(Ordering::SeqCst) {
                engine.alive.store(false, Ordering::SeqCst);
                on_exit(EngineExit {
                    code: Some(1),
                    error: None,
                });
                let spawned_engine: Arc<dyn EngineHandle> = engine.clone();
                return Ok(spawned_engine);
            }
            let on_line = Mutex::new(on_line);
            lock(&self.spawned).push(Arc::new(Spawned {
                chat: request.chat.to_string(),
                generation: request.generation,
                cwd: request.cwd.to_path_buf(),
                args: crate::engine::engine_args(request.launch),
                trust_once: request.trust_once,
                alive_at_spawn,
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

    /// A restart whose new engine cannot start still tells the window that the
    /// old engine is gone, with the reason nothing took its place. The old
    /// engine's own exit, when it comes, is not told twice.
    #[test]
    fn a_failed_restart_tells_the_window_the_old_engine_is_gone() {
        let fixture = Fixture::with("restart-fails", true, MAX_RUNNING, MAX_CHATS);
        let chat = fixture.open();
        let old = fixture.spawner.last();
        let denied = "[engine:denied] The system would not let Neuron run the engine.";
        *lock(&fixture.spawner.refuse) = Some(denied.into());
        let error = fixture
            .registry
            .restart(&chat.chat, &Launch::default(), None, false)
            .unwrap_err();
        assert_eq!(error, denied);
        assert_eq!(old.engine.stops.load(Ordering::SeqCst), 1);
        assert_eq!(
            fixture.exits(),
            [(chat.chat.clone(), 1, ExitReason::Exited)]
        );
        assert_eq!(
            lock(&fixture.events.errors).clone(),
            [Some(denied.to_string())]
        );
        let summary = fixture.registry.summary(&chat.chat).unwrap();
        assert!(!summary.running);
        assert_eq!(summary.generation, 1);
        (old.exit.lock().unwrap().take().unwrap())(EngineExit {
            code: Some(0),
            error: None,
        });
        assert_eq!(fixture.exits().len(), 1);

        // An engine that had just ended on its own, before its exit was told,
        // is told the same way.
        *lock(&fixture.spawner.refuse) = None;
        let other = fixture.open();
        let ended = fixture.spawner.last();
        ended.engine.alive.store(false, Ordering::SeqCst);
        *lock(&fixture.spawner.refuse) = Some(denied.into());
        fixture
            .registry
            .restart(&other.chat, &Launch::default(), None, false)
            .unwrap_err();
        (ended.exit.lock().unwrap().take().unwrap())(EngineExit {
            code: Some(1),
            error: None,
        });
        assert_eq!(
            fixture.exits()[1..],
            [(other.chat.clone(), other.generation, ExitReason::Exited)]
        );

        // Once an engine can start, the chat starts again.
        *lock(&fixture.spawner.refuse) = None;
        assert!(
            fixture
                .registry
                .restart(&chat.chat, &Launch::default(), None, false)
                .unwrap()
                .running
        );
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

    /// Quitting while a chat's engine is starting: the start is waited for, and
    /// the engine it brings up is ended before `shutdown_all` returns.
    #[test]
    fn quitting_while_a_chat_starts_ends_that_engine_too() {
        quit_during_a_start("quit-starting", false);
    }

    /// The same for a chat closed while its engine starts: it is no longer
    /// listed, but its start is still waited for.
    #[test]
    fn quitting_waits_for_the_start_of_a_chat_closed_meanwhile() {
        quit_during_a_start("quit-closed-starting", true);
    }

    /// Restarts a chat whose new engine is held inside `spawn` (closing the chat
    /// meanwhile when `close`), quits, and lets the engine come up once the app
    /// is closing. When `shutdown_all` returns, that engine must be gone.
    fn quit_during_a_start(name: &str, close: bool) {
        let fixture = Fixture::with(name, true, MAX_RUNNING, MAX_CHATS);
        let chat = fixture.open();
        fixture.spawner.held.store(true, Ordering::SeqCst);
        let registry = Arc::clone(&fixture.registry);
        let id = chat.chat.clone();
        let restart =
            std::thread::spawn(move || registry.restart(&id, &Launch::default(), None, false));
        let deadline = Instant::now() + Duration::from_secs(5);
        while fixture.spawner.entered.load(Ordering::SeqCst) < 2 {
            assert!(Instant::now() < deadline, "the restart never reached spawn");
            std::thread::sleep(Duration::from_millis(5));
        }
        if close {
            fixture.registry.close(&chat.chat).unwrap();
        }

        let (registry, spawner) = (Arc::clone(&fixture.registry), Arc::clone(&fixture.spawner));
        let quit = std::thread::spawn(move || {
            registry.shutdown_all();
            // What is there the moment the app would exit.
            let engines = spawner.all();
            let running = engines
                .iter()
                .filter(|spawned| spawned.engine.running())
                .count();
            (engines.len(), running)
        });
        // The engine comes up once the app has begun closing.
        let deadline = Instant::now() + Duration::from_secs(1);
        while !fixture.registry.closing.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        fixture.spawner.held.store(false, Ordering::SeqCst);

        let (started, running) = quit.join().unwrap();
        assert_eq!(
            started, 2,
            "shutdown_all returned before the start was over"
        );
        assert_eq!(running, 0);
        assert!(restart.join().unwrap().is_err());
        assert_eq!(
            fixture.spawner.last().engine.kills.load(Ordering::SeqCst),
            1
        );
    }

    /// Quitting also ends engines told to stop that have not exited yet (here a
    /// closed chat's), and nothing starts once the app is closing.
    #[test]
    fn quitting_ends_engines_still_stopping_and_starts_nothing_after() {
        // Never obeys: a stopped engine lingers until it is ended.
        let fixture = Fixture::with("quit-lingering", false, MAX_RUNNING, MAX_CHATS);
        let closed = fixture.open();
        let kept = fixture.open();
        fixture.registry.close(&closed.chat).unwrap();
        let lingering = fixture.spawner.all()[0].engine.clone();
        assert!(lingering.running(), "told to stop, not gone yet");

        fixture.registry.shutdown_all();
        for spawned in fixture.spawner.all() {
            assert!(
                !spawned.engine.running(),
                "engine {} still runs",
                spawned.generation
            );
        }
        assert_eq!(lingering.kills.load(Ordering::SeqCst), 1);

        assert_eq!(fixture.open_error(), "Neuron is closing.");
        assert!(fixture
            .registry
            .ensure_running(&kept.chat, &Launch::default())
            .is_err());
        assert_eq!(fixture.spawner.all().len(), 2);
    }

    /// Quitting while a start waits for room (the engine it stopped has not
    /// exited yet) leaves no engine running, whichever finishes first.
    #[test]
    fn quitting_while_a_start_waits_for_room_leaves_nothing_running() {
        // Never obeys, and room for one: the new chat waits for the one it stops.
        let fixture = Fixture::with("quit-waiting", false, 1, MAX_CHATS);
        fixture.open();
        let evicted = fixture.spawner.last().engine.clone();
        let (registry, dir) = (Arc::clone(&fixture.registry), fixture.dir.clone());
        let opening = std::thread::spawn(move || {
            registry.open(
                OpenRequest {
                    path: dir,
                    resume: None,
                    worktree: None,
                },
                &Launch::default(),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while evicted.stops.load(Ordering::SeqCst) == 0 {
            assert!(
                Instant::now() < deadline,
                "the first chat was never stopped"
            );
            std::thread::sleep(Duration::from_millis(1));
        }

        fixture.registry.shutdown_all();
        let _ = opening.join().unwrap();
        for spawned in fixture.spawner.all() {
            assert!(
                !spawned.engine.running(),
                "engine {} still runs",
                spawned.generation
            );
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

    fn worktree_record(path: &Path) -> WorktreeRecord {
        WorktreeRecord {
            repo: path.to_path_buf(),
            root: path.to_path_buf(),
            path: path.to_path_buf(),
            subdir: PathBuf::new(),
            slug: "copy".into(),
            branch: "neuron/copy".into(),
            base: "main".into(),
            named: false,
        }
    }

    impl Fixture {
        fn open_copy(&self) -> ChatSummary {
            self.registry
                .open(
                    OpenRequest {
                        path: self.dir.clone(),
                        resume: None,
                        worktree: Some(worktree_record(&self.dir)),
                    },
                    &Launch::default(),
                )
                .expect("a chat opens")
        }
    }

    /// What a quit asks about: chats working, and chats waiting for the user.
    #[test]
    fn working_counts_busy_and_waiting_chats_apart() {
        let fixture = Fixture::new("working");
        let (a, b, c) = (fixture.open(), fixture.open(), fixture.open());
        assert_eq!(fixture.registry.working(), (0, 0));
        fixture.registry.activity(&a.chat, true, false).unwrap();
        fixture.registry.activity(&b.chat, true, true).unwrap();
        assert_eq!(fixture.registry.working(), (1, 1));
        fixture.registry.activity(&c.chat, true, false).unwrap();
        assert_eq!(fixture.registry.working(), (2, 1));
        fixture.registry.activity(&a.chat, false, false).unwrap();
        fixture.registry.activity(&b.chat, false, false).unwrap();
        assert_eq!(fixture.registry.working(), (1, 0));
    }

    #[test]
    fn renaming_a_copy_updates_its_summary_once() {
        let fixture = Fixture::new("rename-copy");
        let chat = fixture.open_copy();
        assert_eq!(chat.worktree.as_ref().map(|w| w.named), Some(false));
        let calls = std::cell::Cell::new(0);
        let renamed = fixture
            .registry
            .rename_worktree(&chat.chat, |record| {
                calls.set(calls.get() + 1);
                Ok(WorktreeRecord {
                    slug: "fix-login".into(),
                    branch: "neuron/fix-login".into(),
                    named: true,
                    ..record.clone()
                })
            })
            .unwrap();
        assert_eq!(renamed.branch.as_deref(), Some("neuron/fix-login"));
        let view = renamed.worktree.unwrap();
        assert_eq!(
            (view.branch.as_str(), view.named),
            ("neuron/fix-login", true)
        );
        assert_eq!(
            fixture.registry.worktree(&chat.chat).unwrap().slug,
            "fix-login"
        );
        // Named now: a later prompt does not run git again.
        fixture
            .registry
            .rename_worktree(&chat.chat, |_| {
                calls.set(calls.get() + 1);
                Err("ran".into())
            })
            .unwrap();
        assert_eq!(calls.get(), 1);
        // A chat in the project's own folder has no copy to name.
        let plain = fixture.open();
        assert!(fixture
            .registry
            .rename_worktree(&plain.chat, |record| Ok(record.clone()))
            .is_err());
    }

    /// An engine that stops reading its input stalls its own `send`, and nothing
    /// else: another chat still sends, stops and closes.
    #[test]
    fn a_stalled_engine_does_not_block_another_chat() {
        let fixture = Fixture::new("stalled");
        let (slow, other) = (fixture.open(), fixture.open());
        let slow_engine = fixture.spawner.all()[0].engine.clone();
        slow_engine.stalled.store(true, Ordering::SeqCst);

        let registry = Arc::clone(&fixture.registry);
        let slow_id = slow.chat.clone();
        let stuck = std::thread::spawn(move || registry.send(&slow_id, r#"{"kind":"command"}"#));
        std::thread::sleep(Duration::from_millis(100));

        let (done_tx, done) = std::sync::mpsc::channel();
        let registry = Arc::clone(&fixture.registry);
        let other_id = other.chat.clone();
        let slow_id = slow.chat.clone();
        std::thread::spawn(move || {
            let sent = registry.send(&other_id, r#"{"kind":"command"}"#);
            let summary = registry.summary(&slow_id);
            let stopped = registry.stop(&other_id);
            let closed = registry.close(&other_id);
            let _ = done_tx.send((sent, summary.is_ok(), stopped, closed));
        });
        let (sent, summary, stopped, closed) = done
            .recv_timeout(Duration::from_secs(3))
            .expect("another chat must not wait for the stalled one");
        assert!(sent.is_ok() && summary && stopped.is_ok() && closed.is_ok());
        assert!(!stuck.is_finished(), "the stalled send is still waiting");
        slow_engine.stalled.store(false, Ordering::SeqCst);
        stuck.join().unwrap().unwrap();
    }

    #[test]
    fn a_session_id_from_a_status_line_must_be_a_valid_one() {
        let fixture = Fixture::new("sniff");
        let chat = fixture.open();
        let engine = fixture.spawner.last();
        (engine.line)(r#"{"type":"status","session_id":"good-1.a_b"}"#.into());
        for bad in ["--model", "a b", "x/../y", "-rf"] {
            (engine.line)(format!(r#"{{"type":"status","session_id":"{bad}"}}"#));
            assert_eq!(
                fixture
                    .registry
                    .summary(&chat.chat)
                    .unwrap()
                    .session
                    .as_deref(),
                Some("good-1.a_b"),
                "{bad}"
            );
        }
        let long = "a".repeat(129);
        (engine.line)(format!(r#"{{"type":"status","session_id":"{long}"}}"#));
        assert_eq!(
            fixture
                .registry
                .summary(&chat.chat)
                .unwrap()
                .session
                .as_deref(),
            Some("good-1.a_b")
        );
        // The line itself still reaches the window.
        assert_eq!(lock(&fixture.events.lines).len(), 6);
    }

    #[test]
    fn a_restart_waits_for_the_old_engine_to_exit() {
        // This engine ignores stop: the restart waits, then ends it, then spawns.
        let fixture = Fixture::with("restart-wait", false, MAX_RUNNING, MAX_CHATS);
        let chat = fixture.open();
        let old = fixture.spawner.last();
        (old.line)(r#"{"type":"status","session_id":"s-keep"}"#.into());
        let started = Instant::now();
        fixture
            .registry
            .restart(&chat.chat, &Launch::default(), None, false)
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(150));
        assert_eq!(old.engine.stops.load(Ordering::SeqCst), 1);
        assert_eq!(old.engine.kills.load(Ordering::SeqCst), 1);
        let new = fixture.spawner.last();
        assert_eq!(new.alive_at_spawn, 0, "the old engine was gone first");
        assert_eq!(new.args, ["engine", "--stdio", "--resume", "s-keep"]);

        // One that obeys is not waited for any longer than it takes to exit.
        let fixture = Fixture::with("restart-fast", true, MAX_RUNNING, MAX_CHATS);
        let chat = fixture.open();
        let started = Instant::now();
        fixture
            .registry
            .restart(&chat.chat, &Launch::default(), None, false)
            .unwrap();
        assert!(started.elapsed() < Duration::from_millis(150));
        assert_eq!(fixture.spawner.last().alive_at_spawn, 0);
    }

    #[test]
    fn stopping_engines_still_count_toward_the_cap() {
        // Never obeys: the evicted engine lingers until it is ended.
        let fixture = Fixture::with("linger", false, 2, MAX_CHATS);
        let (a, _b) = (fixture.open(), fixture.open());
        fixture
            .registry
            .set_last_activity(&a.chat, Instant::now() - Duration::from_secs(60));
        let started = Instant::now();
        let third = fixture.open();
        assert!(third.running);
        assert!(started.elapsed() >= Duration::from_millis(150));
        let engines = fixture.spawner.all();
        assert_eq!(engines.len(), 3);
        assert!(
            engines[2].alive_at_spawn < 2,
            "the evicted engine was still running when the third started: {}",
            engines[2].alive_at_spawn
        );
        assert_eq!(fixture.exits(), [(a.chat, 1, ExitReason::Evicted)]);
    }

    #[test]
    fn an_engine_that_ends_while_it_starts_is_still_reported() {
        let fixture = Fixture::new("early-exit");
        fixture
            .spawner
            .exits_while_starting
            .store(true, Ordering::SeqCst);
        let chat = fixture.open();
        assert!(!fixture.registry.summary(&chat.chat).unwrap().running);
        assert_eq!(
            fixture.exits(),
            [(chat.chat.clone(), 1, ExitReason::Exited)]
        );
        // It can be started again.
        fixture
            .spawner
            .exits_while_starting
            .store(false, Ordering::SeqCst);
        assert!(
            fixture
                .registry
                .ensure_running(&chat.chat, &Launch::default())
                .unwrap()
                .running
        );
    }

    #[test]
    fn closing_a_chat_with_a_copy_changes_nothing() {
        let fixture = Fixture::new("close-copy");
        let chat = fixture.open_copy();
        assert_eq!(
            fixture.registry.close(&chat.chat).unwrap_err(),
            "has_worktree"
        );
        assert_eq!(fixture.registry.list().len(), 1);
        assert!(fixture.registry.summary(&chat.chat).unwrap().running);
        assert_eq!(
            fixture.spawner.last().engine.stops.load(Ordering::SeqCst),
            0
        );
        assert!(fixture.exits().is_empty());
        fixture.registry.send(&chat.chat, "{}").unwrap();
    }

    #[test]
    fn a_chat_being_discarded_refuses_to_start_or_send_until_the_copy_is_gone() {
        let fixture = Fixture::with("discard", true, MAX_RUNNING, MAX_CHATS);
        let chat = fixture.open_copy();
        let id = chat.chat.clone();
        let registry = Arc::clone(&fixture.registry);
        let exits_seen_during_removal = Mutex::new(None);
        fixture
            .registry
            .discard_worktree(
                &id,
                |_| Ok(()),
                |_| {},
                |_| {
                    assert_eq!(registry.send(&id, "{}").unwrap_err(), "discarding");
                    assert_eq!(
                        registry
                            .restart(&id, &Launch::default(), None, false)
                            .unwrap_err(),
                        "discarding"
                    );
                    assert_eq!(
                        registry
                            .ensure_running(&id, &Launch::default())
                            .unwrap_err(),
                        "discarding"
                    );
                    // The exit is not told before the copy is gone.
                    *lock(&exits_seen_during_removal) = Some(fixture.exits());
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(lock(&exits_seen_during_removal).clone(), Some(vec![]));
        assert_eq!(
            fixture.exits(),
            [(chat.chat.clone(), 1, ExitReason::Discarded)]
        );
        assert!(fixture.registry.list().is_empty());
        assert_eq!(fixture.spawner.all().len(), 1, "nothing was restarted");
    }

    #[test]
    fn a_failed_removal_leaves_the_chat_usable_and_says_no_discarded() {
        let fixture = Fixture::with("discard-fails", true, MAX_RUNNING, MAX_CHATS);
        let chat = fixture.open_copy();
        let error = fixture
            .registry
            .discard_worktree(&chat.chat, |_| Ok(()), |_| {}, |_| Err("locked".into()))
            .unwrap_err();
        assert_eq!(error, "locked");
        assert_eq!(
            fixture.exits(),
            [(chat.chat.clone(), 1, ExitReason::Exited)]
        );
        // The chat is still there, and starts, sends and discards again.
        assert_eq!(fixture.registry.list().len(), 1);
        let again = fixture
            .registry
            .ensure_running(&chat.chat, &Launch::default())
            .unwrap();
        assert!(again.running);
        fixture.registry.send(&chat.chat, "{}").unwrap();
        fixture
            .registry
            .discard_worktree(&chat.chat, |_| Ok(()), |_| {}, |_| Ok(()))
            .unwrap();
        assert!(fixture.registry.list().is_empty());
    }

    #[test]
    fn a_refused_check_stops_nothing() {
        let fixture = Fixture::new("discard-dirty");
        let chat = fixture.open_copy();
        let error = fixture
            .registry
            .discard_worktree(
                &chat.chat,
                |_| Err("dirty:2".into()),
                |_| panic!("nothing is closed"),
                |_| panic!("nothing is removed"),
            )
            .unwrap_err();
        assert_eq!(error, "dirty:2");
        assert_eq!(
            fixture.spawner.last().engine.stops.load(Ordering::SeqCst),
            0
        );
        assert!(fixture.exits().is_empty());
        fixture.registry.send(&chat.chat, "{}").unwrap();
        // A chat with no copy has nothing to discard.
        let plain = fixture.open();
        assert!(fixture
            .registry
            .discard_worktree(&plain.chat, |_| Ok(()), |_| {}, |_| Ok(()))
            .is_err());
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
