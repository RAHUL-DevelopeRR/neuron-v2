//! Neuron Desktop's native side: it runs the engine (`neuron engine --stdio`)
//! in the open project and passes its JSON lines to the window, reads the
//! project for the file tree, `@` mentions and the Changes panel, runs terminal
//! panes, and remembers recent projects. The agent lives in the engine; the
//! window is the React app in `../src`.

pub mod engine;
pub mod git;
pub mod projects;
pub mod terminal;
pub mod workspace;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

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
/// runs. Returns its generation.
fn launch(app: &AppHandle, state: &AppState, resume: Option<String>) -> Result<Started, String> {
    if let Some((_, old)) = lock(&state.engine).take() {
        old.stop();
    }
    let workspace = lock(&state.workspace).clone();
    let binary = engine::resolve_binary().ok_or(
        "Could not find the `neuron` binary. Install Neuron, or set NEURON_BIN to its path.",
    )?;
    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let launch = Launch {
        model: std::env::var("NEURON_DESKTOP_MODEL").ok(),
        permission_mode: std::env::var("NEURON_DESKTOP_PERMISSION_MODE").ok(),
        resume: resume.filter(|id| !id.is_empty()),
    };
    let lines = app.clone();
    let exits = app.clone();
    let process = EngineProcess::spawn(
        &binary,
        workspace.root(),
        &launch,
        &Env::Inherit(Vec::new()),
        move |line| {
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

#[tauri::command]
fn engine_start(app: AppHandle, state: State<'_, AppState>) -> Result<Started, String> {
    // The window may ask twice (React runs the effect twice in development).
    if let Some((generation, _)) = lock(&state.engine).as_ref() {
        return Ok(Started {
            generation: *generation,
            project: Project::of(&lock(&state.workspace)),
        });
    }
    launch(&app, &state, None)
}

#[tauri::command]
fn engine_restart(
    app: AppHandle,
    state: State<'_, AppState>,
    resume: Option<String>,
) -> Result<Started, String> {
    launch(&app, &state, resume)
}

/// Opens another folder: a new engine there, on a fresh chat.
#[tauri::command]
fn project_open(
    app: AppHandle,
    state: State<'_, AppState>,
    terminals: State<'_, Terminals>,
    path: String,
) -> Result<Started, String> {
    let workspace = Workspace::new(path.trim())?;
    if !workspace.root().is_dir() {
        return Err(format!("{} is not a folder.", workspace.display_root()));
    }
    // Terminals belong to the old folder.
    terminals.close_all();
    *lock(&state.workspace) = workspace;
    launch(&app, &state, None)
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

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            let workspace = Workspace::new(first_workspace(&handle))
                .or_else(|_| Workspace::new("."))
                .map_err(|error| Box::<dyn std::error::Error>::from(error))?;
            app.manage(AppState {
                workspace: Mutex::new(workspace),
                engine: Mutex::new(None),
                generation: AtomicU64::new(0),
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
            open_external
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
