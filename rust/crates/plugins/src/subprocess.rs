//! Child-process helpers shared by every launcher that runs user or model
//! supplied commands (bash/`PowerShell` tools, hooks, plugin tools, MCP
//! servers).
//!
//! The helpers cover three concerns:
//! - credential scrubbing: Zero-X session tokens and API keys must never leak
//!   into tool subprocesses ([`scrub_credential_env`]);
//! - bounded execution: stdout and stderr are drained concurrently so a chatty
//!   child cannot dead-lock on a full pipe, and a timeout or cancellation
//!   request terminates the child ([`run_bounded`]);
//! - whole-tree termination: shells spawn grandchildren, so killing only the
//!   direct child leaves orphans holding the output pipes open
//!   ([`kill_process_tree`]).

use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Environment variables that always carry Zero-X credentials.
pub const CREDENTIAL_ENV_VARS: &[&str] = &["NEURON_TOKEN", "NEURON_SESSION_TOKEN"];

/// Value prefixes of Zero-X credentials (`ses_` CLI sessions, `nzx_` API keys).
pub const CREDENTIAL_VALUE_PREFIXES: &[&str] = &["ses_", "nzx_"];

/// Default timeout for hook commands.
pub const DEFAULT_HOOK_TIMEOUT: Duration = Duration::from_secs(60);

/// Default timeout for plugin tool commands.
pub const DEFAULT_PLUGIN_TOOL_TIMEOUT: Duration = Duration::from_secs(120);

/// Maximum bytes captured per output stream; the remainder is drained and dropped.
pub const MAX_CAPTURED_BYTES: usize = 8 * 1024 * 1024;

const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How long to keep draining pipes after the child exited. Grandchildren that
/// were detached from the process tree can keep a pipe open forever; output
/// written before the child exited is already buffered and arrives well
/// within this window.
const PIPE_DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Builds `cmd.exe /D /S /C "<command>"` with the command line passed
/// verbatim.
///
/// Rust's default argument quoting follows the MSVC runtime rules (`\"`
/// escapes), which `cmd.exe` does not understand, so commands containing
/// quotes would be mangled. With `/S`, `cmd.exe` strips exactly the outer
/// quote pair and runs the rest unchanged; `/D` skips registry `AutoRun`
/// commands so tool runs are reproducible.
#[cfg(windows)]
#[must_use]
pub fn windows_cmd_command(command: &str) -> Command {
    use std::os::windows::process::CommandExt;

    let program = std::env::var_os("ComSpec").unwrap_or_else(|| OsString::from("cmd.exe"));
    let mut prepared = Command::new(program);
    prepared
        .args(["/D", "/S", "/C"])
        .raw_arg(format!("\"{command}\""));
    prepared
}

/// Returns `true` when an environment variable holds a Zero-X credential.
#[must_use]
pub fn is_credential_env(key: &OsStr, value: &OsStr) -> bool {
    let key = key.to_string_lossy();
    if CREDENTIAL_ENV_VARS
        .iter()
        .any(|name| key.eq_ignore_ascii_case(name))
    {
        return true;
    }
    let value = value.to_string_lossy();
    CREDENTIAL_VALUE_PREFIXES
        .iter()
        .any(|prefix| value.starts_with(prefix))
}

/// Names of inherited environment variables that must not reach children.
#[must_use]
pub fn credential_env_keys() -> Vec<OsString> {
    std::env::vars_os()
        .filter(|(key, value)| is_credential_env(key, value))
        .map(|(key, _)| key)
        .collect()
}

/// Removes inherited Zero-X credentials from the child's environment.
///
/// Call this before applying explicitly configured variables so that a value
/// the user deliberately configured for a child (for example in an MCP server
/// `env` block) still wins.
pub fn scrub_credential_env(command: &mut Command) -> &mut Command {
    for key in credential_env_keys() {
        command.env_remove(key);
    }
    command
}

/// Puts the child in its own process group (Unix) so the whole tree can be
/// signalled at once. A no-op on Windows, where `taskkill /T` walks the tree.
pub fn isolate_process_group(command: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
}

