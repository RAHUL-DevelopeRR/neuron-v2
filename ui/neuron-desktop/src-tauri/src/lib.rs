//! Neuron Desktop's native side: it runs the engine (`neuron engine --stdio`)
//! in the open project and passes its JSON lines to the window, reads the
//! project for the file tree, `@` mentions and the Changes panel, runs terminal
//! panes, and remembers recent projects. The agent lives in the engine; the
//! window is the React app in `../src`.

pub mod diagnostics; // L5: extensions and resilience
pub mod editor;
pub mod engine;
pub mod git;
pub mod projects;
pub mod terminal;
pub mod workspace;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use engine::{EngineExit, EngineProcess, Env, Launch};
use projects::Project;
use terminal::Terminals;
use workspace::{DirListing, FileContent, Workspace};

/// The open project and the engine running in it. Every engine gets a new
/// generation; its lines and its exit carry it, so the window ignores an engine
/// it already replaced.
struct AppState {
    workspace: Mutex<Workspace>,
    engine: Mutex<Option<(u64, EngineProcess)>>,
    generation: AtomicU64,
    /// What the window last asked engines to start with (its Settings).
    options: Mutex<LaunchOptions>,
    /// The session the running engine last reported, so a reloaded window continues it.
    session: Mutex<Option<String>>,
}

