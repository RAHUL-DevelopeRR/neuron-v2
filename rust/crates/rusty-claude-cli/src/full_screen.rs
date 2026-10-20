//! A bare `neuron` on a terminal starts Neuron's full-screen UI, `neuron-tui`,
//! when it is installed beside this binary (the npm and PyPI packages and the
//! install scripts put it there). The UI runs this binary as its engine
//! (`neuron engine --stdio`, found through `NEURON_BIN`). The line REPL stays
//! one flag away: `--no-tui`, or `NEURON_NO_TUI=1`. Without the UI, or off a
//! terminal, nothing changes.

use std::env;
use std::ffi::OsString;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::Command;

use runtime::PermissionMode;

use crate::provider::ProviderId;

pub(crate) const NO_TUI_FLAG: &str = "--no-tui";

/// What a REPL launch asked for, as the UI's flags can carry it.
pub(crate) struct UiLaunch<'a> {
    /// `--model` as typed, when given.
    pub model: Option<String>,
    pub provider: Option<ProviderId>,
    pub permission_mode: PermissionMode,
    pub allowed_tools: Option<&'a std::collections::BTreeSet<String>>,
    pub reasoning_effort: Option<&'a str>,
    pub allow_broad_cwd: bool,
    pub base_commit: Option<&'a str>,
}

/// Removes `--no-tui` from the options (not from a prompt after `-p`, `--` or
/// `prompt`) and says whether it was there.
pub(crate) fn extract_no_tui_flag(args: Vec<String>) -> (Vec<String>, bool) {
    let mut remaining = Vec::with_capacity(args.len());
    let mut found = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == "-p" || arg == "--" || arg == "prompt" {
            remaining.push(arg);
            remaining.extend(iter);
            break;
        }
        if arg == NO_TUI_FLAG {
            found = true;
        } else {
            remaining.push(arg);
        }
    }
    (remaining, found)
}

fn env_truthy(name: &str) -> bool {
    env::var(name).is_ok_and(|value| {
        let value = value.trim().to_ascii_lowercase();
        !value.is_empty() && value != "0" && value != "false" && value != "no"
    })
}

fn ui_file_name() -> &'static str {
    if cfg!(windows) {
        "neuron-tui.exe"
    } else {
        "neuron-tui"
    }
}

/// The UI binary: `NEURON_TUI_BIN` when set (a launcher that keeps it
/// elsewhere), else `neuron-tui` beside `exe`.
pub(crate) fn locate_ui(explicit: Option<OsString>, exe: &Path) -> Option<PathBuf> {
    if let Some(explicit) = explicit.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(explicit);
        return path.is_file().then_some(path);
    }
    let sibling = exe.parent()?.join(ui_file_name());
    sibling.is_file().then_some(sibling)
}

/// The UI to start instead of the line REPL, or `None`: turned off, not on a
/// terminal, or not installed.
pub(crate) fn ui_binary(no_tui_flag: bool) -> Option<PathBuf> {
    if no_tui_flag || env_truthy("NEURON_NO_TUI") {
        return None;
    }
    if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        return None;
    }
    let exe = env::current_exe().ok()?;
    locate_ui(env::var_os("NEURON_TUI_BIN"), &exe)
}

/// The UI's arguments for a REPL launch, or `None` when it can't carry one of
/// them (`--base-commit`), so the line REPL runs instead.
pub(crate) fn ui_args(launch: &UiLaunch<'_>) -> Option<Vec<String>> {
    if launch.base_commit.is_some() {
        return None;
    }
    let mut args = Vec::new();
    if let Some(model) = &launch.model {
        args.extend(["--model".to_string(), model.clone()]);
    }
    if let Some(provider) = launch.provider {
        args.extend(["--provider".to_string(), provider.as_str().to_string()]);
    }
    args.extend([
        "--permission-mode".to_string(),
        launch.permission_mode.as_str().to_string(),
    ]);
    if let Some(tools) = launch.allowed_tools {
        let tools: Vec<&str> = tools.iter().map(String::as_str).collect();
        args.extend(["--allowedTools".to_string(), tools.join(",")]);
    }
    if let Some(effort) = launch.reasoning_effort {
        args.extend(["--reasoning-effort".to_string(), effort.to_string()]);
    }
    if launch.allow_broad_cwd {
        args.push("--allow-broad-cwd".to_string());
    }
    Some(args)
}

