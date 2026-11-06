//! Neuron Desktop's native side: it runs the engine (`neuron engine --stdio`)
//! in the open project and passes its JSON lines to the window, reads the
//! project for the file tree, `@` mentions and the Changes panel, runs terminal
//! panes, and remembers recent projects. The agent lives in the engine; the
//! window is the React app in `../src`.

pub mod chats;
pub mod diagnostics; // L5: extensions and resilience
pub mod editor;
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
use tauri_plugin_dialog::DialogExt;

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
    /// L3: composer. `low`, `medium` or `high`.
    pub reasoning_effort: Option<String>,
}

const REASONING_EFFORTS: [&str; 3] = ["low", "medium", "high"];
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
        let reasoning_effort = self
            .reasoning_effort
            .filter(|effort| REASONING_EFFORTS.contains(&effort.as_str()));
        Self {
            model,
            permission_mode,
            reasoning_effort,
        }
    }

    /// The `NEURON_DESKTOP_*` development variables, held to the same checks as
    /// options from the window.
    fn from_env_values(model: Option<String>, permission_mode: Option<String>) -> Self {
        Self {
            model,
            permission_mode,
            reasoning_effort: None,
        }
        .checked()
    }

    /// What an engine starts with: these options first; the environment is for
    /// development. The one place the options become launch flags.
    fn to_launch(&self) -> Launch {
        let from_env = Self::from_env_values(
            std::env::var("NEURON_DESKTOP_MODEL").ok(),
            std::env::var("NEURON_DESKTOP_PERMISSION_MODE").ok(),
        );
        Launch {
            model: self.model.clone().or(from_env.model),
            permission_mode: self.permission_mode.clone().or(from_env.permission_mode),
            resume: None,
            reasoning_effort: self.reasoning_effort.clone(), // L3: composer
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

/// Runs blocking work (git, files, launching programs) off the async command
/// threads, so one slow call never stalls the others.
pub(crate) async fn blocking<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
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
    blocking(move || Ok(workspace.search(&query, 50))).await
}

/// L3: composer. Copies a pasted or dropped image into the app's staging folder. The
/// bytes are the raw request body; `x-attachment-name` (percent-encoded) and
/// `x-media-type` are headers.
#[tauri::command]
async fn attachment_stage(
    app: AppHandle,
    request: tauri::ipc::Request<'_>,
) -> Result<workspace::StagedAttachment, String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("The image did not arrive.".to_string());
    };
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    let name = workspace::percent_decode(&header("x-attachment-name"));
    let media_type = header("x-media-type");
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("attachments");
    let bytes = bytes.to_vec();
    blocking(move || workspace::stage_attachment(&dir, &name, &media_type, &bytes)).await
}

/// L3: composer. Saves a chat export where the user says, in the system's save
/// dialog: the window never names the path. Returns the saved path, or `None` when
/// the dialog was cancelled.
#[tauri::command]
async fn export_save(
    app: AppHandle,
    suggested_name: String,
    content: String,
) -> Result<Option<String>, String> {
    if content.len() > workspace::MAX_EXPORT_BYTES {
        return Err("That chat is too large to save.".to_string());
    }
    let name = workspace::export_file_name(&suggested_name);
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_file_name(name)
            .add_filter("Chat", &["md", "json"])
            .blocking_save_file()
    })
    .await
    .map_err(|error| error.to_string())?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|error| error.to_string())?;
    blocking(move || {
        workspace::write_export(&path, &content)?;
        Ok(Some(path.display().to_string()))
    })
    .await
}

#[tauri::command]
async fn git_status(
    state: State<'_, AppState>,
    registry: State<'_, Arc<EngineRegistry>>,
    chat: Option<String>,
) -> Result<Option<git::GitStatusView>, String> {
    let workspace = chats::workspace_for(&state, &registry, chat.as_deref())?;
    blocking(move || git::status(workspace.root())).await
}

