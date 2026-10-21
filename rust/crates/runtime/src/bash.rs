use std::env;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use plugins::subprocess::{self, BoundedOutput, Termination};
use serde::{Deserialize, Serialize};

use crate::cancel::CancellationToken;
use crate::sandbox::{
    build_linux_sandbox_command, resolve_sandbox_status_for_request, FilesystemIsolationMode,
    SandboxConfig, SandboxDirs, SandboxStatus,
};
use crate::ConfigLoader;

/// Timeout applied when the caller does not request one (2 minutes).
pub const DEFAULT_BASH_TIMEOUT_MS: u64 = 120_000;
/// Upper bound for caller-requested timeouts (10 minutes).
pub const MAX_BASH_TIMEOUT_MS: u64 = 600_000;

/// Human-readable description of the shell that runs `bash` tool commands on
/// this platform, used by the tool description and the system prompt.
#[must_use]
pub fn bash_tool_shell_description() -> &'static str {
    if cfg!(windows) {
        "Windows cmd.exe (`cmd.exe /D /S /C`): use cmd syntax (`dir`, `type`, `set`, `%VAR%`); POSIX tools work only when they are on PATH. Use the PowerShell tool for PowerShell syntax."
    } else {
        "POSIX sh (`sh -lc`)"
    }
}

/// Resolves the effective timeout: the default when unset, clamped to
/// `1..=MAX_BASH_TIMEOUT_MS`.
#[must_use]
pub fn effective_bash_timeout_ms(requested: Option<u64>) -> u64 {
    requested
        .unwrap_or(DEFAULT_BASH_TIMEOUT_MS)
        .clamp(1, MAX_BASH_TIMEOUT_MS)
}

/// Input schema for the built-in bash execution tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BashCommandInput {
    pub command: String,
    pub timeout: Option<u64>,
    pub description: Option<String>,
    #[serde(rename = "run_in_background")]
    pub run_in_background: Option<bool>,
    #[serde(rename = "dangerouslyDisableSandbox")]
    pub dangerously_disable_sandbox: Option<bool>,
    #[serde(rename = "namespaceRestrictions")]
    pub namespace_restrictions: Option<bool>,
    #[serde(rename = "isolateNetwork")]
    pub isolate_network: Option<bool>,
    #[serde(rename = "filesystemMode")]
    pub filesystem_mode: Option<FilesystemIsolationMode>,
    #[serde(rename = "allowedMounts")]
    pub allowed_mounts: Option<Vec<String>>,
}

/// Output returned from a bash tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BashCommandOutput {
    pub stdout: String,
    pub stderr: String,
    #[serde(rename = "rawOutputPath")]
    pub raw_output_path: Option<String>,
    pub interrupted: bool,
    #[serde(rename = "isImage")]
    pub is_image: Option<bool>,
    #[serde(rename = "backgroundTaskId")]
    pub background_task_id: Option<String>,
    #[serde(rename = "backgroundedByUser")]
    pub backgrounded_by_user: Option<bool>,
    #[serde(rename = "assistantAutoBackgrounded")]
    pub assistant_auto_backgrounded: Option<bool>,
    #[serde(rename = "dangerouslyDisableSandbox")]
    pub dangerously_disable_sandbox: Option<bool>,
    /// Set only when the command did not succeed: `exit_code:<n>`,
    /// `signal:<n>` / `killed`, `timeout`, `cancelled`, or a
    /// `preflight_blocked:*` reason. See [`Self::failed`].
    #[serde(rename = "returnCodeInterpretation")]
    pub return_code_interpretation: Option<String>,
    #[serde(rename = "noOutputExpected")]
    pub no_output_expected: Option<bool>,
    #[serde(rename = "structuredContent")]
    pub structured_content: Option<Vec<serde_json::Value>>,
    #[serde(rename = "persistedOutputPath")]
    pub persisted_output_path: Option<String>,
    #[serde(rename = "persistedOutputSize")]
    pub persisted_output_size: Option<u64>,
    #[serde(rename = "sandboxStatus")]
    pub sandbox_status: Option<SandboxStatus>,
}