/// Runs the UI with this binary as its engine. On Unix the UI replaces this
/// process; on Windows this process waits for it, ignoring the console's
/// Ctrl-C and Ctrl-Break (the UI handles those keys), and exits with its code.
pub(crate) fn run_ui(binary: &Path, args: &[String]) -> io::Result<i32> {
    let engine = env::current_exe()?;
    let mut command = Command::new(binary);
    command.args(args).env("NEURON_BIN", engine);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Returns only if the exec failed.
        Err(command.exec())
    }
    #[cfg(not(unix))]
    {
        let _guard = IgnoreConsoleInterrupts::install();
        let status = command.status()?;
        Ok(status.code().unwrap_or(1))
    }
}

/// While alive, the console's Ctrl-C and Ctrl-Break don't end this process:
/// Tokio's listeners mark them handled. The UI, in the same console, still
/// gets the keys.
#[cfg(windows)]
struct IgnoreConsoleInterrupts {
    _runtime: Option<tokio::runtime::Runtime>,
    _ctrl_c: Option<tokio::signal::windows::CtrlC>,
    _ctrl_break: Option<tokio::signal::windows::CtrlBreak>,
}

#[cfg(windows)]
impl IgnoreConsoleInterrupts {
    fn install() -> Self {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return Self {
                _runtime: None,
                _ctrl_c: None,
                _ctrl_break: None,
            };
        };
        let (ctrl_c, ctrl_break) = runtime.block_on(async {
            (
                tokio::signal::windows::ctrl_c().ok(),
                tokio::signal::windows::ctrl_break().ok(),
            )
        });
        Self {
            _runtime: Some(runtime),
            _ctrl_c: ctrl_c,
            _ctrl_break: ctrl_break,
        }
    }
}

#[cfg(all(not(unix), not(windows)))]
struct IgnoreConsoleInterrupts;

#[cfg(all(not(unix), not(windows)))]
impl IgnoreConsoleInterrupts {
    fn install() -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn no_tui_is_taken_from_the_options_only() {
        assert_eq!(
            extract_no_tui_flag(strings(&["--no-tui", "--model", "opus"])),
            (strings(&["--model", "opus"]), true)
        );
        assert_eq!(
            extract_no_tui_flag(strings(&["-p", "say", "--no-tui"])),
            (strings(&["-p", "say", "--no-tui"]), false)
        );
        assert_eq!(
            extract_no_tui_flag(strings(&["--model", "opus"])),
            (strings(&["--model", "opus"]), false)
        );
    }

    #[test]
    fn ui_args_carry_the_repl_flags() {
        let tools: std::collections::BTreeSet<String> = ["bash", "read_file"]
            .iter()
            .map(|t| (*t).to_string())
            .collect();
        let launch = UiLaunch {
            model: Some("opus".to_string()),
            provider: Some(ProviderId::Anthropic),
            permission_mode: PermissionMode::ReadOnly,
            allowed_tools: Some(&tools),
            reasoning_effort: Some("high"),
            allow_broad_cwd: true,
            base_commit: None,
        };
        assert_eq!(
            ui_args(&launch),
            Some(strings(&[
                "--model",
                "opus",
                "--provider",
                "anthropic",
                "--permission-mode",
                "read-only",
                "--allowedTools",
                "bash,read_file",
                "--reasoning-effort",
                "high",
                "--allow-broad-cwd",
            ]))
        );
    }

    #[test]
    fn a_base_commit_keeps_the_line_repl() {
        let launch = UiLaunch {
            model: None,
            provider: None,
            permission_mode: PermissionMode::WorkspaceWrite,
            allowed_tools: None,
            reasoning_effort: None,
            allow_broad_cwd: false,
            base_commit: Some("HEAD~1"),
        };
        assert_eq!(ui_args(&launch), None);
    }

    #[test]
    fn the_ui_is_found_beside_the_binary_or_where_the_launcher_says() {
        let dir = std::env::temp_dir().join(format!("neuron-full-screen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(if cfg!(windows) {
            "neuron.exe"
        } else {
            "neuron"
        });
        assert_eq!(locate_ui(None, &exe), None);

        let sibling = dir.join(ui_file_name());
        std::fs::write(&sibling, b"ui").unwrap();
        assert_eq!(locate_ui(None, &exe), Some(sibling.clone()));

        let elsewhere = dir.join("cache-neuron-tui");
        assert_eq!(locate_ui(Some(elsewhere.clone().into()), &exe), None);
        std::fs::write(&elsewhere, b"ui").unwrap();
        assert_eq!(
            locate_ui(Some(elsewhere.clone().into()), &exe),
            Some(elsewhere)
        );
        assert_eq!(locate_ui(Some(OsString::new()), &exe), Some(sibling));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
