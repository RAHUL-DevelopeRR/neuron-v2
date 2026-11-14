//! Reading a document in a child process.
//!
//! The parsers are hardened against hostile files, but a file that still
//! finds a way past their limits must not take the agent down with it: a
//! failed allocation aborts a process and a slow loop cannot be interrupted.
//! So the CLI runs each read in a copy of itself ([`WORKER_ARG`]): the parent
//! has already checked the path against the workspace and the permissions,
//! sends the request on stdin (its length, then JSON) and reads one JSON
//! reply from stdout. The parent kills a child that runs past
//! [`WORKER_TIMEOUT`] or whose turn is cancelled (Esc), caps what it reads
//! back, and turns a crash, an abort or a kill into a tool error.
//!
//! The child also stops itself, so it cannot outlive the parent: it exits
//! once it has run [`WORKER_GRACE`] past [`WORKER_TIMEOUT`], and as soon as
//! its stdin closes. The parent keeps stdin open until the child is done, so
//! stdin closes early only when the parent has gone (killed, crashed, or its
//! terminal closed).
//!
//! On Windows the child also runs in a Job Object with a memory limit
//! ([`WORKER_MEMORY_BYTES`]), no room to start other processes, and
//! kill-on-close. If the job cannot be set up (Windows refuses to create or
//! assign it), the read still runs in the child, without the memory limit,
//! and its result carries a note saying so; the time limit and the parent
//! watch above still apply, and the parsers' own limits bound the work.
//!
//! The path travels as the operating system's own units (bytes on Unix, wide
//! characters on Windows), so a path that is not valid Unicode reads too.
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
/// How long past [`WORKER_TIMEOUT`] a worker runs before it stops itself:
/// the parent normally kills it first.
pub(crate) const WORKER_GRACE: Duration = Duration::from_secs(5);
/// Exit code of a worker that stopped itself (out of time, or its parent
/// went away).
const SELF_STOP_CODE: i32 = 4;
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
    /// The path as the operating system's units: see [`path_units`].
    path: PathUnits,
    pages: Option<String>,
    rows: Option<String>,
    columns: Option<String>,
    max_chars: Option<usize>,
}

#[cfg(windows)]
type PathUnits = Vec<u16>;
#[cfg(not(windows))]
type PathUnits = Vec<u8>;

/// `path` as wide characters (Windows) or bytes (elsewhere), exactly as the
/// operating system holds it, whether or not it is valid Unicode.
fn path_units(path: &Path) -> PathUnits {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().collect()
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(any(windows, unix)))]
    {
        path.to_string_lossy().into_owned().into_bytes()
    }
}

/// The path [`path_units`] made.
#[cfg_attr(windows, allow(clippy::needless_pass_by_value))]
fn path_from_units(units: PathUnits) -> PathBuf {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_wide(&units))
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_vec(units))
    }
    #[cfg(not(any(windows, unix)))]
    {
        PathBuf::from(String::from_utf8_lossy(&units).into_owned())
    }
}

/// The bytes a worker reads from stdin for one request: its length as eight
/// little-endian bytes, then the JSON.
fn encode_request(request: &Request) -> Result<Vec<u8>, String> {
    let json = serde_json::to_vec(request).map_err(|error| error.to_string())?;
    let mut framed = Vec::with_capacity(json.len() + 8);
    framed.extend((json.len() as u64).to_le_bytes());
    framed.extend(json);
    Ok(framed)
}

