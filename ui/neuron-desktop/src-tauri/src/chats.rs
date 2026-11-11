//! The window's chats: commands to open, resume, stop and close them, the
//! events their engines send, copies of the project (worktrees), and the
//! taskbar badge. The registry (`registry.rs`) holds the chats and decides
//! which engines run; this file is the Tauri side of it.
//!
//! Events: `neuron://chat-line {chat, generation, line}` and
//! `neuron://chat-exit {chat, generation, code, error, reason}`, where `reason`
//! is `exited`, `idle`, `evicted` or `discarded`. Engine errors (spawn and exit
//! messages) pass to the window exactly as the engine layer wrote them.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::engine::EngineExit;
use crate::git::{self, BranchChanges};
use crate::pr::{self, PrCache, PullRequestSnapshot};
use crate::projects::{self, Project};
use crate::registry::{
    valid_chat_id, ChatEvents, ChatSummary, EngineRegistry, ExitReason, OpenRequest,
};
use crate::terminal::Terminals;
use crate::workspace::Workspace;
use crate::worktree::{self, WorktreeStatus};
use crate::{lock, AppState, LaunchOptions};

/// How often idle engines are looked for.
const REAP_EVERY: Duration = Duration::from_secs(30);

#[derive(Clone, Serialize)]
struct ChatLine {
    chat: String,
    generation: u64,
    line: String,
}

#[derive(Clone, Serialize)]
struct ChatExit {
    chat: String,
    generation: u64,
    code: Option<i32>,
    error: Option<String>,
    reason: ExitReason,
}

impl ChatExit {
    /// The event for an engine that ended. Its error is the engine layer's own
    /// (L5's `[engine:<code>] ...`), passed on unchanged.
    fn of(chat: &str, generation: u64, exit: EngineExit, reason: ExitReason) -> Self {
        Self {
            chat: chat.to_string(),
            generation,
            code: exit.code,
            error: exit.error,
            reason,
        }
    }
}

/// Sends chat events to the window.
pub struct WindowEvents(pub AppHandle);

impl ChatEvents for WindowEvents {
    fn line(&self, chat: &str, generation: u64, line: String) {
        let _ = self.0.emit(
            "neuron://chat-line",
            ChatLine {
                chat: chat.to_string(),
                generation,
                line,
            },
        );
    }

    fn exit(&self, chat: &str, generation: u64, exit: EngineExit, reason: ExitReason) {
        let _ = self.0.emit(
            "neuron://chat-exit",
            ChatExit::of(chat, generation, exit, reason),
        );
    }
}

/// Starts the idle sweep for the life of the app.
pub fn start_reaper(registry: Arc<EngineRegistry>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(REAP_EVERY);
        registry.reap(Instant::now());
    });
}

type Registry<'a> = State<'a, Arc<EngineRegistry>>;

/// A chat id as the window sent it: well formed, or an error.
fn chat_id(chat: &str) -> Result<&str, String> {
    if valid_chat_id(chat) {
        Ok(chat)
    } else {
        Err("No such chat.".into())
    }
}

/// The folder a command reads: the chat's, or the default workspace without one.
pub fn workspace_for(
    state: &AppState,
    registry: &EngineRegistry,
    chat: Option<&str>,
) -> Result<Workspace, String> {
    match chat {
        None => Ok(lock(&state.workspace).clone()),
        Some(chat) => Workspace::new(registry.cwd(chat_id(chat)?)?),
    }
}

