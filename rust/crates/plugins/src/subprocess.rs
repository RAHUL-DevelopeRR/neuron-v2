//! Child-process helpers shared by every launcher that runs user or model
//! supplied commands (bash/`PowerShell` tools, hooks, plugin tools, MCP
//! servers).
//!
//! The helpers cover three concerns:
//! - credential scrubbing: Zero-X session tokens, provider API keys and other
//!   secret-looking variables must never leak into tool subprocesses, whose
//!   output reaches the model ([`scrub_credential_env`]);
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

/// Environment variables that always carry Zero-X or upstream-provider
/// credentials.
pub const CREDENTIAL_ENV_VARS: &[&str] = &[
    "NEURON_TOKEN",
    "NEURON_SESSION_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "OPENAI_API_KEY",
    "AZURE_OPENAI_API_KEY",
    "XAI_API_KEY",
    "GROK_API_KEY",
    "DASHSCOPE_API_KEY",
    "OPENROUTER_API_KEY",
];

/// Name suffixes that mark any other variable as a secret (`GITHUB_TOKEN`,
/// `STRIPE_SECRET`, `MISTRAL_API_KEY`); matched after a `_` or as the whole
/// name, ignoring case.
pub const CREDENTIAL_ENV_SUFFIXES: &[&str] = &["API_KEY", "TOKEN", "SECRET"];

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

