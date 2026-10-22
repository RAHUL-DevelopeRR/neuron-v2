//! Neuron Desktop's native side: it runs the engine (`neuron engine --stdio`)
//! and passes its JSON lines to the window, reads the workspace for the file
//! tree, and runs terminal panes. The agent lives in the engine; the window is
//! the React app in `../src`.

pub mod engine;
pub mod terminal;
pub mod workspace;

use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use engine::{EngineExit, EngineProcess, Env, Launch};
use terminal::Terminals;
use workspace::{DirListing, FileContent, Workspace};

/// The engine of this window, while it runs.
#[derive(Default)]
struct EngineSlot(Mutex<Option<EngineProcess>>);

/// The folder the app works in: `NEURON_WORKSPACE`, the first argument, or the
/// folder it was started from.
fn workspace_root() -> PathBuf {
    std::env::var_os("NEURON_WORKSPACE")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::args_os()
                .nth(1)
                .map(PathBuf::from)
                .filter(|path| path.is_dir())
        })
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[tauri::command]
fn engine_start(
    app: AppHandle,
    slot: State<'_, EngineSlot>,
    workspace: State<'_, Workspace>,
) -> Result<(), String> {
    let mut running = slot.0.lock().map_err(|_| "engine lock poisoned")?;
    if running.is_some() {
        return Ok(());
    }
    let binary = engine::resolve_binary().ok_or(
        "Could not find the `neuron` binary. Install Neuron, or set NEURON_BIN to its path.",
    )?;
    let launch = Launch {
        model: std::env::var("NEURON_DESKTOP_MODEL").ok(),
        permission_mode: std::env::var("NEURON_DESKTOP_PERMISSION_MODE").ok(),
        resume: None,
    };
    let lines = app.clone();
    let exits = app.clone();
    let process = EngineProcess::spawn(
        &binary,
        workspace.root(),
        &launch,
        &Env::Inherit(Vec::new()),
        move |line| {
            let _ = lines.emit("neuron://engine", line);
        },
        move |exit: EngineExit| {
            if let Some(slot) = exits.try_state::<EngineSlot>() {
                if let Ok(mut running) = slot.0.lock() {
                    running.take();
                }
            }
            let _ = exits.emit("neuron://engine-exit", exit);
        },
    )?;
    *running = Some(process);
    Ok(())
}

#[tauri::command]
fn engine_send(slot: State<'_, EngineSlot>, line: String) -> Result<(), String> {
    let running = slot.0.lock().map_err(|_| "engine lock poisoned")?;
    running
        .as_ref()
        .ok_or("The Neuron engine is not running.")?
        .send(&line)
}

#[tauri::command]
fn engine_stop(slot: State<'_, EngineSlot>) {
    if let Ok(running) = slot.0.lock() {
        if let Some(process) = running.as_ref() {
            process.stop();
        }
    }
}

#[tauri::command]
fn fs_list(workspace: State<'_, Workspace>, path: String) -> Result<DirListing, String> {
    workspace.list(&path)
}

#[tauri::command]
fn fs_read(workspace: State<'_, Workspace>, path: String) -> Result<FileContent, String> {
    workspace.read(&path)
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
    workspace: State<'_, Workspace>,
    cols: u16,
    rows: u16,
) -> Result<u32, String> {
    let output = app.clone();
    terminals.open(
        workspace.root(),
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
    let workspace = match Workspace::new(workspace_root()) {
        Ok(workspace) => workspace,
        Err(error) => {
            eprintln!("neuron-desktop: {error}");
            std::process::exit(1);
        }
    };
    tauri::Builder::default()
        .manage(EngineSlot::default())
        .manage(Terminals::default())
        .manage(workspace)
        .invoke_handler(tauri::generate_handler![
            engine_start,
            engine_send,
            engine_stop,
            fs_list,
            fs_read,
            term_open,
            term_write,
            term_resize,
            term_close,
            open_external
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                engine_stop(window.state::<EngineSlot>());
                window.state::<Terminals>().close_all();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Neuron Desktop");
}
