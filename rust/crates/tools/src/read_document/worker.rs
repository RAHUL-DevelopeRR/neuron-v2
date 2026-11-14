//! Reading a document in a child process.
//!
//! The parsers are hardened against hostile files, but a file that still
//! finds a way past their limits must not take the agent down with it: a
//! failed allocation aborts a process and a slow loop cannot be interrupted.
//! So the CLI runs each read in a copy of itself ([`WORKER_ARG`]): the parent
//! has already checked the path against the workspace and the permissions,
//! sends the request as JSON on stdin and reads one JSON reply from stdout.
//! The parent kills a child that runs past [`WORKER_TIMEOUT`] or whose turn
//! is cancelled (Esc), caps what it reads back, and turns a crash, an abort or a kill into a tool error. On
//! Windows the child also runs in a Job Object with a memory limit
//! ([`WORKER_MEMORY_BYTES`]) and no room to start other processes.
//!
//! Outside the CLI (the tools crate's own tests, other embedders) no worker
//! is registered and the read runs in this process.
//!
//! Follow-up: on Linux and macOS the child has no memory limit of its own
//! yet (an `RLIMIT_AS` needs `unsafe` code in `pre_exec`); the timeout and
//! the parsers' own limits apply there.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use runtime::cancel::CancellationToken;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Options;

/// The hidden argument that makes the CLI binary a document reader.
pub const WORKER_ARG: &str = "__neuron-read-document";
/// Wall-clock time one read may take.
pub(crate) const WORKER_TIMEOUT: Duration = Duration::from_secs(30);
/// Memory the child may commit (Windows Job Object).
#[cfg_attr(not(windows), allow(dead_code))]
const WORKER_MEMORY_BYTES: u64 = 1024 * 1024 * 1024;
/// Bytes of reply read back; a reply is a few hundred KB at most.
const MAX_REPLY_BYTES: u64 = 8 * 1024 * 1024;
/// Bytes of request a worker reads from stdin.
const MAX_REQUEST_BYTES: u64 = 1024 * 1024;

static WORKER: OnceLock<PathBuf> = OnceLock::new();

/// Makes `read_document` run each read in `exe` (the CLI's own binary,
/// which must handle [`WORKER_ARG`] through [`run_worker`]).
pub fn use_worker(exe: PathBuf) {
    let _ = WORKER.set(exe);
}

#[derive(Debug, Serialize, Deserialize)]
struct Request {
    path: PathBuf,
    pages: Option<String>,
    rows: Option<String>,
    columns: Option<String>,
    max_chars: Option<usize>,
}

/// Reads `path` (already checked against the workspace and size limits):
/// in a worker process when one is registered, else in this process. A
/// cancelled `cancel` stops the worker at once.
pub(crate) fn read(
    path: &Path,
    options: &Options<'_>,
    cancel: &CancellationToken,
) -> Result<String, String> {
    if cancel.is_cancelled() {
        return Err(CANCELLED.to_string());
    }
    match WORKER.get() {
        Some(exe) => read_in_worker(exe, path, options, WORKER_TIMEOUT, cancel),
        None => super::read_document(path, options),
    }
}

/// The error for a read stopped because its turn was cancelled.
pub(crate) const CANCELLED: &str = "read_document was cancelled";

/// Reads `path` in a child process started from `exe`.
pub(crate) fn read_in_worker(
    exe: &Path,
    path: &Path,
    options: &Options<'_>,
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<String, String> {
    let request = Request {
        path: path.to_path_buf(),
        pages: options.pages.map(str::to_string),
        rows: options.rows.map(str::to_string),
        columns: options.columns.map(str::to_string),
        max_chars: options.max_chars,
    };
    let input = serde_json::to_vec(&request).map_err(|error| unsafe_read(&error.to_string()))?;
    let mut command = Command::new(exe);
    command.arg(WORKER_ARG);
    let output = run_child(command, &input, timeout, cancel)?;
    let reply: Value = serde_json::from_slice(&output)
        .map_err(|_| unsafe_read("the reader sent back an unreadable reply"))?;
    if let Some(markdown) = reply.get("ok").and_then(Value::as_str) {
        return Ok(markdown.to_string());
    }
    match reply.get("error").and_then(Value::as_str) {
        Some(error) => Err(error.to_string()),
        None => Err(unsafe_read("the reader sent back an unreadable reply")),
    }
}

/// Reads `path` in a worker started from `exe`, for checks of the worker
/// path from outside this crate (the CLI's tests run its own binary).
pub fn read_document_with_worker(
    exe: &Path,
    path: &Path,
    pages: Option<&str>,
    max_chars: Option<usize>,
) -> Result<String, String> {
    read_in_worker(
        exe,
        path,
        &Options {
            pages,
            max_chars,
            ..Options::default()
        },
        WORKER_TIMEOUT,
        &CancellationToken::new(),
    )
}

fn unsafe_read(reason: &str) -> String {
    format!("could not read this document safely: {reason}")
}

/// Runs `command` with `input` on stdin and returns its stdout, enforcing
/// `timeout`, `cancel`, [`MAX_REPLY_BYTES`] and (on Windows) the memory
/// limit. Any way the child ends other than exiting with status 0 is an
/// error.
pub(crate) fn run_child(
    mut command: Command,
    input: &[u8],
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, String> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Its own process group, so a Ctrl-C in the terminal reaches the agent,
    // which cancels the read, instead of killing the reader mid-way.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// No console window for the child.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(winjob::CREATE_SUSPENDED | CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .map_err(|error| unsafe_read(&format!("the reader could not start ({error})")))?;
    // Kept alive until the child is done: closing it kills the child.
    #[cfg(windows)]
    let _job = {
        let job = winjob::Job::new(&winjob::JobLimits {
            max_processes: Some(1),
            memory_limit_bytes: Some(WORKER_MEMORY_BYTES),
        })
        .ok()
        .filter(|job| job.assign(&child).is_ok());
        if winjob::resume_process(child.id()).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(unsafe_read("the reader could not start"));
        }
        job
    };
    if let Some(mut stdin) = child.stdin.take() {
        // A child that exits early closes the pipe; its exit status says why.
        let _ = stdin.write_all(input);
    }
    let reader = child.stdout.take().map(|stdout| {
        std::thread::spawn(move || {
            let mut stdout = stdout;
            let mut reply = Vec::new();
            let _ = (&mut stdout)
                .take(MAX_REPLY_BYTES + 1)
                .read_to_end(&mut reply);
            // Drain the rest so the child is never stuck on a full pipe.
            let _ = std::io::copy(&mut stdout, &mut std::io::sink());
            reply
        })
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if cancel.is_cancelled() => {
                let _ = child.kill();
                let _ = child.wait();
                if let Some(reader) = reader {
                    let _ = reader.join();
                }
                return Err(CANCELLED.to_string());
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(unsafe_read(&format!("the reader was lost ({error})")));
            }
        }
    };
    let reply = reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let Some(status) = status else {
        return Err(unsafe_read(&format!(
            "reading took longer than {} seconds and was stopped",
            timeout.as_secs()
        )));
    };
    if !status.success() {
        let code = status
            .code()
            .map_or_else(|| "a signal".to_string(), |code| format!("code {code}"));
        return Err(unsafe_read(&format!(
            "the reader stopped unexpectedly ({code}); the file may need more memory than allowed or be damaged"
        )));
    }
    if reply.len() as u64 > MAX_REPLY_BYTES {
        return Err(unsafe_read("the reader's output was too large"));
    }
    Ok(reply)
}

