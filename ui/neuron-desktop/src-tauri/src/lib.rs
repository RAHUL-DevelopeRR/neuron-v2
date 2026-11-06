//! Neuron Desktop's native side: it runs the engine (`neuron engine --stdio`)
//! in the open project and passes its JSON lines to the window, reads the
//! project for the file tree, `@` mentions and the Changes panel, runs terminal
//! panes, and remembers recent projects. The agent lives in the engine; the
//! window is the React app in `../src`.

pub mod chats;
pub mod engine;
pub mod git;
pub mod projects;
pub mod registry;
pub mod terminal;
pub mod workspace;
pub mod worktree;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use engine::Launch;
use projects::Project;
use registry::{EngineRegistry, ProcessSpawner};
use terminal::Terminals;
use workspace::{DirListing, FileContent, Workspace};

/// The folder the app opened first (what a command without a chat reads), and
/// what the window last asked engines to start with. Engines and chats live in
/// the registry (`registry.rs`).
pub struct AppState {
    workspace: Mutex<Workspace>,
    /// What the window last asked engines to start with (its Settings).
    options: Mutex<LaunchOptions>,
}

/// The window's defaults for new engines: Settings › Sessions.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct LaunchOptions {
    pub model: Option<String>,
    pub permission_mode: Option<String>,
}

const PERMISSION_MODES: [&str; 3] = ["read-only", "workspace-write", "danger-full-access"];

impl LaunchOptions {
    /// Drops what an engine would refuse: an unknown mode, or a model name with
    /// control characters or that could read as a flag.
    fn checked(self) -> Self {
        let model = self
            .model
            .map(|model| model.trim().to_string())
            .filter(|model| {
                !model.is_empty() && !model.starts_with('-') && !model.chars().any(char::is_control)
            });
        let permission_mode = self
            .permission_mode
            .filter(|mode| PERMISSION_MODES.contains(&mode.as_str()));
        Self {
            model,
            permission_mode,
        }
    }

    /// What an engine starts with: these options first; the environment is for
    /// development. The one place the options become launch flags.
    fn to_launch(&self) -> Launch {
        Launch {
            model: self
                .model
                .clone()
                .or_else(|| std::env::var("NEURON_DESKTOP_MODEL").ok()),
            permission_mode: self
                .permission_mode
                .clone()
                .or_else(|| std::env::var("NEURON_DESKTOP_PERMISSION_MODE").ok()),
            resume: None,
        }
    }
}

/// The folder the app opens first: `NEURON_WORKSPACE`, the first argument, the
/// most recent project, or the folder it was started from.
fn first_workspace(app: &AppHandle) -> PathBuf {
    std::env::var_os("NEURON_WORKSPACE")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::args_os()
                .nth(1)
                .map(PathBuf::from)
                .filter(|path| path.is_dir())
        })
        .or_else(|| {
            projects::recent(app)
                .into_iter()
                .map(|project| PathBuf::from(project.path))
                .find(|path| path.is_dir())
        })
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[tauri::command]
fn projects_recent(app: AppHandle) -> Vec<Project> {
    projects::recent(&app)
}

/// The commands below read the chat's folder, or the default workspace without a chat.
#[tauri::command]
fn fs_list(
    state: State<'_, AppState>,
    registry: State<'_, Arc<EngineRegistry>>,
    path: String,
    chat: Option<String>,
) -> Result<DirListing, String> {
    chats::workspace_for(&state, &registry, chat.as_deref())?.list(&path)
}

#[tauri::command]
fn fs_read(
    state: State<'_, AppState>,
    registry: State<'_, Arc<EngineRegistry>>,
    path: String,
    chat: Option<String>,
) -> Result<FileContent, String> {
    chats::workspace_for(&state, &registry, chat.as_deref())?.read(&path)
}

#[tauri::command]
async fn fs_search(
    state: State<'_, AppState>,
    registry: State<'_, Arc<EngineRegistry>>,
    query: String,
    chat: Option<String>,
) -> Result<Vec<String>, String> {
    let workspace = chats::workspace_for(&state, &registry, chat.as_deref())?;
    Ok(workspace.search(&query, 50))
}

#[tauri::command]
async fn git_status(
    state: State<'_, AppState>,
    registry: State<'_, Arc<EngineRegistry>>,
    chat: Option<String>,
) -> Result<Option<git::GitStatusView>, String> {
    let workspace = chats::workspace_for(&state, &registry, chat.as_deref())?;
    git::status(workspace.root())
}

#[tauri::command]
async fn git_diff(
    state: State<'_, AppState>,
    registry: State<'_, Arc<EngineRegistry>>,
    path: String,
    chat: Option<String>,
) -> Result<String, String> {
    let workspace = chats::workspace_for(&state, &registry, chat.as_deref())?;
    git::diff(workspace.root(), &path)
}

#[derive(Clone, Serialize)]
struct TerminalOutput {
    id: u32,
    data: String,
}