impl BashCommandOutput {
    /// Converts a bounded subprocess run into the tool output envelope.
    ///
    /// Output captured before a timeout or cancellation is kept and the
    /// termination reason is appended to stderr, so the model can see how far
    /// the command got.
    #[must_use]
    pub fn from_bounded(output: &BoundedOutput, timeout: Option<Duration>) -> Self {
        let stdout = truncate_output(&String::from_utf8_lossy(&output.stdout));
        let mut stderr = truncate_output(&String::from_utf8_lossy(&output.stderr));
        if let Some(message) = subprocess::termination_message(output.termination, timeout) {
            if !stderr.trim().is_empty() {
                stderr = format!("{}\n", stderr.trim_end());
            }
            stderr.push_str(&message);
        }
        let return_code_interpretation = match output.termination {
            Termination::TimedOut => Some(String::from("timeout")),
            Termination::Cancelled => Some(String::from("cancelled")),
            Termination::Exited => match output.status {
                Some(status) if status.success() => None,
                Some(status) => Some(status.code().map_or_else(
                    || killed_interpretation(status),
                    |code| format!("exit_code:{code}"),
                )),
                None => Some(String::from("killed")),
            },
        };
        let no_output_expected = Some(stdout.trim().is_empty() && stderr.trim().is_empty());
        Self {
            stdout,
            stderr,
            raw_output_path: None,
            interrupted: output.termination != Termination::Exited,
            is_image: None,
            background_task_id: None,
            backgrounded_by_user: None,
            assistant_auto_backgrounded: None,
            dangerously_disable_sandbox: None,
            return_code_interpretation,
            no_output_expected,
            structured_content: None,
            persisted_output_path: None,
            persisted_output_size: None,
            sandbox_status: None,
        }
    }

    /// Whether the command failed: it exited non-zero, was killed (timeout,
    /// cancellation, signal) or was blocked before it ran. Tools report such
    /// a run as an error result that still carries the whole envelope, so
    /// the model sees the output and exit code.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.interrupted || self.return_code_interpretation.is_some()
    }

    /// Output envelope for a command that was started in the background.
    #[must_use]
    pub fn background(task_id: String, backgrounded_by_user: bool) -> Self {
        Self {
            stdout: String::new(),
            stderr: String::new(),
            raw_output_path: None,
            interrupted: false,
            is_image: None,
            background_task_id: Some(task_id),
            backgrounded_by_user: Some(backgrounded_by_user),
            assistant_auto_backgrounded: Some(false),
            dangerously_disable_sandbox: None,
            return_code_interpretation: None,
            no_output_expected: Some(true),
            structured_content: None,
            persisted_output_path: None,
            persisted_output_size: None,
            sandbox_status: None,
        }
    }
}

/// Executes a shell command with the requested sandbox settings.
///
/// Equivalent to [`execute_bash_with_cancel`] with a token that is never
/// cancelled.
#[allow(clippy::needless_pass_by_value)]
pub fn execute_bash(input: BashCommandInput) -> io::Result<BashCommandOutput> {
    execute_bash_with_cancel(&input, &CancellationToken::new())
}

/// Executes a shell command, stopping it when `cancel` fires.
///
/// The command runs with stdin set to null, stdout and stderr drained
/// concurrently, Zero-X credentials removed from its environment, and a
/// timeout of [`DEFAULT_BASH_TIMEOUT_MS`] unless the input requests another
/// (capped at [`MAX_BASH_TIMEOUT_MS`]). On timeout or cancellation the whole
/// process tree is killed (`taskkill /T /F` on Windows, the process group on
/// Unix) and the output captured so far is returned with `interrupted: true`.
pub fn execute_bash_with_cancel(
    input: &BashCommandInput,
    cancel: &CancellationToken,
) -> io::Result<BashCommandOutput> {
    let cwd = env::current_dir()?;
    let sandbox_status = sandbox_status_for_input(input, &cwd);

    if input.run_in_background.unwrap_or(false) {
        let mut command = prepare_command(&input.command, &cwd, &sandbox_status);
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        let mut output = BashCommandOutput::background(child.id().to_string(), false);
        output.dangerously_disable_sandbox = input.dangerously_disable_sandbox;
        output.sandbox_status = Some(sandbox_status);
        return Ok(output);
    }

    let timeout = Duration::from_millis(effective_bash_timeout_ms(input.timeout));
    let mut command = prepare_command(&input.command, &cwd, &sandbox_status);
    let bounded =
        subprocess::run_bounded(&mut command, None, Some(timeout), &|| cancel.is_cancelled())?;
    let mut output = BashCommandOutput::from_bounded(&bounded, Some(timeout));
    output.dangerously_disable_sandbox = input.dangerously_disable_sandbox;
    output.sandbox_status = Some(sandbox_status);
    Ok(output)
}