/// Terminates `child` and every process it spawned.
///
/// Windows uses `taskkill /T /F /PID`; Unix signals the process group created
/// by [`isolate_process_group`]. The direct child is always killed as well, so
/// this is safe to call for children that were not isolated.
pub fn kill_process_tree(child: &mut Child) {
    let pid = child.id().to_string();
    #[cfg(windows)]
    {
        let taskkill = std::env::var_os("SystemRoot").map_or_else(
            || std::path::PathBuf::from("taskkill.exe"),
            |root| {
                std::path::PathBuf::from(root)
                    .join("System32")
                    .join("taskkill.exe")
            },
        );
        let _ = Command::new(taskkill)
            .args(["/T", "/F", "/PID", &pid])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        let kill = if std::path::Path::new("/bin/kill").exists() {
            "/bin/kill"
        } else {
            "kill"
        };
        let _ = Command::new(kill)
            .args(["-KILL", "--", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

/// Why a bounded run stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Termination {
    /// The child exited on its own.
    Exited,
    /// The timeout elapsed and the process tree was killed.
    TimedOut,
    /// Cancellation was requested and the process tree was killed.
    Cancelled,
}

/// Captured result of [`run_bounded`].
#[derive(Debug)]
pub struct BoundedOutput {
    /// Exit status, when the child could be reaped.
    pub status: Option<ExitStatus>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub termination: Termination,
}

impl BoundedOutput {
    #[must_use]
    pub fn timed_out(&self) -> bool {
        self.termination == Termination::TimedOut
    }

    #[must_use]
    pub fn cancelled(&self) -> bool {
        self.termination == Termination::Cancelled
    }

    #[must_use]
    pub fn success(&self) -> bool {
        self.termination == Termination::Exited && self.status.is_some_and(|s| s.success())
    }

    #[must_use]
    pub fn exit_code(&self) -> Option<i32> {
        self.status.and_then(|status| status.code())
    }
}

struct PipeReader {
    buffer: Arc<Mutex<Vec<u8>>>,
    handle: Option<JoinHandle<()>>,
}

impl PipeReader {
    fn spawn<R: Read + Send + 'static>(pipe: Option<R>) -> Self {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let handle = pipe.map(|mut pipe| {
            let sink = Arc::clone(&buffer);
            thread::spawn(move || {
                let mut chunk = [0_u8; 8192];
                loop {
                    match pipe.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(read) => {
                            let mut sink = sink.lock().unwrap_or_else(PoisonError::into_inner);
                            let room = MAX_CAPTURED_BYTES.saturating_sub(sink.len());
                            sink.extend_from_slice(&chunk[..read.min(room)]);
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
            })
        });
        Self { buffer, handle }
    }

    fn is_finished(&self) -> bool {
        self.handle.as_ref().is_none_or(JoinHandle::is_finished)
    }

    fn into_bytes(mut self) -> Vec<u8> {
        if let Some(handle) = self.handle.take() {
            if handle.is_finished() {
                let _ = handle.join();
            }
            // An unfinished reader is detached: a process outside the killed
            // tree still holds the pipe, and blocking on it would hang.
        }
        let mut buffer = self.buffer.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut *buffer)
    }
}

/// Runs `command` to completion with concurrent stdout/stderr draining, an
/// optional timeout and cooperative cancellation.
///
/// stdin is `/dev/null` unless `stdin` bytes are supplied, in which case they
/// are written from a helper thread (a child that exits without reading them
/// is not an error). On timeout or cancellation the whole process tree is
/// killed and the output captured so far is returned.
pub fn run_bounded(
    command: &mut Command,
    stdin: Option<&[u8]>,
    timeout: Option<Duration>,
    is_cancelled: &dyn Fn() -> bool,
) -> io::Result<BoundedOutput> {
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate_process_group(command);
    let mut child = command.spawn()?;

    let writer = match (stdin, child.stdin.take()) {
        (Some(bytes), Some(mut pipe)) => {
            let bytes = bytes.to_vec();
            Some(thread::spawn(move || {
                // BrokenPipe means the child exited without reading stdin.
                let _ = pipe.write_all(&bytes);
            }))
        }
        _ => None,
    };
    let stdout = PipeReader::spawn(child.stdout.take());
    let stderr = PipeReader::spawn(child.stderr.take());

    let started = Instant::now();
    let (termination, status) = loop {
        if let Some(status) = child.try_wait()? {
            break (Termination::Exited, Some(status));
        }
        if is_cancelled() {
            kill_process_tree(&mut child);
            break (Termination::Cancelled, child.wait().ok());
        }
        if timeout.is_some_and(|limit| started.elapsed() >= limit) {
            kill_process_tree(&mut child);
            break (Termination::TimedOut, child.wait().ok());
        }
        thread::sleep(POLL_INTERVAL);
    };

    let drain_started = Instant::now();
    while !(stdout.is_finished() && stderr.is_finished())
        && drain_started.elapsed() < PIPE_DRAIN_GRACE
    {
        thread::sleep(POLL_INTERVAL);
    }
    if let Some(writer) = writer {
        if writer.is_finished() {
            let _ = writer.join();
        }
    }

    Ok(BoundedOutput {
        status,
        stdout: stdout.into_bytes(),
        stderr: stderr.into_bytes(),
        termination,
    })
}

/// Formats the standard message appended when a child was stopped early.
#[must_use]
pub fn termination_message(termination: Termination, timeout: Option<Duration>) -> Option<String> {
    match termination {
        Termination::Exited => None,
        Termination::TimedOut => Some(format!(
            "Command exceeded timeout of {} ms; the process tree was terminated",
            timeout.map_or(0, |limit| limit.as_millis())
        )),
        Termination::Cancelled => {
            Some("Command was cancelled; the process tree was terminated".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        credential_env_keys, is_credential_env, run_bounded, scrub_credential_env,
        termination_message, Termination,
    };
    use std::ffi::OsStr;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn shell(script: &str) -> Command {
        if cfg!(windows) {
            let mut command = Command::new("cmd.exe");
            command.args(["/D", "/C", script]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", script]);
            command
        }
    }

    fn long_sleep() -> Command {
        if cfg!(windows) {
            // `ping` is always present and sleeps ~1 s per echo request.
            let mut command = Command::new("cmd.exe");
            command.args(["/D", "/C", "ping -n 30 127.0.0.1 >NUL"]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", "sleep 30; echo late"]);
            command
        }
    }

    #[test]
    fn detects_credentials_by_name_and_value_prefix() {
        assert!(is_credential_env(
            OsStr::new("NEURON_TOKEN"),
            OsStr::new("anything")
        ));
        assert!(is_credential_env(
            OsStr::new("neuron_session_token"),
            OsStr::new("x")
        ));
        assert!(is_credential_env(
            OsStr::new("MY_GATEWAY"),
            OsStr::new("ses_abc123")
        ));
        assert!(is_credential_env(
            OsStr::new("CI_KEY"),
            OsStr::new("nzx_live_key")
        ));
        assert!(!is_credential_env(
            OsStr::new("PATH"),
            OsStr::new("/usr/bin")
        ));
        assert!(!is_credential_env(
            OsStr::new("SESSION_NAME"),
            OsStr::new("my-ses_name")
        ));
    }

    #[test]
    fn scrubbed_children_do_not_inherit_credentials_but_keep_other_vars() {
        std::env::set_var("PLUGINS_SCRUB_TEST_SECRET", "nzx_scrub_test_secret");
        std::env::set_var("PLUGINS_SCRUB_TEST_KEEP", "keep-me");
        assert!(credential_env_keys()
            .iter()
            .any(|key| key == "PLUGINS_SCRUB_TEST_SECRET"));

        let script = if cfg!(windows) {
            "echo secret=%PLUGINS_SCRUB_TEST_SECRET% keep=%PLUGINS_SCRUB_TEST_KEEP%"
        } else {
            "echo secret=${PLUGINS_SCRUB_TEST_SECRET:-unset} keep=$PLUGINS_SCRUB_TEST_KEEP"
        };
        let mut command = shell(script);
        scrub_credential_env(&mut command);
        let output = run_bounded(&mut command, None, Some(Duration::from_secs(20)), &|| false)
            .expect("child should run");
        std::env::remove_var("PLUGINS_SCRUB_TEST_SECRET");
        std::env::remove_var("PLUGINS_SCRUB_TEST_KEEP");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.contains("nzx_scrub_test_secret"), "{stdout}");
        assert!(stdout.contains("keep=keep-me"), "{stdout}");
    }

    #[test]
    fn captures_large_output_on_both_streams_without_deadlock() {
        let script = if cfg!(windows) {
            "for /L %i in (1,1,3000) do @(echo stdout-line-%i & echo stderr-line-%i 1>&2)"
        } else {
            "i=0; while [ $i -lt 3000 ]; do echo stdout-line-$i; echo stderr-line-$i >&2; i=$((i+1)); done"
        };
        let mut command = shell(script);
        let output = run_bounded(&mut command, None, Some(Duration::from_secs(60)), &|| false)
            .expect("child should run");
        assert_eq!(output.termination, Termination::Exited);
        assert!(output.stdout.len() > 30_000);
        assert!(output.stderr.len() > 30_000);
    }

    #[test]
    fn timeout_kills_the_process_tree_promptly() {
        let started = Instant::now();
        let mut command = long_sleep();
        let output = run_bounded(
            &mut command,
            None,
            Some(Duration::from_millis(300)),
            &|| false,
        )
        .expect("child should spawn");
        assert_eq!(output.termination, Termination::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "timed-out child must be killed, took {:?}",
            started.elapsed()
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("late"));
        assert!(
            termination_message(output.termination, Some(Duration::from_millis(300)))
                .expect("message")
                .contains("300 ms")
        );
    }

    #[test]
    fn cancellation_kills_the_process_tree() {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let trigger = Arc::clone(&stop_flag);
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            trigger.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let mut command = long_sleep();
        let output = run_bounded(&mut command, None, None, &|| {
            stop_flag.load(Ordering::SeqCst)
        })
        .expect("child should spawn");
        canceller.join().expect("canceller thread");
        assert_eq!(output.termination, Termination::Cancelled);
        assert!(output.cancelled());
        assert!(started.elapsed() < Duration::from_secs(15));
    }

    #[test]
    fn feeds_stdin_to_the_child() {
        let mut command = if cfg!(windows) {
            let mut command = Command::new("cmd.exe");
            command.args(["/D", "/C", "more"]);
            command
        } else {
            Command::new("cat")
        };
        let output = run_bounded(
            &mut command,
            Some(b"payload-from-parent\n"),
            Some(Duration::from_secs(20)),
            &|| false,
        )
        .expect("child should run");
        assert!(output.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("payload-from-parent"));
    }
}