/// The request bytes for reading `path`, for checks of the worker protocol
/// from outside this crate (the CLI's tests talk to its own binary).
pub fn worker_request(
    path: &Path,
    pages: Option<&str>,
    max_chars: Option<usize>,
) -> Result<Vec<u8>, String> {
    encode_request(&Request {
        path: path_units(path),
        pages: pages.map(str::to_string),
        rows: None,
        columns: None,
        max_chars,
    })
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
        path: path_units(path),
        pages: options.pages.map(str::to_string),
        rows: options.rows.map(str::to_string),
        columns: options.columns.map(str::to_string),
        max_chars: options.max_chars,
    };
    let input = encode_request(&request).map_err(|error| unsafe_read(&error))?;
    let mut command = Command::new(exe);
    command.arg(WORKER_ARG);
    let output = run_child(command, &input, timeout, cancel)?;
    let note = output.uncontained.as_deref();
    let reply: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| unsafe_read("the reader sent back an unreadable reply"))?;
    if let Some(markdown) = reply.get("ok").and_then(Value::as_str) {
        return Ok(with_containment_note(markdown.to_string(), note));
    }
    match reply.get("error").and_then(Value::as_str) {
        Some(error) => Err(with_containment_note(error.to_string(), note)),
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

/// Runs `command` with `input` on stdin and returns its stdout (with any
/// note on its containment), enforcing
/// `timeout`, `cancel`, [`MAX_REPLY_BYTES`] and (on Windows) the memory
/// limit. Stdin is written from a thread, so a child that never reads it
/// cannot block the parent, and stays open until the child is done: a
/// worker takes stdin closing as its parent going away. Any way the child
/// ends other than exiting with status 0 is an error.
pub(crate) fn run_child(
    mut command: Command,
    input: &[u8],
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<ChildOutput, String> {
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
    let mut containment = contain(&mut child)?;
    let writer = child.stdin.take().map(|mut stdin| {
        let input = input.to_vec();
        std::thread::spawn(move || {
            // A child that exits early closes the pipe; its exit status says
            // why. The pipe is handed back to be closed once the child is done.
            let _ = stdin.write_all(&input);
            let _ = stdin.flush();
            stdin
        })
    });
    // Closes stdin once the child has ended. A write still pending is left
    // to fail on its own once the pipe's reader is gone, rather than waited
    // on.
    let close_stdin = move || {
        if let Some(writer) = writer {
            if writer.is_finished() {
                drop(writer.join());
            }
        }
    };
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
                close_stdin();
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
                close_stdin();
                return Err(unsafe_read(&format!("the reader was lost ({error})")));
            }
        }
    };
    close_stdin();
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
    Ok(ChildOutput {
        stdout: reply,
        uncontained: containment.uncontained.take(),
    })
}

/// The child's Windows Job Object, kept alive until the child is done:
/// closing it kills the child.
struct Containment {
    #[cfg(windows)]
    _job: Option<winjob::Job>,
    /// Why the child runs without its Job Object, when it does.
    uncontained: Option<String>,
}