fn sandbox_status_for_input(input: &BashCommandInput, cwd: &Path) -> SandboxStatus {
    let config = ConfigLoader::default_for(cwd).load().map_or_else(
        |_| SandboxConfig::default(),
        |runtime_config| runtime_config.sandbox().clone(),
    );
    let request = config.resolve_request(
        input.dangerously_disable_sandbox.map(|disabled| !disabled),
        input.namespace_restrictions,
        input.isolate_network,
        input.filesystem_mode,
        input.allowed_mounts.clone(),
    );
    resolve_sandbox_status_for_request(&request, cwd)
}

fn prepare_command(command: &str, cwd: &Path, sandbox_status: &SandboxStatus) -> Command {
    // Only active filesystem isolation (never on Windows) moves HOME and
    // TMPDIR, to per-workspace directories outside the project that are
    // created on first use.
    let sandbox_dirs = sandbox_status
        .filesystem_active
        .then(|| SandboxDirs::for_workspace(cwd));
    if let Some(dirs) = &sandbox_dirs {
        let _ = dirs.create();
    }

    let mut prepared =
        if let Some(launcher) = build_linux_sandbox_command(command, cwd, sandbox_status) {
            let mut prepared = Command::new(launcher.program);
            prepared.args(launcher.args);
            subprocess::scrub_credential_env(&mut prepared);
            prepared.envs(launcher.env);
            prepared
        } else {
            let mut prepared = host_shell_command(command);
            subprocess::scrub_credential_env(&mut prepared);
            if let Some(dirs) = &sandbox_dirs {
                prepared.env("HOME", &dirs.home);
                prepared.env("TMPDIR", &dirs.tmp);
            }
            prepared
        };
    prepared.current_dir(cwd);
    prepared
}

/// Interpretation for a process that ended without an exit code.
#[cfg(unix)]
fn killed_interpretation(status: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    status.signal().map_or_else(
        || String::from("killed"),
        |signal| format!("signal:{signal}"),
    )
}

#[cfg(not(unix))]
fn killed_interpretation(_status: std::process::ExitStatus) -> String {
    String::from("killed")
}

#[cfg(windows)]
fn host_shell_command(command: &str) -> Command {
    subprocess::windows_cmd_command(command)
}

