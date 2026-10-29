//! A bare `neuron` on a terminal starts Neuron's full-screen UI, `neuron-tui`,
//! when it is installed beside this binary (the npm and PyPI packages and the
//! install scripts put it there). The UI runs this binary as its engine
//! (`neuron engine --stdio`, found through `NEURON_BIN`). The line REPL stays
//! one flag away: `--no-tui`, or `NEURON_NO_TUI=1`. Without the UI, or off a
//! terminal, nothing changes. `--continue [chat-id]`, which the UI's exit hint
//! names, is the UI's own flag: it is passed on to it.

use std::env;
use std::ffi::OsString;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::Command;

use runtime::PermissionMode;

use crate::provider::ProviderId;

pub(crate) const NO_TUI_FLAG: &str = "--no-tui";

/// For `--continue` where the UI can't run it.
pub(crate) const CONTINUE_NEEDS_UI: &str = "--continue reopens a chat in the full-screen UI \
(neuron-tui), which needs a terminal and neuron-tui installed beside neuron, and can't be \
combined with a prompt, a subcommand or --no-tui. In the line REPL, use --resume instead; \
from a script, send a saved session another prompt with `neuron --resume latest prompt \"...\"`.";

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
    pub continue_chat: Option<ContinueChat>,
}

/// `--continue`: the latest chat, or the one named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContinueChat {
    Latest,
    Chat(String),
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

/// Removes `--continue [chat-id]` (or `--continue=<chat-id>`) from the options,
/// as `extract_no_tui_flag` does. The CLI's own parser has no such flag.
pub(crate) fn extract_continue_flag(args: Vec<String>) -> (Vec<String>, Option<ContinueChat>) {
    let mut remaining = Vec::with_capacity(args.len());
    let mut found = None;
    let mut iter = args.into_iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "-p" || arg == "--" || arg == "prompt" {
            remaining.push(arg);
            remaining.extend(iter);
            break;
        }
        if arg == "--continue" {
            found = Some(match iter.peek() {
                Some(next) if !next.starts_with('-') => {
                    ContinueChat::Chat(iter.next().unwrap_or_default())
                }
                _ => ContinueChat::Latest,
            });
        } else if let Some(id) = arg.strip_prefix("--continue=") {
            found = Some(if id.is_empty() {
                ContinueChat::Latest
            } else {
                ContinueChat::Chat(id.to_string())
            });
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
    if let Some(chat) = &launch.continue_chat {
        args.push("--continue".to_string());
        if let ContinueChat::Chat(id) = chat {
            args.push(id.clone());
        }
    }
    Some(args)
}

/// Where the UI leaves its exit message (the `--continue` hint) for this process to
/// print; read by `exit-cleanly.ts`.
pub(crate) const EXIT_MESSAGE_FILE_ENV: &str = "NEURON_TUI_EXIT_MESSAGE_FILE";

/// Where the UI says whether the terminal is in its modes: "armed" once it sets
/// them up, "disarmed" once it has reset them (`terminal-watchdog.ts`). With it,
/// the UI starts no watchdog process of its own: this process is the watchdog.
pub(crate) const DISARM_FILE_ENV: &str = "NEURON_TUI_DISARM_FILE";

/// The UI's terminal resets (`ui/neuron-tui/src/utils/terminal-reset-sequences.ts`;
/// a UI test checks the two agree): leave the alternate screen, turn off mouse
/// tracking, focus reports and bracketed paste, pop the keyboard modes, show the
/// cursor.
pub(crate) const TERMINAL_RESET_SEQUENCES: &str = "\x1b[?1049l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l\x1b[?2004l\x1b[<u\x1b[>4;0m\x1b[?25h";

/// Runs the UI with this binary as its engine. On Unix the UI replaces this
/// process; on Windows this process waits for it, ignoring the console's
/// Ctrl-C and Ctrl-Break (the UI handles those keys), gives the console its
/// input mode back, resets the terminal if the UI died in its modes, prints
/// the UI's exit message, and exits with its code.
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
        // The UI's own write races its renderer's teardown, which leaves the
        // alternate screen on a thread of its own; once the UI has exited, the
        // main screen is back for certain.
        let exit_message = temp_file("exit");
        // This process waits for the UI anyway, so it is the UI's terminal
        // watchdog. The UI's own one is a PowerShell on the user's console,
        // which retitled the window and was one more process on its input.
        let disarm = temp_file("terminal");
        command
            .env(EXIT_MESSAGE_FILE_ENV, &exit_message)
            .env(DISARM_FILE_ENV, &disarm);
        let _guard = IgnoreConsoleInterrupts::install();
        let input_mode = SavedInputMode::save();
        let status = command.status();
        input_mode.restore();
        let mut stdout = io::stdout();
        if ui_left_terminal_armed(&disarm) {
            // The UI turned on virtual-terminal processing for the console;
            // make sure the resets are read as such, not printed.
            let _ = crossterm::ansi_support::supports_ansi();
            let _ = io::Write::write_all(&mut stdout, TERMINAL_RESET_SEQUENCES.as_bytes());
            let _ = io::Write::flush(&mut stdout);
        }
        print_exit_message(&exit_message, &mut stdout);
        Ok(status?.code().unwrap_or(1))
    }
}

