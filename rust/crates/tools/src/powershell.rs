//! Execution for the `PowerShell` tool.
//!
//! The command is passed with `-EncodedCommand` (base64 of UTF-16LE), so no
//! quoting layer between Neuron and PowerShell can mangle quotes, `$` or
//! backticks. Runs are bounded like the `bash` tool: stdin is empty, stdout
//! and stderr are drained concurrently, Zero-X credentials are removed from
//! the environment, and on timeout or cancellation the process tree is
//! killed.

use std::io;
use std::process::{Command, Stdio};
use std::time::Duration;

use plugins::subprocess;
use runtime::cancel::CancellationToken;
use runtime::{effective_bash_timeout_ms, BashCommandOutput};

/// Statements run before the user's command: no progress records on
/// stderr, and UTF-8 output so non-ASCII text survives the pipe.
const PREAMBLE: &str = "$ProgressPreference = 'SilentlyContinue'\n\
try { [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false } catch {}\n\
$OutputEncoding = New-Object System.Text.UTF8Encoding $false\n";

/// The `PowerShell` executable: `pwsh` when installed, otherwise Windows
/// `PowerShell` (`powershell`). Looked up on `PATH` for every call, so a
/// `PATH` change takes effect immediately.
pub(crate) fn detect_powershell() -> io::Result<&'static str> {
    ["pwsh", "powershell"]
        .into_iter()
        .find(|candidate| super::command_exists(candidate))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "PowerShell executable not found (expected `pwsh` or `powershell` in PATH)",
            )
        })
}

/// Arguments that make `shell` run `command` non-interactively.
#[must_use]
pub(crate) fn powershell_args(command: &str) -> Vec<String> {
    vec![
        "-NoLogo".to_string(),
        "-NoProfile".to_string(),
        "-NonInteractive".to_string(),
        "-OutputFormat".to_string(),
        "Text".to_string(),
        "-EncodedCommand".to_string(),
        encode_command(command),
    ]
}

/// Base64 of the UTF-16LE script, as `-EncodedCommand` expects.
#[must_use]
pub(crate) fn encode_command(command: &str) -> String {
    let script = format!("{PREAMBLE}{command}");
    let bytes = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    base64_encode(&bytes)
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let triple = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        for (position, shift) in [18_u32, 12, 6, 0].into_iter().enumerate() {
            if position <= chunk.len() {
                let index = usize::try_from((triple >> shift) & 0x3f).unwrap_or(0);
                encoded.push(char::from(ALPHABET[index]));
            } else {
                encoded.push('=');
            }
        }
    }
    encoded
}

/// Runs `command` with the detected `PowerShell`.
pub(crate) fn execute(
    command: &str,
    timeout_ms: Option<u64>,
    run_in_background: bool,
    cancel: &CancellationToken,
) -> io::Result<BashCommandOutput> {
    let shell = detect_powershell()?;
    let mut process = Command::new(shell);
    process.args(powershell_args(command));
    subprocess::scrub_credential_env(&mut process);
    if let Ok(cwd) = std::env::current_dir() {
        process.current_dir(cwd);
    }

    if run_in_background {
        process
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Stopped with its tree when the session ends (`runtime::stop_background_commands`).
        subprocess::isolate_process_group(&mut process);
        let child = process.spawn()?;
        let task_id = child.id().to_string();
        subprocess::track_background_child(child);
        return Ok(BashCommandOutput::background(task_id, true));
    }

    let timeout = Duration::from_millis(effective_bash_timeout_ms(timeout_ms));
    let bounded =
        subprocess::run_bounded(&mut process, None, Some(timeout), &|| cancel.is_cancelled())?;
    Ok(BashCommandOutput::from_bounded(&bounded, Some(timeout)))
}

#[cfg(test)]
mod tests {
    use super::{base64_encode, detect_powershell, encode_command, execute, PREAMBLE};
    use runtime::cancel::CancellationToken;
    use std::time::{Duration, Instant};

    #[test]
    fn base64_matches_the_reference_encoding() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        // PowerShell's own example: "dir" as UTF-16LE.
        assert_eq!(base64_encode(&[100, 0, 105, 0, 114, 0]), "ZABpAHIA");
        let script = format!("{PREAMBLE}dir")
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(encode_command("dir"), base64_encode(&script));
    }

    #[test]
    fn quotes_and_variables_reach_powershell_unchanged() {
        if detect_powershell().is_err() {
            eprintln!("skipping: PowerShell is not installed");
            return;
        }
        let output = execute(
            r#"$name = "quoted value"; Write-Output "[$name]" 'single $literal'"#,
            Some(60_000),
            false,
            &CancellationToken::new(),
        )
        .expect("PowerShell runs");
        assert!(output.stdout.contains("[quoted value]"), "{output:?}");
        assert!(output.stdout.contains("single $literal"), "{output:?}");
        assert!(!output.stderr.contains("CLIXML"), "{output:?}");
    }

    #[test]
    fn powershell_children_do_not_inherit_zero_x_credentials() {
        if detect_powershell().is_err() {
            eprintln!("skipping: PowerShell is not installed");
            return;
        }
        std::env::set_var("TOOLS_PS_SCRUB_SECRET", "nzx_ps_scrub_secret");
        std::env::set_var("TOOLS_PS_SCRUB_KEEP", "keep-ps");
        let output = execute(
            "Write-Output \"secret=$env:TOOLS_PS_SCRUB_SECRET keep=$env:TOOLS_PS_SCRUB_KEEP\"",
            Some(60_000),
            false,
            &CancellationToken::new(),
        );
        std::env::remove_var("TOOLS_PS_SCRUB_SECRET");
        std::env::remove_var("TOOLS_PS_SCRUB_KEEP");

        let output = output.expect("PowerShell runs");
        assert!(!output.stdout.contains("nzx_ps_scrub_secret"), "{output:?}");
        assert!(output.stdout.contains("keep=keep-ps"), "{output:?}");
    }

    #[test]
    fn timeout_kills_powershell_and_reports_interruption() {
        if detect_powershell().is_err() {
            eprintln!("skipping: PowerShell is not installed");
            return;
        }
        let started = Instant::now();
        let output = execute(
            "Start-Sleep -Seconds 30; Write-Output late",
            Some(1_500),
            false,
            &CancellationToken::new(),
        )
        .expect("timed-out command still returns output");
        assert!(output.interrupted, "{output:?}");
        assert_eq!(
            output.return_code_interpretation.as_deref(),
            Some("timeout")
        );
        assert!(!output.stdout.contains("late"));
        assert!(started.elapsed() < Duration::from_secs(20));
    }
}