#[cfg(not(windows))]
fn host_shell_command(command: &str) -> Command {
    let mut prepared = Command::new("sh");
    prepared.arg("-lc").arg(command);
    prepared
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{
        effective_bash_timeout_ms, execute_bash, execute_bash_with_cancel, BashCommandInput,
        DEFAULT_BASH_TIMEOUT_MS, MAX_BASH_TIMEOUT_MS,
    };
    use crate::cancel::CancellationToken;
    use crate::sandbox::FilesystemIsolationMode;

    fn input(command: &str, timeout: Option<u64>) -> BashCommandInput {
        BashCommandInput {
            command: command.to_string(),
            timeout,
            description: None,
            run_in_background: Some(false),
            dangerously_disable_sandbox: Some(true),
            namespace_restrictions: None,
            isolate_network: None,
            filesystem_mode: None,
            allowed_mounts: None,
        }
    }

    /// A command that would run for ~30 s and then print `late`.
    fn long_sleep_command() -> &'static str {
        if cfg!(windows) {
            "ping -n 30 127.0.0.1 >NUL & echo late"
        } else {
            "sleep 30; echo late"
        }
    }

    #[test]
    fn executes_simple_command() {
        let output = execute_bash(BashCommandInput {
            command: String::from("echo hello"),
            timeout: Some(10_000),
            description: None,
            run_in_background: Some(false),
            dangerously_disable_sandbox: Some(false),
            namespace_restrictions: Some(false),
            isolate_network: Some(false),
            filesystem_mode: Some(FilesystemIsolationMode::WorkspaceOnly),
            allowed_mounts: None,
        })
        .expect("bash command should execute");

        assert_eq!(output.stdout.trim(), "hello");
        assert!(!output.interrupted);
        assert!(output.sandbox_status.is_some());
    }

    fn workspace_only_input(command: &str) -> BashCommandInput {
        BashCommandInput {
            command: command.to_string(),
            timeout: Some(20_000),
            description: None,
            run_in_background: Some(false),
            dangerously_disable_sandbox: Some(false),
            namespace_restrictions: Some(false),
            isolate_network: Some(false),
            filesystem_mode: Some(FilesystemIsolationMode::WorkspaceOnly),
            allowed_mounts: None,
        }
    }

    /// The sandbox HOME / TMPDIR live in the config home, never in the
    /// project, and only exist once a command used them.
    #[cfg(unix)]
    #[test]
    fn filesystem_isolation_keeps_sandbox_dirs_out_of_the_workspace() {
        let _guard = crate::test_env_lock();
        let config_home = std::env::temp_dir().join(format!(
            "runtime-bash-sandbox-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));
        let previous = std::env::var_os("NEURON_CONFIG_HOME");
        std::env::set_var("NEURON_CONFIG_HOME", &config_home);
        let output = execute_bash(workspace_only_input(
            "printf '%s\\n%s' \"$HOME\" \"$TMPDIR\"",
        ));
        match previous {
            Some(value) => std::env::set_var("NEURON_CONFIG_HOME", value),
            None => std::env::remove_var("NEURON_CONFIG_HOME"),
        }

        let output = output.expect("command runs");
        assert!(
            output
                .sandbox_status
                .as_ref()
                .expect("status")
                .filesystem_active
        );
        let cwd = std::env::current_dir().expect("cwd");
        let mut lines = output.stdout.lines();
        for dir in [lines.next(), lines.next()] {
            let dir = std::path::PathBuf::from(dir.expect("printed dir"));
            assert!(dir.starts_with(&config_home), "{dir:?} {output:?}");
            assert!(!dir.starts_with(&cwd), "{dir:?}");
            assert!(dir.is_dir(), "{dir:?}");
        }
        let _ = std::fs::remove_dir_all(&config_home);
    }

    #[cfg(windows)]
    #[test]
    fn windows_commands_report_no_filesystem_isolation() {
        let output = execute_bash(workspace_only_input("echo hello")).expect("command runs");
        let status = output.sandbox_status.expect("sandbox status");
        assert!(!status.filesystem_active, "{status:?}");
        assert_eq!(status.filesystem_mode, FilesystemIsolationMode::Off);
        assert!(!status.active, "{status:?}");
    }

    #[test]
    fn disables_sandbox_when_requested() {
        let output = execute_bash(BashCommandInput {
            command: String::from("echo hello"),
            timeout: Some(10_000),
            description: None,
            run_in_background: Some(false),
            dangerously_disable_sandbox: Some(true),
            namespace_restrictions: None,
            isolate_network: None,
            filesystem_mode: None,
            allowed_mounts: None,
        })
        .expect("bash command should execute");

        assert!(!output.sandbox_status.expect("sandbox status").enabled);
    }

    #[test]
    fn timeout_defaults_and_clamps() {
        assert_eq!(effective_bash_timeout_ms(None), DEFAULT_BASH_TIMEOUT_MS);
        assert_eq!(effective_bash_timeout_ms(Some(5)), 5);
        assert_eq!(effective_bash_timeout_ms(Some(0)), 1);
        assert_eq!(
            effective_bash_timeout_ms(Some(u64::MAX)),
            MAX_BASH_TIMEOUT_MS
        );
        assert_eq!(DEFAULT_BASH_TIMEOUT_MS, 120_000);
        assert_eq!(MAX_BASH_TIMEOUT_MS, 600_000);
    }

    #[test]
    fn timeout_kills_the_process_tree_and_reports_interruption() {
        let started = Instant::now();
        let output = execute_bash(input(long_sleep_command(), Some(500)))
            .expect("timed-out command still returns output");
        assert!(output.interrupted);
        assert_eq!(
            output.return_code_interpretation.as_deref(),
            Some("timeout")
        );
        assert!(output.stderr.contains("Command exceeded timeout of 500 ms"));
        assert!(!output.stdout.contains("late"));
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "timed-out shell must be killed promptly, took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn cancellation_stops_a_running_command() {
        let token = CancellationToken::new();
        let trigger = token.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            trigger.cancel();
        });
        let started = Instant::now();
        let output = execute_bash_with_cancel(&input(long_sleep_command(), None), &token)
            .expect("cancelled command still returns output");
        canceller.join().expect("canceller");
        assert!(output.interrupted);
        assert_eq!(
            output.return_code_interpretation.as_deref(),
            Some("cancelled")
        );
        assert!(started.elapsed() < Duration::from_secs(15));
    }

    #[test]
    fn non_zero_exit_and_timeout_are_failures() {
        let command = if cfg!(windows) {
            "echo partial & exit /b 3"
        } else {
            "echo partial; exit 3"
        };
        let failed = execute_bash(input(command, Some(20_000))).expect("command runs");
        assert!(failed.failed(), "{failed:?}");
        assert_eq!(
            failed.return_code_interpretation.as_deref(),
            Some("exit_code:3")
        );
        assert!(failed.stdout.contains("partial"), "{failed:?}");

        let ok = execute_bash(input("echo fine", Some(20_000))).expect("command runs");
        assert!(!ok.failed(), "{ok:?}");
        assert_eq!(ok.return_code_interpretation, None);

        let timed_out =
            execute_bash(input(long_sleep_command(), Some(300))).expect("timed-out command");
        assert!(timed_out.failed(), "{timed_out:?}");
    }

    #[test]
    fn stdin_is_null_so_reading_commands_do_not_hang() {
        let command = if cfg!(windows) { "more" } else { "cat" };
        let started = Instant::now();
        let output =
            execute_bash(input(command, Some(20_000))).expect("stdin-reading command runs");
        assert!(!output.interrupted, "{output:?}");
        assert!(started.elapsed() < Duration::from_secs(15));
    }

    #[test]
    fn child_environment_excludes_zero_x_credentials() {
        let _guard = crate::test_env_lock();
        std::env::set_var("NEURON_TOKEN", "nzx_bash_scrub_token");
        std::env::set_var("RUNTIME_BASH_SCRUB_SESSION", "ses_bash_scrub_session");
        std::env::set_var("RUNTIME_BASH_SCRUB_KEEP", "visible-value");
        let command = if cfg!(windows) {
            "echo %NEURON_TOKEN% %RUNTIME_BASH_SCRUB_SESSION% %RUNTIME_BASH_SCRUB_KEEP%"
        } else {
            "echo ${NEURON_TOKEN:-unset} ${RUNTIME_BASH_SCRUB_SESSION:-unset} $RUNTIME_BASH_SCRUB_KEEP"
        };
        let output = execute_bash(input(command, Some(20_000)));
        std::env::remove_var("NEURON_TOKEN");
        std::env::remove_var("RUNTIME_BASH_SCRUB_SESSION");
        std::env::remove_var("RUNTIME_BASH_SCRUB_KEEP");

        let output = output.expect("command runs");
        assert!(
            !output.stdout.contains("nzx_bash_scrub_token"),
            "{output:?}"
        );
        assert!(
            !output.stdout.contains("ses_bash_scrub_session"),
            "{output:?}"
        );
        assert!(output.stdout.contains("visible-value"), "{output:?}");
    }

    #[cfg(windows)]
    #[test]
    fn cmd_receives_the_command_line_verbatim() {
        let output = execute_bash(input(r#"echo "quoted value" & echo second"#, Some(20_000)))
            .expect("command runs");
        assert!(
            output.stdout.contains("\"quoted value\""),
            "quotes must reach cmd.exe unescaped: {output:?}"
        );
        assert!(!output.stdout.contains("\\\""), "{output:?}");
        assert!(output.stdout.contains("second"), "{output:?}");
    }
}

/// Maximum output bytes before truncation (16 KiB, matching upstream).
const MAX_OUTPUT_BYTES: usize = 16_384;

/// Truncate output to `MAX_OUTPUT_BYTES`, appending a marker when trimmed.
fn truncate_output(s: &str) -> String {
    if s.len() <= MAX_OUTPUT_BYTES {
        return s.to_string();
    }
    // Find the last valid UTF-8 boundary at or before MAX_OUTPUT_BYTES
    let mut end = MAX_OUTPUT_BYTES;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = s[..end].to_string();
    truncated.push_str("\n\n[output truncated — exceeded 16384 bytes]");
    truncated
}

#[cfg(test)]
mod truncation_tests {
    use super::*;

    #[test]
    fn short_output_unchanged() {
        let s = "hello world";
        assert_eq!(truncate_output(s), s);
    }

    #[test]
    fn long_output_truncated() {
        let s = "x".repeat(20_000);
        let result = truncate_output(&s);
        assert!(result.len() < 20_000);
        assert!(result.ends_with("[output truncated — exceeded 16384 bytes]"));
    }

    #[test]
    fn exact_boundary_unchanged() {
        let s = "a".repeat(MAX_OUTPUT_BYTES);
        assert_eq!(truncate_output(&s), s);
    }

    #[test]
    fn one_over_boundary_truncated() {
        let s = "a".repeat(MAX_OUTPUT_BYTES + 1);
        let result = truncate_output(&s);
        assert!(result.contains("[output truncated"));
    }
}