#[tauri::command]
async fn git_diff(
    state: State<'_, AppState>,
    registry: State<'_, Arc<EngineRegistry>>,
    path: String,
    chat: Option<String>,
) -> Result<String, String> {
    let workspace = chats::workspace_for(&state, &registry, chat.as_deref())?;
    blocking(move || git::diff(workspace.root(), &path)).await
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

/// L5: which engine would run (`env`, `bundled`, `path` or `missing`).
#[tauri::command]
fn engine_info() -> diagnostics::EngineInfo {
    diagnostics::current_engine_info()
}

/// L5: the host's part of the support report (no account data, no environment).
#[tauri::command]
fn diagnostics(app: AppHandle) -> String {
    diagnostics::current_diagnostics(&app.package_info().version.to_string())
}

/// L2: review and git. Puts one changed file back as HEAD has it (a new file is deleted),
/// in the chat's folder: a chat in a copy of its own reverts in that copy.
#[tauri::command]
async fn git_revert(
    state: State<'_, AppState>,
    registry: State<'_, Arc<EngineRegistry>>,
    path: String,
    chat: Option<String>,
) -> Result<(), String> {
    let workspace = chats::workspace_for(&state, &registry, chat.as_deref())?;
    blocking(move || git::revert(workspace.root(), &path)).await
}

/// L2: review and git. Opens a file of the chat's folder in VS Code, or shows it in
/// the file manager. `.` is the folder itself (the chat bar's "Open in editor").
#[tauri::command]
async fn open_in_editor(
    state: State<'_, AppState>,
    registry: State<'_, Arc<EngineRegistry>>,
    path: String,
    chat: Option<String>,
) -> Result<(), String> {
    let workspace = chats::workspace_for(&state, &registry, chat.as_deref())?;
    blocking(move || editor::open(workspace.root(), &path)).await
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
            // Staged images older than a week go (only from the staging folder).
            if let Ok(data) = app.path().app_data_dir() {
                let staging = data.join("attachments");
                std::thread::spawn(move || {
                    workspace::prune_staged(
                        &staging,
                        std::time::SystemTime::now(),
                        workspace::STAGED_MAX_AGE,
                    );
                });
            }
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
            engine_info, // L5: extensions and resilience
            diagnostics,
            attachment_stage, // L3: composer
            export_save,      // L3: composer
            git_revert,
            open_in_editor, // L2: review and git
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
            ..LaunchOptions::default()
        }
        .checked();
        assert_eq!(options.model.as_deref(), Some("sonnet"));
        assert_eq!(options.permission_mode.as_deref(), Some("workspace-write"));

        let refused = LaunchOptions {
            model: Some("--dangerously-skip-permissions".into()),
            permission_mode: Some("bypass".into()),
            ..LaunchOptions::default()
        }
        .checked();
        assert_eq!(refused, LaunchOptions::default());
        let control = LaunchOptions {
            model: Some("gpt\n--x".into()),
            permission_mode: None,
            ..LaunchOptions::default()
        }
        .checked();
        assert_eq!(control.model, None);
    }

    #[test]
    fn launch_options_keep_a_known_reasoning_effort() {
        for effort in ["low", "medium", "high"] {
            let options = LaunchOptions {
                reasoning_effort: Some(effort.into()),
                ..LaunchOptions::default()
            }
            .checked();
            assert_eq!(options.reasoning_effort.as_deref(), Some(effort));
        }
        for refused in ["max", "--model", "HIGH", "", "high\n"] {
            let options = LaunchOptions {
                reasoning_effort: Some(refused.into()),
                ..LaunchOptions::default()
            }
            .checked();
            assert_eq!(options.reasoning_effort, None, "{refused:?}");
        }
        let read: LaunchOptions = serde_json::from_str(r#"{"reasoningEffort":"low"}"#).unwrap();
        assert_eq!(read.reasoning_effort.as_deref(), Some("low"));
    }

    /// L3 + L4: what the window asked for becomes the flags of every engine the
    /// registry starts (`chat_open` and `chat_resume` both use `to_launch`).
    #[test]
    fn launch_options_become_the_engine_flags() {
        let launch = LaunchOptions {
            model: Some("sonnet".into()),
            permission_mode: Some("read-only".into()),
            reasoning_effort: Some("high".into()),
        }
        .checked()
        .to_launch();
        assert_eq!(launch.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(launch.resume, None);
        assert_eq!(
            crate::engine::engine_args(&launch),
            [
                "engine",
                "--stdio",
                "--model",
                "sonnet",
                "--permission-mode",
                "read-only",
                "--reasoning-effort",
                "high"
            ]
        );
        // An effort the engine would refuse is no flag at all.
        let refused = LaunchOptions {
            model: Some("sonnet".into()),
            permission_mode: Some("read-only".into()),
            reasoning_effort: Some("max".into()),
        }
        .checked()
        .to_launch();
        assert!(!crate::engine::engine_args(&refused).contains(&"--reasoning-effort".to_string()));
    }

    #[test]
    fn launch_options_from_the_environment_are_checked_too() {
        let refused = LaunchOptions::from_env_values(
            Some("--dangerously-skip-permissions".into()),
            Some("bypass".into()),
        );
        assert_eq!(refused, LaunchOptions::default());
        let kept =
            LaunchOptions::from_env_values(Some(" sonnet ".into()), Some("read-only".into()));
        assert_eq!(kept.model.as_deref(), Some("sonnet"));
        assert_eq!(kept.permission_mode.as_deref(), Some("read-only"));
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