/// Windows `CREATE_NO_WINDOW` creation flag: the child gets its own console,
/// which has no window. Background children (MCP servers, hooks) then cannot
/// retitle the user's terminal (`cmd.exe` and `npm` set the console title) or
/// get its Ctrl+C; their stdio pipes work as before.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Returns `true` when an environment variable holds a credential: a known
/// Zero-X or provider variable, a secret-looking name, or a Zero-X value.
#[must_use]
pub fn is_credential_env(key: &OsStr, value: &OsStr) -> bool {
    let key = key.to_string_lossy().to_ascii_uppercase();
    if CREDENTIAL_ENV_VARS.contains(&key.as_str())
        || CREDENTIAL_ENV_SUFFIXES.iter().any(|suffix| {
            key == *suffix
                || key
                    .strip_suffix(suffix)
                    .is_some_and(|stem| stem.ends_with('_'))
        })
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

/// Removes inherited credentials from the child's environment.
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
/// this is safe to call for children that were not isolated. It never waits
/// longer than about [`TREE_KILL_WAIT`] (see [`kill_tree_of`]).
pub fn kill_process_tree(child: &mut Child) {
    let _ = kill_tree_of(child.id());
    let _ = child.kill();
}

/// How long [`kill_tree_of`] waits for the tree to be killed. `taskkill` hangs
/// while the WMI service is stuck, and an interrupt or a shutdown must not hang
/// with it.
pub const TREE_KILL_WAIT: Duration = Duration::from_secs(4);

/// When `taskkill` has not finished by then, the WMI-free kill
/// ([`fallback_tree_kill_command`]) starts beside it; at once after `taskkill`
/// was given up on once ([`TASKKILL_HUNG`]).
#[cfg(windows)]
const TREE_KILL_FALLBACK_AFTER: Duration = Duration::from_millis(300);

/// A `taskkill` did not finish in time: WMI is stuck, and the next kills start
/// their fallback at once.
#[cfg(windows)]
static TASKKILL_HUNG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Kills process `pid` and every process it started: `taskkill /T /F /PID` on
/// Windows, a `KILL` to its process group on Unix. On Windows, a `taskkill` that
/// has not finished after [`TREE_KILL_FALLBACK_AFTER`] gets company: a kill that
/// reads the tree from the `Process` performance counters instead of WMI. Waits at
/// most [`TREE_KILL_WAIT`]; killers still running then are killed themselves, and
/// the caller falls back to killing its own child. Returns whether a killer
/// finished in time.
#[must_use]
pub fn kill_tree_of(pid: u32) -> bool {
    let Some(mut killer) = spawn_killer(tree_kill_command(pid)) else {
        return false;
    };
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut fallback: Option<Child> = None;
    let started = Instant::now();
    let finished = loop {
        if matches!(killer.try_wait(), Ok(Some(_)) | Err(_)) {
            break true;
        }
        if let Some(fallback) = fallback.as_mut() {
            if matches!(fallback.try_wait(), Ok(Some(_)) | Err(_)) {
                break true;
            }
        }
        if started.elapsed() >= TREE_KILL_WAIT {
            break false;
        }
        #[cfg(windows)]
        if fallback.is_none()
            && (started.elapsed() >= TREE_KILL_FALLBACK_AFTER
                || TASKKILL_HUNG.load(std::sync::atomic::Ordering::Relaxed))
        {
            fallback = spawn_killer(fallback_tree_kill_command(pid));
        }
        thread::sleep(POLL_INTERVAL);
    };
    #[cfg(windows)]
    if matches!(killer.try_wait(), Ok(None)) {
        TASKKILL_HUNG.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    for mut stuck in std::iter::once(killer).chain(fallback) {
        if matches!(stuck.try_wait(), Ok(None)) {
            let _ = stuck.kill();
            // Reaped when it goes at once; never waited for beyond that.
            let reap_by = Instant::now() + Duration::from_millis(200);
            while matches!(stuck.try_wait(), Ok(None)) && Instant::now() < reap_by {
                thread::sleep(POLL_INTERVAL);
            }
        }
    }
    finished
}

/// Starts a tree killer without a console window or stdio.
fn spawn_killer(mut command: Command) -> Option<Child> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.spawn().ok()
}

/// Kills the tree of `pid` without `taskkill`, which goes through WMI: Windows
/// `PowerShell` reads every process's parent from the `Process` performance counters,
/// then stops `pid` and the processes started under it (and not before it, so a
/// reused id is left alone). `pid` goes first: a shell whose command died would run
/// its next one.
#[cfg(windows)]
fn fallback_tree_kill_command(pid: u32) -> Command {
    let script = format!(
        r"$ErrorActionPreference = 'SilentlyContinue'
$root = {pid}
$data = (New-Object System.Diagnostics.PerformanceCounterCategory('Process')).ReadCategory()
$ids = $data['ID Process']
$parents = $data['Creating Process ID']
$children = @{{}}
foreach ($name in $ids.Keys) {{
    $id = [int]$ids[$name].RawValue
    $parent = [int]$parents[$name].RawValue
    if ($id -ne 0 -and $id -ne $parent) {{
        if (-not $children.ContainsKey($parent)) {{ $children[$parent] = @() }}
        $children[$parent] += $id
    }}
}}
$since = (Get-Process -Id $root).StartTime
$level = @($root)
$tree = @()
while ($level.Count -gt 0) {{
    $next = @()
    foreach ($p in $level) {{
        foreach ($c in $children[$p]) {{
            $process = Get-Process -Id $c
            if ($process -and $since -and $process.StartTime -ge $since) {{
                $tree += $c
                $next += $c
            }}
        }}
    }}
    $level = $next
}}
# The root first: a shell whose child died would go on to its next command.
Stop-Process -Id $root -Force
foreach ($id in $tree) {{ Stop-Process -Id $id -Force }}
"
    );
    let system_root =
        std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from(r"C:\Windows"));
    let mut command = Command::new(
        std::path::PathBuf::from(system_root)
            .join(r"System32\WindowsPowerShell\v1.0\powershell.exe"),
    );
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-EncodedCommand",
        &utf16le_base64(&script),
    ]);
    command
}

/// `PowerShell`'s `-EncodedCommand` form of `script`: UTF-16LE, base64.
#[cfg(windows)]
fn utf16le_base64(script: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = chunk.iter().enumerate().fold(0_u32, |acc, (index, byte)| {
            acc | (u32::from(*byte) << (16 - 8 * index))
        });
        for position in 0..4 {
            if position <= chunk.len() {
                let index = (triple >> (18 - 6 * position)) & 0x3f;
                encoded.push(char::from(ALPHABET[index as usize]));
            } else {
                encoded.push('=');
            }
        }
    }
    encoded
}

#[cfg(test)]
thread_local! {
    /// A test's stand-in for `taskkill` / `kill` on its own thread: program and arguments.
    static TREE_KILL_OVERRIDE: std::cell::RefCell<Option<(String, Vec<String>)>> =
        const { std::cell::RefCell::new(None) };
}

