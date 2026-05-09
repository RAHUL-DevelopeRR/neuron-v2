//! PTY-based persistent shell session for NeuronCLI
//!
//! Inspired by Claude Code's streaming tool executor:
//! - One PTY session per workspace
//! - Commands streamed chunk-by-chunk
//! - Output captured incrementally
//! - Timeout enforcement
//! - Command filtering (banned/safe lists from opencode)

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Max output before truncation (30K chars, per opencode).
const MAX_OUTPUT_LEN: usize = 30_000;
/// Default command timeout (60 seconds).
const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// Max command timeout (10 minutes, per opencode).
const MAX_TIMEOUT_SECS: u64 = 600;

/// Commands that are completely banned for security.
const BANNED_COMMANDS: &[&str] = &[
    "curl", "curlie", "wget", "axel", "aria2c", "nc", "netcat", "telnet", "lynx", "w3m", "links",
    "httpie", "xh", "chrome", "firefox", "safari", "chromium", "edge",
];

/// Commands allowed without confirmation (read-only / safe).
const SAFE_READONLY_COMMANDS: &[&str] = &[
    "ls",
    "dir",
    "echo",
    "pwd",
    "cd",
    "cat",
    "type",
    "git status",
    "git log",
    "git diff",
    "git show",
    "git branch",
    "git remote",
    "git ls-files",
    "git rev-parse",
    "git describe",
    "git blame",
    "go version",
    "go env",
    "go list",
    "go help",
    "rustc --version",
    "cargo --version",
    "python --version",
    "node --version",
    "find",
    "grep",
    "rg",
    "fd",
];

/// Result of a PTY shell execution.
#[derive(Debug, Clone)]
pub struct PtyResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub truncated: bool,
}

/// PTY shell session — one per workspace, persistent across turns.
pub struct PtyShell {
    cwd: Arc<Mutex<String>>,
    env: Arc<Mutex<Vec<(String, String)>>>,
    last_output: Arc<Mutex<String>>,
}

impl PtyShell {
    pub fn new() -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string());
        Self {
            cwd: Arc::new(Mutex::new(cwd)),
            env: Arc::new(Mutex::new(Vec::new())),
            last_output: Arc::new(Mutex::new(String::new())),
        }
    }

    /// Check if a command is banned.
    pub fn is_banned(cmd: &str) -> bool {
        let cmd_lower = cmd.trim().to_lowercase();
        for banned in BANNED_COMMANDS {
            if cmd_lower.starts_with(banned) || cmd_lower.contains(&format!(" {banned} ")) {
                return true;
            }
        }
        false
    }

    /// Check if a command is safe read-only (no confirmation needed).
    pub fn is_safe_readonly(cmd: &str) -> bool {
        let cmd_trim = cmd.trim();
        for safe in SAFE_READONLY_COMMANDS {
            if cmd_trim.starts_with(safe) {
                return true;
            }
        }
        false
    }

    /// Execute a command with streaming output.
    /// Returns incremental chunks via callback for real-time display.
    pub fn execute_streaming<F>(
        &self,
        command: &str,
        timeout_secs: u64,
        mut on_chunk: F,
    ) -> Result<PtyResult, String>
    where
        F: FnMut(&str),
    {
        if Self::is_banned(command) {
            return Err(format!(
                "Command blocked for security: '{}'.\n\
                 Banned commands: {}.",
                command,
                BANNED_COMMANDS.join(", ")
            ));
        }

        let timeout = Duration::from_secs(timeout_secs.clamp(1, MAX_TIMEOUT_SECS));
        let start = Instant::now();

        let cwd = self.cwd.lock().unwrap().clone();

        // Build command (PowerShell on Windows, bash on Unix)
        #[cfg(windows)]
        let mut child = Command::new("powershell.exe")
            .arg("-Command")
            .arg(command)
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn shell: {e}"))?;

        #[cfg(not(windows))]
        let mut child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn shell: {e}"))?;

        let mut stdout_buf = String::new();
        let mut stderr_buf = String::new();
        let mut total_len = 0usize;
        let mut truncated = false;

        // Read stdout incrementally
        if let Some(mut stdout) = child.stdout.take() {
            let mut buf = [0u8; 4096];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = String::from_utf8_lossy(&buf[..n]);
                        if total_len + chunk.len() > MAX_OUTPUT_LEN {
                            let remaining = MAX_OUTPUT_LEN.saturating_sub(total_len);
                            if remaining > 0 {
                                let partial = &chunk[..remaining];
                                stdout_buf.push_str(partial);
                                on_chunk(partial);
                            }
                            truncated = true;
                            break;
                        }
                        stdout_buf.push_str(&chunk);
                        on_chunk(&chunk);
                        total_len += chunk.len();
                    }
                    Err(e) => {
                        eprintln!("[shell] stdout read error: {e}");
                        break;
                    }
                }
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    truncated = true;
                    break;
                }
            }
        }

        // Collect stderr (non-streaming, usually small)
        if let Some(mut stderr) = child.stderr.take() {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf);
            stderr_buf = String::from_utf8_lossy(&buf).to_string();
        }

        let exit_code = match child.wait() {
            Ok(status) => status.code(),
            Err(e) => {
                eprintln!("[shell] wait error: {e}");
                None
            }
        };

        let duration_ms = start.elapsed().as_millis() as u64;

        if truncated {
            stdout_buf.push_str("\n... [output truncated at 30K chars]");
        }

        let result = PtyResult {
            stdout: stdout_buf.clone(),
            stderr: stderr_buf,
            exit_code,
            duration_ms,
            truncated,
        };

        // Store last output for context
        *self.last_output.lock().unwrap() = stdout_buf;

        Ok(result)
    }

    /// Execute without streaming callback (blocking, for internal use).
    pub fn execute(&self, command: &str, timeout_secs: u64) -> Result<PtyResult, String> {
        self.execute_streaming(command, timeout_secs, |_chunk| {})
    }

    /// Change working directory.
    pub fn set_cwd(&self, path: &str) {
        *self.cwd.lock().unwrap() = path.to_string();
    }

    /// Get current working directory.
    pub fn cwd(&self) -> String {
        self.cwd.lock().unwrap().clone()
    }

    /// Get last command output (for context injection).
    pub fn last_output(&self) -> String {
        self.last_output.lock().unwrap().clone()
    }

    /// Clear all state — NO CACHE RESIDUE.
    pub fn clear_cache(&self) {
        *self.last_output.lock().unwrap() = String::new();
        // CWD and env are preserved (they are workspace state, not cache)
    }
}

/// Global singleton PTY shell (one per process).
static GLOBAL_PTY: std::sync::OnceLock<PtyShell> = std::sync::OnceLock::new();

pub fn global_pty() -> &'static PtyShell {
    GLOBAL_PTY.get_or_init(PtyShell::new)
}

/// Reset all cached state between turns — ZERO CACHE RESIDUE.
pub fn clear_all_cache() {
    let pty = global_pty();
    pty.clear_cache();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_banned_commands() {
        assert!(PtyShell::is_banned("curl https://example.com"));
        assert!(PtyShell::is_banned("wget file.zip"));
        assert!(!PtyShell::is_banned("ls -la"));
        assert!(!PtyShell::is_banned("git status"));
    }

    #[test]
    fn test_safe_readonly() {
        assert!(PtyShell::is_safe_readonly("git status"));
        assert!(PtyShell::is_safe_readonly("ls"));
        assert!(!PtyShell::is_safe_readonly("rm file.txt"));
    }
}