/// Keeps the window's launch options for this and later engines.
fn remember_options(state: &AppState, options: Option<LaunchOptions>) {
    if let Some(options) = options {
        *lock(&state.options) = options.checked();
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct WorktreeRequest {
    pub name: Option<String>,
    /// The branch the copy starts from: an existing local or remote branch.
    /// The repository's default branch (else HEAD) when absent.
    pub start: Option<String>,
}

/// Opens a chat in `path` (the default workspace when absent), on a fresh
/// session or continuing `resume`, optionally in a copy of its own.
#[tauri::command(async)]
pub fn chat_open(
    app: AppHandle,
    state: State<'_, AppState>,
    registry: Registry<'_>,
    path: Option<String>,
    resume: Option<String>,
    worktree: Option<WorktreeRequest>,
    options: Option<LaunchOptions>,
) -> Result<ChatSummary, String> {
    remember_options(&state, options);
    let path = path.map_or_else(
        || lock(&state.workspace).root().to_path_buf(),
        |path| PathBuf::from(path.trim()),
    );
    let record = match worktree {
        Some(request) => {
            let data = app
                .path()
                .app_data_dir()
                .map_err(|error| error.to_string())?;
            Some(worktree::create(
                &data,
                &path,
                request.name.as_deref(),
                request.start.as_deref(),
            )?)
        }
        None => None,
    };
    let launch = lock(&state.options).to_launch();
    let opened = registry.open(
        OpenRequest {
            path,
            resume,
            worktree: record.clone(),
        },
        &launch,
    );
    match opened {
        Ok(summary) => {
            projects::remember(
                &app,
                &Project {
                    name: summary.project.name.clone(),
                    path: summary.project.path.clone(),
                },
            );
            Ok(summary)
        }
        Err(error) => {
            // A copy no chat uses is not left behind.
            if let Some(record) = record {
                let _ = worktree::discard(&record, true, true);
            }
            Err(error)
        }
    }
}

/// A new engine for the chat, which also starts a stopped one. `resume`
/// continues that session (`""` starts a fresh one); without it, the chat's own.
#[tauri::command]
pub async fn chat_resume(
    state: State<'_, AppState>,
    registry: Registry<'_>,
    chat: String,
    resume: Option<String>,
    options: Option<LaunchOptions>,
    trust_once: Option<bool>,
) -> Result<ChatSummary, String> {
    remember_options(&state, options);
    let launch = lock(&state.options).to_launch();
    let registry = Arc::clone(&registry);
    // Waits for the old engine to exit (up to its grace period): off the async threads.
    crate::blocking(move || {
        registry.restart(
            chat_id(&chat)?,
            &launch,
            resume.as_deref(),
            trust_once.unwrap_or(false),
        )
    })
    .await
}

/// Queues one client message for the chat's engine. Errors: `discarding` while
/// the chat's copy is being removed.
#[tauri::command]
pub async fn chat_send(registry: Registry<'_>, chat: String, line: String) -> Result<(), String> {
    let registry = Arc::clone(&registry);
    crate::blocking(move || registry.send(chat_id(&chat)?, &line)).await
}

/// Stops the chat's engine; the chat stays, with its session.
#[tauri::command]
pub async fn chat_stop(registry: Registry<'_>, chat: String) -> Result<(), String> {
    let registry = Arc::clone(&registry);
    crate::blocking(move || registry.stop(chat_id(&chat)?)).await
}

/// Forgets the chat. A chat with a copy of its own is not closed: the error is
/// `has_worktree` and nothing changes; the window offers the discard flow.
#[tauri::command]
pub async fn chat_close(registry: Registry<'_>, chat: String) -> Result<(), String> {
    let registry = Arc::clone(&registry);
    crate::blocking(move || registry.close(chat_id(&chat)?)).await
}

/// The chat in view, which is never stopped to make room or for being idle.
#[tauri::command]
pub fn chat_activate(registry: Registry<'_>, chat: Option<String>) -> Result<(), String> {
    registry.activate(chat.as_deref().map(chat_id).transpose()?)
}

/// What the window's state says about the chat: working, or waiting for an answer.
#[tauri::command]
pub fn chat_activity(
    registry: Registry<'_>,
    chat: String,
    busy: bool,
    waiting: bool,
) -> Result<(), String> {
    registry.activity(chat_id(&chat)?, busy, waiting)
}

/// Every chat, for a window that was reloaded.
#[tauri::command]
pub fn chat_list(registry: Registry<'_>) -> Vec<ChatSummary> {
    registry.list()
}

#[tauri::command(async)]
pub fn worktree_status(registry: Registry<'_>, chat: String) -> Result<WorktreeStatus, String> {
    worktree::status(&registry.worktree(chat_id(&chat)?)?)
}

/// The branches a copy of `path`'s repository (the default workspace when
/// absent) can start from, and the default one.
#[tauri::command(async)]
pub fn worktree_branches(
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<worktree::BranchList, String> {
    let path = path.map_or_else(
        || lock(&state.workspace).root().to_path_buf(),
        |path| PathBuf::from(path.trim()),
    );
    worktree::branches(&path)
}

/// Names a copy made without a name after its first prompt (`name` is what the
/// window made of the prompt; it is slugified here). A copy with a name keeps it.
#[tauri::command(async)]
pub fn worktree_name(
    registry: Registry<'_>,
    chat: String,
    name: String,
) -> Result<ChatSummary, String> {
    registry.rename_worktree(chat_id(&chat)?, |record| worktree::rename(record, &name))
}

/// Everything the chat's copy changed since its branch left its base.
#[tauri::command(async)]
pub fn branch_changes(registry: Registry<'_>, chat: String) -> Result<BranchChanges, String> {
    let record = registry.worktree(chat_id(&chat)?)?;
    git::branch_changes(&record.path, &record.base)
}

/// One file's change on the chat's branch, as a unified diff; the path is
/// relative to the copy's top folder.
#[tauri::command(async)]
pub fn branch_diff(registry: Registry<'_>, chat: String, path: String) -> Result<String, String> {
    let record = registry.worktree(chat_id(&chat)?)?;
    git::branch_diff(&record.path, &record.base, &path)
}

/// The pull request of the chat's branch, from `gh`, reused for a minute.
/// Quietly nothing when `gh` is missing, signed out, or there is none.
#[tauri::command(async)]
pub fn pr_status(
    registry: Registry<'_>,
    cache: State<'_, PrCache>,
    chat: String,
) -> Result<PullRequestSnapshot, String> {
    let id = chat_id(&chat)?;
    let summary = registry.summary(id)?;
    let Some(branch) = summary.branch else {
        return Ok(PullRequestSnapshot::default());
    };
    let dir = registry.cwd(id)?;
    Ok(cache.get(&dir, &branch, Instant::now(), || {
        pr::gh_pr_view(&dir, &branch)
    }))
}

/// Removes the chat's copy and its branch, and forgets the chat. Nothing is
/// stopped or removed unless every check passes first. Errors the window
/// answers: `unmerged:<n>` (commits the main folder lacks; confirm with
/// `deleteUnmerged`) and `dirty:<n>` (changed files; confirm with `force`).
/// While it runs the chat refuses to start or take messages (`discarding`).
/// The `discarded` exit is sent only after the copy is gone; if removing it
/// fails, the chat stays and works again.
#[tauri::command(async)]
pub fn worktree_discard(
    registry: Registry<'_>,
    terminals: State<'_, Terminals>,
    chat: String,
    force: bool,
    delete_unmerged: bool,
) -> Result<(), String> {
    let chat = chat_id(&chat)?;
    registry.discard_worktree(
        chat,
        // Looked at before anything is stopped, and again by `discard` itself.
        |record| worktree::check_discard(record, force, delete_unmerged).map(|_| ()),
        // Shells inside the folder would keep it locked.
        |record| terminals.close_under(&record.path),
        |record| worktree::discard(record, force, delete_unmerged),
    )
}

/// A red dot for the Windows taskbar button, 16 by 16 pixels of RGBA.
#[cfg(windows)]
fn attention_dot() -> Vec<u8> {
    const SIZE: i32 = 16;
    let mut pixels = Vec::with_capacity(16 * 16 * 4);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (dx, dy) = (2 * x - 15, 2 * y - 15);
            let inside = dx * dx + dy * dy <= 14 * 14;
            pixels.extend_from_slice(if inside {
                &[0xe5, 0x39, 0x35, 0xff]
            } else {
                &[0, 0, 0, 0]
            });
        }
    }
    pixels
}

/// How many chats need the user: a count on the dock or launcher icon, a dot on
/// the Windows taskbar button, the window title, and a flash of the taskbar
/// when the count rises while the window is in the background.
#[tauri::command]
pub fn attention_set(
    window: tauri::WebviewWindow,
    state: State<'_, AttentionState>,
    count: u32,
) -> Result<(), String> {
    let before = state.0.swap(count, std::sync::atomic::Ordering::SeqCst);
    window
        .set_title(&if count > 0 {
            format!("({count}) Neuron")
        } else {
            "Neuron".to_string()
        })
        .map_err(|error| error.to_string())?;
    #[cfg(windows)]
    {
        let icon = (count > 0).then(|| tauri::image::Image::new_owned(attention_dot(), 16, 16));
        window
            .set_overlay_icon(icon)
            .map_err(|error| error.to_string())?;
    }
    #[cfg(not(windows))]
    {
        // Some desktops (GNOME without the extension) ignore the count.
        let _ = window.set_badge_count((count > 0).then_some(i64::from(count)));
    }
    if count > before && !window.is_focused().unwrap_or(true) {
        let _ = window.request_user_attention(Some(tauri::UserAttentionType::Informational));
    }
    Ok(())
}

/// The last count `attention_set` was given.
#[derive(Default)]
pub struct AttentionState(pub std::sync::atomic::AtomicU32);

#[cfg(test)]
mod tests {
    use super::*;

    /// L5: `neuron://chat-exit` carries the engine layer's coded problem as is.
    #[test]
    fn the_exit_event_carries_the_engine_problem_unchanged() {
        let crashed = "[engine:crashed] The Neuron engine stopped unexpectedly (exit code 3). Last message: out of memory";
        let event = ChatExit::of(
            "c1abcdef",
            4,
            EngineExit {
                code: Some(3),
                error: Some(crashed.into()),
            },
            ExitReason::Exited,
        );
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            serde_json::json!({
                "chat": "c1abcdef",
                "generation": 4,
                "code": 3,
                "error": crashed,
                "reason": "exited",
            })
        );
        let idle = ChatExit::of(
            "c1abcdef",
            5,
            EngineExit {
                code: None,
                error: None,
            },
            ExitReason::Idle,
        );
        let idle = serde_json::to_value(&idle).unwrap();
        assert_eq!(idle["reason"], "idle");
        assert!(idle["error"].is_null());
    }

    #[test]
    fn chat_ids_are_checked_before_the_registry_is_asked() {
        assert_eq!(chat_id("c1abcdef"), Ok("c1abcdef"));
        for bad in ["", "x1", "c..", "c1234/..", "C1234"] {
            assert_eq!(chat_id(bad), Err("No such chat.".to_string()), "{bad:?}");
        }
    }
}