/// A file name of this process's own in the temp folder.
#[cfg_attr(unix, allow(dead_code))]
fn temp_file(kind: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    env::temp_dir().join(format!(
        "neuron-tui-{kind}-{}-{nanos}.txt",
        std::process::id()
    ))
}

/// Whether the UI exited with the terminal still in its modes: it wrote "armed"
/// to `file` and never "disarmed" (it died, or its reset failed). Removes the file.
/// No file means the UI never got as far as setting the terminal up.
#[cfg_attr(unix, allow(dead_code))]
fn ui_left_terminal_armed(file: &Path) -> bool {
    let Ok(state) = std::fs::read_to_string(file) else {
        return false;
    };
    let _ = std::fs::remove_file(file);
    state.trim() == "armed"
}

/// Prints and removes the message the UI left in `file`, if it left one.
#[cfg_attr(unix, allow(dead_code))]
fn print_exit_message(file: &Path, out: &mut impl io::Write) {
    let Ok(message) = std::fs::read_to_string(file) else {
        return;
    };
    let _ = std::fs::remove_file(file);
    let _ = out.write_all(message.as_bytes());
    let _ = out.flush();
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

/// The console's input mode as this process found it, put back once the UI
/// has exited. The UI's raw mode turns on virtual-terminal input, and a UI
/// that died leaves it on: cmd's Up arrow then types `ESC [ A`, and an Enter
/// typed under it reaches PowerShell as a character, not a key.
#[cfg(windows)]
struct SavedInputMode(Option<u32>);

#[cfg(windows)]
impl SavedInputMode {
    fn save() -> Self {
        Self(console_input().and_then(|console| console.mode().ok()))
    }

    fn restore(&self) {
        if let (Some(mode), Some(console)) = (self.0, console_input()) {
            let _ = console.set_mode(mode);
        }
    }
}

/// The console's input buffer (CONIN$, whatever stdin is); `None` without a
/// console.
#[cfg(windows)]
fn console_input() -> Option<crossterm_winapi::ConsoleMode> {
    crossterm_winapi::Handle::current_in_handle()
        .ok()
        .map(crossterm_winapi::ConsoleMode::from)
}

#[cfg(all(not(unix), not(windows)))]
struct SavedInputMode;

#[cfg(all(not(unix), not(windows)))]
impl SavedInputMode {
    fn save() -> Self {
        Self
    }

    fn restore(&self) {}
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
            continue_chat: Some(ContinueChat::Chat("chat-42".to_string())),
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
                "--continue",
                "chat-42",
            ]))
        );
    }

    #[test]
    fn continue_is_taken_from_the_options_with_its_chat_id() {
        assert_eq!(
            extract_continue_flag(strings(&["--continue", "chat-42", "--model", "opus"])),
            (
                strings(&["--model", "opus"]),
                Some(ContinueChat::Chat("chat-42".to_string()))
            )
        );
        assert_eq!(
            extract_continue_flag(strings(&["--continue", "--model", "opus"])),
            (strings(&["--model", "opus"]), Some(ContinueChat::Latest))
        );
        assert_eq!(
            extract_continue_flag(strings(&["--continue=chat-7"])),
            (Vec::new(), Some(ContinueChat::Chat("chat-7".to_string())))
        );
        assert_eq!(
            extract_continue_flag(strings(&["-p", "--continue"])),
            (strings(&["-p", "--continue"]), None)
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
            continue_chat: None,
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

    #[test]
    fn the_exit_message_the_ui_left_is_printed_once_and_removed() {
        let file = temp_file("exit");
        let mut out = Vec::new();
        print_exit_message(&file, &mut out);
        assert_eq!(out, b"", "nothing left, nothing printed");

        let hint = "\nTo continue this session later, run:\nneuron --continue c1\n";
        std::fs::write(&file, hint).unwrap();
        print_exit_message(&file, &mut out);
        assert_eq!(String::from_utf8(out).unwrap(), hint);
        assert!(!file.exists(), "the file is removed");
        assert_ne!(temp_file("exit"), file, "each launch gets its own file");
    }

    #[test]
    fn the_terminal_is_reset_only_after_a_ui_that_died_in_its_modes() {
        let file = temp_file("terminal");
        // The UI never set the terminal up (it failed to start): leave it be.
        assert!(!ui_left_terminal_armed(&file));

        std::fs::write(&file, "armed").unwrap();
        assert!(
            ui_left_terminal_armed(&file),
            "it died with the terminal in its modes"
        );
        assert!(!file.exists(), "the file is removed");

        std::fs::write(&file, "disarmed").unwrap();
        assert!(
            !ui_left_terminal_armed(&file),
            "it reset the terminal itself"
        );
        assert!(!file.exists());
    }

    #[cfg(windows)]
    #[test]
    fn the_saved_input_mode_comes_back_after_the_ui_changed_it() {
        // Only where the tests have a console.
        let Some(console) = console_input() else {
            return;
        };
        let Ok(before) = console.mode() else {
            return;
        };
        let saved = SavedInputMode::save();
        // What the UI's raw mode does: virtual-terminal input on.
        let _ = console.set_mode(before ^ 0x200);
        saved.restore();
        assert_eq!(console.mode().ok(), Some(before));
    }
}