#[derive(Clone, Serialize)]
struct TerminalExit {
    id: u32,
    code: Option<u32>,
}

#[tauri::command]
fn term_open(
    app: AppHandle,
    terminals: State<'_, Terminals>,
    state: State<'_, AppState>,
    registry: State<'_, Arc<EngineRegistry>>,
    cols: u16,
    rows: u16,
    chat: Option<String>,
) -> Result<u32, String> {
    let root = chats::workspace_for(&state, &registry, chat.as_deref())?
        .root()
        .to_path_buf();
    let output = app.clone();
    terminals.open(
        &root,
        cols,
        rows,
        move |id, data| {
            let _ = output.emit("neuron://term-output", TerminalOutput { id, data });
        },
        move |id, code| {
            let _ = app.emit("neuron://term-exit", TerminalExit { id, code });
        },
    )
}

#[tauri::command]
fn term_write(terminals: State<'_, Terminals>, id: u32, data: String) -> Result<(), String> {
    terminals.write(id, &data)
}

#[tauri::command]
fn term_resize(
    terminals: State<'_, Terminals>,
    id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    terminals.resize(id, cols, rows)
}

#[tauri::command]
fn term_close(terminals: State<'_, Terminals>, id: u32) {
    terminals.close(id);
}

/// Opens an http(s) link in the user's browser (sign-in pages, links in replies).
#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://"))
        || url.chars().any(char::is_control)
    {
        return Err("Only http and https links open.".into());
    }
    #[cfg(windows)]
    let mut command = std::process::Command::new("rundll32");
    #[cfg(windows)]
    command.args(["url.dll,FileProtocolHandler", &url]);
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "macos")]
    command.arg(&url);
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    #[cfg(all(unix, not(target_os = "macos")))]
    command.arg(&url);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let workspace = Workspace::new(first_workspace(&handle))
                .or_else(|_| Workspace::new("."))
                .map_err(Box::<dyn std::error::Error>::from)?;
            app.manage(AppState {
                workspace: Mutex::new(workspace),
                options: Mutex::new(LaunchOptions::default()),
            });
            let registry = EngineRegistry::new(
                Arc::new(ProcessSpawner::default()),
                Arc::new(chats::WindowEvents(handle)),
            );
            chats::start_reaper(Arc::clone(&registry));
            app.manage(registry);
            Ok(())
        })
        .manage(Terminals::default())
        .manage(chats::AttentionState::default())
        .invoke_handler(tauri::generate_handler![
            projects_recent,
            fs_list,
            fs_read,
            fs_search,
            git_status,
            git_diff,
            term_open,
            term_write,
            term_resize,
            term_close,
            open_external,
            // L4: multi-chat
            chats::chat_open,
            chats::chat_resume,
            chats::chat_send,
            chats::chat_stop,
            chats::chat_close,
            chats::chat_activate,
            chats::chat_activity,
            chats::chat_list,
            chats::worktree_status,
            chats::worktree_discard,
            chats::attention_set
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                if let Some(registry) = window.try_state::<Arc<EngineRegistry>>() {
                    registry.shutdown_all();
                }
                window.state::<Terminals>().close_all();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Neuron Desktop");
}
#[cfg(test)]
mod tests {
    use super::LaunchOptions;
    use crate::engine::status_session;

    #[test]
    fn status_lines_name_the_session() {
        assert_eq!(
            status_session(r#"{"type":"status","session_id":"session-7","model":"sonnet"}"#)
                .as_deref(),
            Some("session-7")
        );
        assert_eq!(status_session(r#"{"type":"status","session_id":""}"#), None);
        assert_eq!(
            status_session(r#"{"type":"text_delta","text":"\"status\""}"#),
            None
        );
        assert_eq!(status_session("not json \"status\""), None);
    }

    #[test]
    fn launch_options_keep_only_what_an_engine_takes() {
        let options = LaunchOptions {
            model: Some("  sonnet ".into()),
            permission_mode: Some("workspace-write".into()),
        }
        .checked();
        assert_eq!(options.model.as_deref(), Some("sonnet"));
        assert_eq!(options.permission_mode.as_deref(), Some("workspace-write"));

        let refused = LaunchOptions {
            model: Some("--dangerously-skip-permissions".into()),
            permission_mode: Some("bypass".into()),
        }
        .checked();
        assert_eq!(refused, LaunchOptions::default());
        let control = LaunchOptions {
            model: Some("gpt\n--x".into()),
            permission_mode: None,
        }
        .checked();
        assert_eq!(control.model, None);
    }

    #[test]
    fn launch_options_read_the_window_names() {
        let options: LaunchOptions =
            serde_json::from_str(r#"{"model":"opus","permissionMode":"read-only"}"#).unwrap();
        assert_eq!(options.model.as_deref(), Some("opus"));
        assert_eq!(options.permission_mode.as_deref(), Some("read-only"));
        let empty: LaunchOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, LaunchOptions::default());
    }
}