/// The session id in a `status` line, if it is one.
fn status_session(line: &str) -> Option<String> {
    if !line.contains("\"status\"") {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("type")?.as_str()? != "status" {
        return None;
    }
    value
        .get("session_id")?
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
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
}

#[derive(Clone, Serialize)]
struct EngineLine {
    generation: u64,
    line: String,
}

#[derive(Clone, Serialize)]
struct EngineExitEvent {
    generation: u64,
    code: Option<i32>,
    error: Option<String>,
}

#[derive(Clone, Serialize)]
struct Started {
    generation: u64,
    project: Project,
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

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Starts an engine in the open project (continuing `resume`), replacing any that
/// runs. `trust_once` trusts the workspace for this engine only (the home folder,
/// which is never recorded). Returns its generation.
fn launch(
    app: &AppHandle,
    state: &AppState,
    resume: Option<String>,
    trust_once: bool,
) -> Result<Started, String> {
    if let Some((_, old)) = lock(&state.engine).take() {
        old.stop();
    }
    let workspace = lock(&state.workspace).clone();
    let binary = engine::resolve_binary().ok_or_else(|| diagnostics::missing_problem().render())?;
    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    // Settings first; the environment is for development.
    let options = lock(&state.options).clone();
    let launch = Launch {
        model: options
            .model
            .or_else(|| std::env::var("NEURON_DESKTOP_MODEL").ok()),
        permission_mode: options
            .permission_mode
            .or_else(|| std::env::var("NEURON_DESKTOP_PERMISSION_MODE").ok()),
        resume: resume.filter(|id| !id.is_empty()),
        reasoning_effort: options.reasoning_effort,
    };
    let lines = app.clone();
    let exits = app.clone();
    let process = EngineProcess::spawn(
        &binary,
        workspace.root(),
        &launch,
        &Env::Inherit(if trust_once {
            vec![("NEURON_TRUST_WORKSPACE".to_string(), "1".to_string())]
        } else {
            Vec::new()
        }),
        move |line| {
            if let Some(session) = status_session(&line) {
                if let Some(state) = lines.try_state::<AppState>() {
                    if lock(&state.engine)
                        .as_ref()
                        .is_none_or(|(current, _)| *current == generation)
                    {
                        *lock(&state.session) = Some(session);
                    }
                }
            }
            let _ = lines.emit("neuron://engine", EngineLine { generation, line });
        },
        move |exit: EngineExit| {
            if let Some(state) = exits.try_state::<AppState>() {
                let mut running = lock(&state.engine);
                if running
                    .as_ref()
                    .is_some_and(|(current, _)| *current == generation)
                {
                    running.take();
                }
            }
            let _ = exits.emit(
                "neuron://engine-exit",
                EngineExitEvent {
                    generation,
                    code: exit.code,
                    error: exit.error,
                },
            );
        },
    )?;
    *lock(&state.engine) = Some((generation, process));
    let project = Project::of(&workspace);
    projects::remember(app, &project);
    Ok(Started {
        generation,
        project,
    })
}

/// Keeps the window's launch options for this and later engines.
fn remember_options(state: &AppState, options: Option<LaunchOptions>) {
    if let Some(options) = options {
        *lock(&state.options) = options.checked();
    }
}

#[tauri::command]
fn engine_start(
    app: AppHandle,
    state: State<'_, AppState>,
    options: Option<LaunchOptions>,
) -> Result<Started, String> {
    remember_options(&state, options);
    // A running engine means the window was reloaded: its `ready` is gone, so a fresh
    // engine continues the session it was on.
    let resume = lock(&state.engine)
        .is_some()
        .then(|| lock(&state.session).clone())
        .flatten();
    launch(&app, &state, resume, false)
}

#[tauri::command]
fn engine_restart(
    app: AppHandle,
    state: State<'_, AppState>,
    resume: Option<String>,
    options: Option<LaunchOptions>,
    trust_once: Option<bool>,
) -> Result<Started, String> {
    remember_options(&state, options);
    launch(&app, &state, resume, trust_once.unwrap_or(false))
}

/// Opens another folder: a new engine there, on a fresh chat or continuing
/// `resume` (a chat of that folder).
#[tauri::command]
fn project_open(
    app: AppHandle,
    state: State<'_, AppState>,
    terminals: State<'_, Terminals>,
    path: String,
    resume: Option<String>,
    options: Option<LaunchOptions>,
) -> Result<Started, String> {
    remember_options(&state, options);
    let workspace = Workspace::new(path.trim())?;
    if !workspace.root().is_dir() {
        return Err(format!("{} is not a folder.", workspace.display_root()));
    }
    // Terminals belong to the old folder.
    terminals.close_all();
    *lock(&state.workspace) = workspace;
    launch(&app, &state, resume, false)
}

#[tauri::command]
fn projects_recent(app: AppHandle) -> Vec<Project> {
    projects::recent(&app)
}

#[tauri::command]
fn engine_send(state: State<'_, AppState>, line: String) -> Result<(), String> {
    let running = lock(&state.engine);
    running
        .as_ref()
        .ok_or("The Neuron engine is not running.")?
        .1
        .send(&line)
}

#[tauri::command]
fn engine_stop(state: State<'_, AppState>) {
    if let Some((_, process)) = lock(&state.engine).as_ref() {
        process.stop();
    }
}

#[tauri::command]
fn fs_list(state: State<'_, AppState>, path: String) -> Result<DirListing, String> {
    lock(&state.workspace).list(&path)
}

#[tauri::command]
fn fs_read(state: State<'_, AppState>, path: String) -> Result<FileContent, String> {
    lock(&state.workspace).read(&path)
}

#[tauri::command]
async fn fs_search(state: State<'_, AppState>, query: String) -> Result<Vec<String>, String> {
    let workspace = lock(&state.workspace).clone();
    Ok(workspace.search(&query, 50))
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
    workspace::stage_attachment(&dir, &name, &media_type, bytes)
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
    workspace::write_export(&path, &content)?;
    Ok(Some(path.display().to_string()))
}

#[tauri::command]
async fn git_status(state: State<'_, AppState>) -> Result<Option<git::GitStatusView>, String> {
    let root = lock(&state.workspace).root().to_path_buf();
    git::status(&root)
}

#[tauri::command]
async fn git_diff(state: State<'_, AppState>, path: String) -> Result<String, String> {
    let root = lock(&state.workspace).root().to_path_buf();
    git::diff(&root, &path)
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
    cols: u16,
    rows: u16,
) -> Result<u32, String> {
    let root = lock(&state.workspace).root().to_path_buf();
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

/// L2: review and git. Puts one changed file back as HEAD has it (a new file is deleted).
#[tauri::command]
async fn git_revert(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let root = lock(&state.workspace).root().to_path_buf();
    git::revert(&root, &path)
}

/// L2: review and git. Opens a workspace file in VS Code, or shows it in the file manager.
#[tauri::command]
async fn open_in_editor(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let root = lock(&state.workspace).root().to_path_buf();
    editor::open(&root, &path)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let handle = app.handle().clone();
            let workspace = Workspace::new(first_workspace(&handle))
                .or_else(|_| Workspace::new("."))
                .map_err(Box::<dyn std::error::Error>::from)?;
            app.manage(AppState {
                workspace: Mutex::new(workspace),
                engine: Mutex::new(None),
                generation: AtomicU64::new(0),
                options: Mutex::new(LaunchOptions::default()),
                session: Mutex::new(None),
            });
            Ok(())
        })
        .manage(Terminals::default())
        .invoke_handler(tauri::generate_handler![
            engine_start,
            engine_restart,
            engine_send,
            engine_stop,
            project_open,
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
            open_in_editor // L2: review and git
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                if let Some(state) = window.try_state::<AppState>() {
                    if let Some((_, process)) = lock(&state.engine).take() {
                        process.stop();
                    }
                }
                window.state::<Terminals>().close_all();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Neuron Desktop");
}
#[cfg(test)]
mod tests {
    use super::{status_session, LaunchOptions};

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
