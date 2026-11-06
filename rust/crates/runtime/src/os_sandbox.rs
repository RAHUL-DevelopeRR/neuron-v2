//! Per-platform sandbox plumbing for shell commands (used by `bash.rs`).
//!
//! - Linux: `unshare` namespaces ([`crate::sandbox::build_linux_sandbox_command`]).
//! - macOS: a generated `sandbox-exec` profile
//!   ([`crate::sandbox::build_macos_sandbox_command`]).
//! - Windows: a kill-on-close Job Object with an active-process limit and an
//!   optional memory limit (`winjob`). The child is spawned suspended, put in
//!   the job, then resumed, so nothing it starts escapes the job. A restricted
//!   token / low integrity level is not applied: `std::process::Command` spawns
//!   with `CreateProcessW` and has no way to pass a token, so that would need
//!   a separate `CreateProcessAsUserW` spawn path with its own pipe and
//!   output handling.
//! - All: with `sandbox.network.allow` set, commands get proxy variables for
//!   the loopback allowlist proxy ([`crate::net_proxy`]).

use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::process::{Child, Command};
use std::time::Duration;

use plugins::subprocess::{self, BoundedOutput};

use crate::net_allow::{proxy_env, NetworkAllowlist};
use crate::sandbox::{
    build_linux_sandbox_command, build_macos_sandbox_command, LinuxSandboxCommand, SandboxStatus,
};

/// The allowlist proxy's address when commands should use it: an allowlist
/// is set and the command still has a network (a Linux network namespace has
/// none, and could not reach a loopback proxy on the host anyway).
#[must_use]
pub fn allowlist_proxy(status: &SandboxStatus) -> Option<SocketAddr> {
    let allow = status.network_allowlist.as_ref()?;
    if status.network_active && !status.seatbelt_active {
        return None;
    }
    crate::net_proxy::ensure_started(NetworkAllowlist::new(allow)).ok()
}

/// The OS launcher wrapping the shell, if this platform has one active.
#[must_use]
pub fn launcher(
    command: &str,
    cwd: &Path,
    status: &SandboxStatus,
    proxy: Option<SocketAddr>,
) -> Option<LinuxSandboxCommand> {
    build_linux_sandbox_command(command, cwd, status).or_else(|| {
        build_macos_sandbox_command(command, cwd, status, proxy.map(|addr| addr.port()))
    })
}

/// Adds the proxy variables for `proxy` to `command`.
pub fn apply_proxy_env(command: &mut Command, proxy: Option<SocketAddr>) {
    if let Some(proxy) = proxy {
        command.envs(proxy_env(proxy));
    }
}

/// Windows process containment for one command (a no-op elsewhere).
#[derive(Default)]
pub struct Containment {
    #[cfg(windows)]
    job: Option<winjob::Job>,
    /// Why containment did not apply, for the command's stderr.
    pub note: Option<String>,
}

impl Containment {
    /// Creates the job when the status says containment is active, and sets
    /// `CREATE_SUSPENDED` on `command` so it starts inside the job.
    #[cfg_attr(not(windows), allow(unused_variables, clippy::unnecessary_wraps))]
    pub fn prepare(command: &mut Command, status: &SandboxStatus) -> Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            if !status.process_containment_active {
                return Self::default();
            }
            let limits = winjob::JobLimits {
                max_processes: Some(
                    status
                        .requested
                        .max_processes
                        .unwrap_or(crate::sandbox::DEFAULT_MAX_PROCESSES),
                ),
                memory_limit_bytes: status
                    .requested
                    .memory_limit_mb
                    .map(|mb| mb.saturating_mul(1024 * 1024)),
            };
            match winjob::Job::new(&limits) {
                Ok(job) => {
                    command.creation_flags(winjob::CREATE_SUSPENDED);
                    Self {
                        job: Some(job),
                        note: None,
                    }
                }
                Err(error) => Self {
                    job: None,
                    note: Some(format!("process containment unavailable: {error}")),
                },
            }
        }
        #[cfg(not(windows))]
        Self::default()
    }

    /// Puts the just-spawned (suspended) child in the job and resumes it.
    /// A failed assignment still resumes the child, uncontained, with a note;
    /// a failed resume is an error (the caller kills the child).
    #[cfg_attr(not(windows), allow(unused_variables, clippy::unnecessary_wraps))]
    pub fn after_spawn(&mut self, child: &Child) -> io::Result<()> {
        #[cfg(windows)]
        if let Some(job) = &self.job {
            if let Err(error) = job.assign(child) {
                self.note = Some(format!("process containment unavailable: {error}"));
            }
            winjob::resume_process(child.id())?;
        }
        Ok(())
    }

    /// Keeps the job (and so the kill-on-close guarantee) alive until the
    /// process exits: for background commands, which outlive this call.
    pub fn keep_until_exit(self) {
        #[cfg(windows)]
        if let Some(job) = self.job {
            use std::sync::{Mutex, PoisonError};
            static JOBS: Mutex<Vec<winjob::Job>> = Mutex::new(Vec::new());
            JOBS.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(job);
        }
    }
}

/// [`subprocess::run_bounded`] inside the platform containment. Dropping the
/// job at the end kills anything the command left running (Windows).
pub fn run_bounded(
    command: &mut Command,
    status: &SandboxStatus,
    timeout: Duration,
    is_cancelled: &dyn Fn() -> bool,
) -> io::Result<(BoundedOutput, Option<String>)> {
    let mut containment = Containment::prepare(command, status);
    let output = crate::tool_output::run_bounded_live_with_spawn_hook(
        command,
        None,
        Some(timeout),
        is_cancelled,
        &mut |child| containment.after_spawn(child),
    )?;
    Ok((output, containment.note.take()))
}

/// Spawns a background command inside the platform containment.
pub fn spawn_background(command: &mut Command, status: &SandboxStatus) -> io::Result<Child> {
    let mut containment = Containment::prepare(command, status);
    let mut child = command.spawn()?;
    if let Err(error) = containment.after_spawn(&child) {
        subprocess::kill_process_tree(&mut child);
        let _ = child.wait();
        return Err(error);
    }
    containment.keep_until_exit();
    Ok(child)
}