/// Puts the just-started (suspended) child in its Job Object and resumes
/// it. When Windows refuses the job, the child runs without it (see the
/// module docs) and the containment says why; the time limit and the
/// child's own watch on its parent remain. A child that cannot be resumed
/// is killed.
#[cfg_attr(
    not(windows),
    allow(clippy::unnecessary_wraps, clippy::needless_pass_by_ref_mut)
)]
fn contain(child: &mut std::process::Child) -> Result<Containment, String> {
    #[cfg(windows)]
    {
        let job = winjob::Job::new(&winjob::JobLimits {
            max_processes: Some(1),
            memory_limit_bytes: Some(WORKER_MEMORY_BYTES),
        })
        .and_then(|job| job.assign(child).map(|()| job));
        let (job, uncontained) = match job {
            Ok(job) => (Some(job), None),
            Err(error) => (None, Some(error.to_string())),
        };
        if winjob::resume_process(child.id()).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(unsafe_read("the reader could not start"));
        }
        Ok(Containment {
            _job: job,
            uncontained,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = child;
        Ok(Containment { uncontained: None })
    }
}

/// What [`run_child`] got from a child that exited cleanly.
#[derive(Debug)]
pub(crate) struct ChildOutput {
    pub(crate) stdout: Vec<u8>,
    /// Why the child ran without its Windows Job Object (no memory limit,
    /// no kill-on-close), when it did.
    pub(crate) uncontained: Option<String>,
}

/// `text` with a note that the read ran without its memory limit, when
/// `uncontained` says so.
fn with_containment_note(text: String, uncontained: Option<&str>) -> String {
    match uncontained {
        Some(reason) => format!(
            "{text}\n\n[note: this file was read without the reader's memory limit, because Windows refused its job object ({reason}); the time limit still applied]"
        ),
        None => text,
    }
}

/// The worker side: reads one request from stdin, reads the document and
/// writes the reply to stdout. Returns the process exit code. The worker
/// stops itself when it runs [`WORKER_GRACE`] past [`WORKER_TIMEOUT`] or
/// when its stdin closes (the parent went away).
#[must_use]
pub fn run_worker() -> i32 {
    std::thread::spawn(|| {
        std::thread::sleep(WORKER_TIMEOUT + WORKER_GRACE);
        std::process::exit(SELF_STOP_CODE);
    });
    let request = read_request(&mut std::io::stdin().lock());
    if request.is_ok() {
        // The parent holds stdin open until this process is done; it closes
        // early only when the parent has gone, and then so does this reader.
        // (A bad request is answered at once, so it needs no watch.)
        std::thread::spawn(|| {
            let mut stdin = std::io::stdin();
            let mut buffer = [0u8; 256];
            while matches!(stdin.read(&mut buffer), Ok(read) if read > 0) {}
            std::process::exit(SELF_STOP_CODE);
        });
    }
    let reply = match request {
        Ok(request) => {
            let options = Options {
                pages: request.pages.as_deref(),
                rows: request.rows.as_deref(),
                columns: request.columns.as_deref(),
                max_chars: request.max_chars,
            };
            match super::read_document(&path_from_units(request.path), &options) {
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

/// One request as [`encode_request`] wrote it, reading no more than it.
fn read_request(input: &mut impl Read) -> Result<Request, String> {
    let mut length = [0u8; 8];
    input
        .read_exact(&mut length)
        .map_err(|error| format!("no request ({error})"))?;
    let length = u64::from_le_bytes(length);
    if length > MAX_REQUEST_BYTES {
        return Err(format!("the request is {length} bytes, more than allowed"));
    }
    let mut json = Vec::new();
    input
        .take(length)
        .read_to_end(&mut json)
        .map_err(|error| error.to_string())?;
    if json.len() as u64 != length {
        return Err("the request ended early".to_string());
    }
    serde_json::from_slice(&json).map_err(|error| error.to_string())
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
        assert!(String::from_utf8_lossy(&output.stdout).contains("hello"));
        assert!(serde_json::from_slice::<serde_json::Value>(&output.stdout).is_err());
        // The job object is set up here, so nothing says otherwise.
        assert!(output.uncontained.is_none(), "{:?}", output.uncontained);
    }

    #[test]
    fn a_read_without_its_job_object_says_so() {
        let noted = super::with_containment_note("body".to_string(), Some("access denied"));
        assert!(noted.starts_with("body\n\n[note: "), "{noted}");
        assert!(
            noted.contains("without the reader's memory limit"),
            "{noted}"
        );
        assert!(noted.contains("access denied"), "{noted}");
        assert_eq!(
            super::with_containment_note("body".to_string(), None),
            "body"
        );
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
    fn requests_carry_any_path_the_system_can_name() {
        #[cfg(windows)]
        let path = {
            use std::os::windows::ffi::OsStringExt;
            // An unpaired surrogate: no UTF-8 or UTF-16 string holds it.
            std::path::PathBuf::from(std::ffi::OsString::from_wide(&[
                0x43, 0x3A, 0x5C, 0x63, 0xD800, 0x2E, 0x64, 0x6F, 0x63, 0x78,
            ]))
        };
        #[cfg(not(windows))]
        let path = {
            use std::os::unix::ffi::OsStringExt;
            std::path::PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/caf\xe9.docx".to_vec()))
        };
        assert!(path.to_str().is_none(), "not valid Unicode");
        let framed = super::worker_request(&path, Some("2-3"), Some(900)).expect("encodes");
        let request = super::read_request(&mut framed.as_slice()).expect("decodes");
        assert_eq!(super::path_from_units(request.path), path);
        assert_eq!(request.pages.as_deref(), Some("2-3"));
        assert_eq!(request.max_chars, Some(900));
    }

    #[test]
    fn a_request_is_read_to_its_length_and_no_further() {
        let mut framed =
            super::worker_request(std::path::Path::new("a.pdf"), None, None).expect("encodes");
        framed.extend(b"trailing bytes stay unread");
        let mut input = framed.as_slice();
        super::read_request(&mut input).expect("decodes");
        assert_eq!(input, b"trailing bytes stay unread");
        // A short or oversized request is refused, not waited on.
        assert!(super::read_request(&mut &framed[..5]).is_err());
        assert!(super::read_request(&mut &framed[..12]).is_err());
        let mut huge = u64::MAX.to_le_bytes().to_vec();
        huge.extend(b"{}");
        assert!(super::read_request(&mut huge.as_slice())
            .expect_err("too large")
            .contains("more than allowed"));
    }

    #[test]
    fn a_child_that_never_reads_its_input_cannot_block_the_parent() {
        // More than any pipe buffer, to a child that never reads stdin.
        let input = vec![b'x'; 4 * 1024 * 1024];
        let started = Instant::now();
        let error = run_child(
            sleeper(),
            &input,
            Duration::from_millis(500),
            &CancellationToken::new(),
        )
        .expect_err("killed");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert!(error.contains("reading took longer"), "{error}");
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