/// The worker side: reads one request from stdin, reads the document and
/// writes the reply to stdout. Returns the process exit code.
#[must_use]
pub fn run_worker() -> i32 {
    let mut input = Vec::new();
    if std::io::stdin()
        .take(MAX_REQUEST_BYTES)
        .read_to_end(&mut input)
        .is_err()
    {
        return 2;
    }
    let reply = match serde_json::from_slice::<Request>(&input) {
        Ok(request) => {
            let options = Options {
                pages: request.pages.as_deref(),
                rows: request.rows.as_deref(),
                columns: request.columns.as_deref(),
                max_chars: request.max_chars,
            };
            match super::read_document(&request.path, &options) {
                Ok(markdown) => serde_json::json!({ "ok": markdown }),
                Err(error) => serde_json::json!({ "error": error }),
            }
        }
        Err(error) => serde_json::json!({ "error": format!("bad read_document request: {error}") }),
    };
    let mut stdout = std::io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &reply).is_err() || stdout.flush().is_err() {
        return 2;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::{run_child, CANCELLED};
    use runtime::cancel::CancellationToken;
    use std::process::Command;
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

    #[test]
    fn a_child_that_runs_too_long_is_killed_with_a_clear_error() {
        let sleeper = if cfg!(windows) {
            let mut command = Command::new("ping.exe");
            command.args(["-n", "30", "127.0.0.1"]);
            command
        } else {
            let mut command = Command::new("sleep");
            command.arg("30");
            command
        };
        let started = Instant::now();
        let error = run_child(
            sleeper,
            b"",
            Duration::from_millis(500),
            &CancellationToken::new(),
        )
        .expect_err("killed");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "killed promptly"
        );
        assert!(
            error.starts_with("could not read this document safely: reading took longer"),
            "{error}"
        );
    }

    #[test]
    fn a_child_that_fails_or_answers_nonsense_is_an_error() {
        let error = run_child(
            shell("exit 3"),
            b"",
            Duration::from_secs(20),
            &CancellationToken::new(),
        )
        .expect_err("fails");
        assert!(
            error.contains(
                "could not read this document safely: the reader stopped unexpectedly (code 3)"
            ),
            "{error}"
        );
        let output = run_child(
            shell("echo hello"),
            b"",
            Duration::from_secs(20),
            &CancellationToken::new(),
        )
        .expect("runs");
        assert!(String::from_utf8_lossy(&output).contains("hello"));
        assert!(serde_json::from_slice::<serde_json::Value>(&output).is_err());
    }

    fn sleeper() -> Command {
        if cfg!(windows) {
            let mut command = Command::new("ping.exe");
            command.args(["-n", "30", "127.0.0.1"]);
            command
        } else {
            let mut command = Command::new("sleep");
            command.arg("30");
            command
        }
    }

    #[test]
    fn a_cancelled_turn_stops_the_child_at_once() {
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            trigger.cancel();
        });
        let started = Instant::now();
        let error =
            run_child(sleeper(), b"", Duration::from_secs(30), &cancel).expect_err("cancelled");
        canceller.join().expect("canceller");
        assert_eq!(error, CANCELLED);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "stopped well before the timeout: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_read_cancelled_before_it_starts_does_nothing() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = super::read(
            std::path::Path::new("missing.docx"),
            &super::Options::default(),
            &cancel,
        )
        .expect_err("cancelled");
        assert_eq!(error, CANCELLED);
    }

    #[test]
    fn without_a_registered_worker_reads_run_in_process() {
        // The tools crate's tests never register a worker.
        assert!(super::WORKER.get().is_none());
        let error = super::read(
            std::path::Path::new("missing.docx"),
            &super::Options::default(),
            &CancellationToken::new(),
        )
        .expect_err("no such file");
        assert!(
            !error.contains("could not read this document safely"),
            "{error}"
        );
    }
}