fn tree_kill_command(pid: u32) -> Command {
    #[cfg(test)]
    if let Some((program, args)) = TREE_KILL_OVERRIDE.with(|stand_in| stand_in.borrow().clone()) {
        let mut command = Command::new(program);
        command.args(args);
        return command;
    }
    system_tree_kill_command(pid)
}

#[cfg(windows)]
fn system_tree_kill_command(pid: u32) -> Command {
    let taskkill = std::env::var_os("SystemRoot").map_or_else(
        || std::path::PathBuf::from("taskkill.exe"),
        |root| {
            std::path::PathBuf::from(root)
                .join("System32")
                .join("taskkill.exe")
        },
    );
    let mut command = Command::new(taskkill);
    command.args(["/T", "/F", "/PID", &pid.to_string()]);
    command
}

#[cfg(not(windows))]
fn system_tree_kill_command(pid: u32) -> Command {
    let kill = if std::path::Path::new("/bin/kill").exists() {
        "/bin/kill"
    } else {
        "kill"
    };
    let mut command = Command::new(kill);
    command.args(["-KILL", "--", &format!("-{pid}")]);
    command
}

/// Commands started in the background (`run_in_background`): nothing waits for them,
/// and they must not outlive the session.
static BACKGROUND_CHILDREN: Mutex<Vec<Child>> = Mutex::new(Vec::new());

/// Keeps `child`, a command started in the background, for
/// [`kill_background_children`]. Spawn it with [`isolate_process_group`] so that its
/// whole tree can be stopped.
pub fn track_background_child(child: Child) {
    let mut children = BACKGROUND_CHILDREN
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    // The ones that ended are reaped.
    children.retain_mut(|child| matches!(child.try_wait(), Ok(None)));
    children.push(child);
}

/// Kills every background command still running, with every process it started (the
/// session is ending). Returns how many were still running.
#[must_use = "how many were still running"]
pub fn kill_background_children() -> usize {
    let children = std::mem::take(
        &mut *BACKGROUND_CHILDREN
            .lock()
            .unwrap_or_else(PoisonError::into_inner),
    );
    let mut killed = 0;
    for mut child in children {
        if matches!(child.try_wait(), Ok(None)) {
            kill_process_tree(&mut child);
            let _ = child.wait();
            killed += 1;
        }
    }
    killed
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
    fn detects_provider_keys_and_secret_looking_names() {
        for key in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "OPENAI_API_KEY",
            "AZURE_OPENAI_API_KEY",
            "XAI_API_KEY",
            "GROK_API_KEY",
            "DASHSCOPE_API_KEY",
            "OPENROUTER_API_KEY",
            "MISTRAL_API_KEY",
            "GITHUB_TOKEN",
            "stripe_secret",
            "API_KEY",
        ] {
            assert!(
                is_credential_env(OsStr::new(key), OsStr::new("plain-value")),
                "{key}"
            );
        }
        for key in [
            "PATH",
            "HOME",
            "TOKENIZERS_PARALLELISM",
            "SECRETARY",
            "KEYBOARD_LAYOUT",
            "MYTOKEN",
        ] {
            assert!(
                !is_credential_env(OsStr::new(key), OsStr::new("plain-value")),
                "{key}"
            );
        }
    }

    #[test]
    fn scrubbed_children_do_not_inherit_credentials_but_keep_other_vars() {
        std::env::set_var("PLUGINS_SCRUB_TEST_SECRET", "nzx_scrub_test_secret");
        std::env::set_var("PLUGINS_SCRUB_TEST_API_KEY", "sk-plain-provider-key");
        std::env::set_var("PLUGINS_SCRUB_TEST_KEEP", "keep-me");
        assert!(credential_env_keys()
            .iter()
            .any(|key| key == "PLUGINS_SCRUB_TEST_SECRET"));

        let script = if cfg!(windows) {
            "echo secret=%PLUGINS_SCRUB_TEST_SECRET% key=%PLUGINS_SCRUB_TEST_API_KEY% keep=%PLUGINS_SCRUB_TEST_KEEP%"
        } else {
            "echo secret=${PLUGINS_SCRUB_TEST_SECRET:-unset} key=${PLUGINS_SCRUB_TEST_API_KEY:-unset} keep=$PLUGINS_SCRUB_TEST_KEEP"
        };
        let mut command = shell(script);
        scrub_credential_env(&mut command);
        let output = run_bounded(&mut command, None, Some(Duration::from_secs(20)), &|| false)
            .expect("child should run");
        std::env::remove_var("PLUGINS_SCRUB_TEST_SECRET");
        std::env::remove_var("PLUGINS_SCRUB_TEST_API_KEY");
        std::env::remove_var("PLUGINS_SCRUB_TEST_KEEP");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.contains("nzx_scrub_test_secret"), "{stdout}");
        assert!(!stdout.contains("sk-plain-provider-key"), "{stdout}");
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

    /// A process that sleeps for about 30 s without starting any other.
    fn lone_sleeper() -> (String, Vec<String>) {
        if cfg!(windows) {
            (
                "ping".to_string(),
                ["-n", "30", "127.0.0.1"].map(String::from).to_vec(),
            )
        } else {
            ("sleep".to_string(), vec!["30".to_string()])
        }
    }

    /// `taskkill` hangs while the WMI service is stuck: killing a tree gives it
    /// [`super::TREE_KILL_WAIT`], then kills it and the child itself, so an interrupt or
    /// a shutdown never hangs with it.
    #[test]
    fn a_hanging_tree_killer_is_given_up_on_and_the_child_still_dies() {
        let (program, args) = lone_sleeper();
        let mut child = Command::new(&program)
            .args(&args)
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("the child starts");
        super::TREE_KILL_OVERRIDE.with(|stand_in| *stand_in.borrow_mut() = Some(lone_sleeper()));

        let started = Instant::now();
        super::kill_process_tree(&mut child);
        let took = started.elapsed();
        super::TREE_KILL_OVERRIDE.with(|stand_in| *stand_in.borrow_mut() = None);

        assert!(
            took < super::TREE_KILL_WAIT + Duration::from_secs(3),
            "took {took:?}"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().expect("try_wait").is_none() {
            assert!(Instant::now() < deadline, "the child still runs");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[cfg(windows)]
    #[test]
    fn powershell_encoded_commands_are_utf16le_base64() {
        assert_eq!(super::utf16le_base64(""), "");
        assert_eq!(super::utf16le_base64("dir"), "ZABpAHIA");
        assert_eq!(super::utf16le_base64("f"), "ZgA=");
        assert_eq!(super::utf16le_base64("fo"), "ZgBvAA==");
    }

    /// Whether process `pid` runs, asked without WMI (`tasklist` hangs with it).
    #[cfg(windows)]
    fn is_running(pid: u32) -> bool {
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!(
                    "if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ 'running' }}"
                ),
            ])
            .output()
            .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains("running"))
    }

    /// With `taskkill` hanging (the WMI service stuck), a shell's grandchild still dies:
    /// the tree is read from the performance counters instead.
    #[cfg(windows)]
    #[test]
    fn a_tree_is_killed_without_taskkill_when_it_hangs() {
        let pid_file = std::env::temp_dir().join(format!(
            "neuron-tree-kill-{}-{:?}.pid",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&pid_file);
        let mut shell = super::windows_cmd_command(&format!(
            "powershell -NoProfile -NonInteractive -Command \"Set-Content -LiteralPath '{}' -Value $PID; Start-Sleep -Seconds 30\"",
            pid_file.display()
        ));
        let mut shell = shell
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("the shell starts");
        let deadline = Instant::now() + Duration::from_secs(60);
        let grandchild = loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|text| text.trim().parse::<u32>().ok())
            {
                break pid;
            }
            assert!(Instant::now() < deadline, "the grandchild never started");
            std::thread::sleep(Duration::from_millis(50));
        };
        super::TREE_KILL_OVERRIDE.with(|stand_in| *stand_in.borrow_mut() = Some(lone_sleeper()));

        let started = Instant::now();
        let finished = super::kill_tree_of(shell.id());
        let took = started.elapsed();
        let _ = shell.kill();
        super::TREE_KILL_OVERRIDE.with(|stand_in| *stand_in.borrow_mut() = None);
        let _ = std::fs::remove_file(&pid_file);

        let deadline = Instant::now() + Duration::from_secs(10);
        while is_running(grandchild) {
            assert!(
                Instant::now() < deadline,
                "grandchild {grandchild} still runs (a killer finished: {finished}, after {took:?})"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = shell.wait();
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
