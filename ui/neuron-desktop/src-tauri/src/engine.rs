//! `neuron engine --stdio` as a child of the app: client messages go to its
//! stdin, and every stdout line (one engine event) goes to the window as is.
//! The bridge reads nothing into the protocol; the UI parses the lines. It has
//! no Tauri types, so it is tested on its own against the mock service.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// How long a shutdown waits for the engine to save and exit before killing it.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Where the `neuron` binary is: `NEURON_BIN`, beside the app's executable,
/// then the PATH.
pub fn resolve_binary() -> Option<PathBuf> {
    crate::diagnostics::resolve_from_environment().1 // L5: one lookup, with its source
}

#[derive(Debug, Clone, Default)]
pub struct Launch {
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub resume: Option<String>,
    /// L3: `low`, `medium` or `high`; the engine's own default when `None`.
    pub reasoning_effort: Option<String>,
}

pub fn engine_args(launch: &Launch) -> Vec<String> {
    let mut args = vec!["engine".to_string(), "--stdio".to_string()];
    for (flag, value) in [
        ("--model", &launch.model),
        ("--permission-mode", &launch.permission_mode),
        ("--resume", &launch.resume),
        ("--reasoning-effort", &launch.reasoning_effort),
    ] {
        if let Some(value) = value {
            args.push(flag.to_string());
            args.push(value.clone());
        }
    }
    args
}

/// The engine's environment.
#[derive(Debug, Clone)]
pub enum Env {
    /// This process's environment, plus these.
    Inherit(Vec<(String, String)>),
    /// Only these (tests: nothing of the developer's own keys or login).
    Only(Vec<(String, String)>),
}

/// How the engine ended: its exit code, and the reason when it failed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EngineExit {
    pub code: Option<i32>,
    pub error: Option<String>,
}

/// What the registry needs of a running engine, so tests can swap in a fake.
pub trait EngineHandle: Send + Sync {
    /// Writes one client message (a JSON line).
    fn send(&self, line: &str) -> Result<(), String>;
    /// Asks the engine to save and stop; it is ended after a grace period.
    fn stop(&self);
    /// Ends the engine and every process it started.
    fn kill_tree(&self);
    /// Whether the engine process is still there.
    fn running(&self) -> bool;
}

/// The session id in a `status` line, if it is one.
pub fn status_session(line: &str) -> Option<String> {
    if !line.contains("\"status\"") {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("type")?.as_str()? != "status" {
        return None;
    }
    value
        .get("session_id")?
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// Ends the process `pid` and everything it started. On Windows `taskkill /T`
/// walks the tree; elsewhere the engine leads its own process group, which is
/// signalled as a whole. No shell is involved.
pub fn kill_tree_pid(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-KILL", "--", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// One Windows Job Object for the whole app, set to kill its processes when the
/// app's last handle closes: an app that crashes takes its engines (and their
/// children) with it.
#[cfg(windows)]
mod job {
    use std::os::windows::io::RawHandle;
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    // A HANDLE is a raw pointer; it is only ever passed back to the OS.
    struct Job(isize);

    fn job() -> Option<&'static Job> {
        static JOB: OnceLock<Option<Job>> = OnceLock::new();
        JOB.get_or_init(|| unsafe {
            let handle: HANDLE = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(info).cast(),
                u32::try_from(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                    .unwrap_or(0),
            );
            (set != 0).then_some(Job(handle as isize))
        })
        .as_ref()
    }

    /// Puts the process into the app's job. A failure only loses the cleanup
    /// after a crash.
    pub fn assign(process: RawHandle) {
        if let Some(job) = job() {
            unsafe {
                AssignProcessToJobObject(job.0 as HANDLE, process as HANDLE);
            }
        }
    }
}

/// Client messages waiting for the engine to read them.
const WRITE_QUEUE: usize = 64;

/// Writes to an engine's stdin from a thread of its own, fed by a bounded
/// channel, so an engine that stops reading never holds a lock or a command:
/// `send` only queues, and says so when the queue is full.
struct StdinWriter {
    queue: Mutex<Option<SyncSender<String>>>,
}

impl StdinWriter {
    fn new(mut pipe: impl Write + Send + 'static) -> Self {
        let (queue, lines) = sync_channel::<String>(WRITE_QUEUE);
        thread::spawn(move || {
            // Ends when the queue is closed (after what is queued is written)
            // or when the pipe breaks.
            for line in lines {
                let written = pipe
                    .write_all(line.as_bytes())
                    .and_then(|()| pipe.write_all(b"\n"))
                    .and_then(|()| pipe.flush());
                if written.is_err() {
                    break;
                }
            }
        });
        Self {
            queue: Mutex::new(Some(queue)),
        }
    }

    fn send(&self, line: &str) -> Result<(), String> {
        if line.contains('\n') {
            return Err("a client message is one line".into());
        }
        let queue = lock(&self.queue);
        let queue = queue.as_ref().ok_or("The Neuron engine is not running.")?;
        queue
            .try_send(line.to_string())
            .map_err(|error| match error {
                TrySendError::Full(_) => {
                    "The Neuron engine is not keeping up. Try again in a moment.".to_string()
                }
                TrySendError::Disconnected(_) => "The Neuron engine is not running.".to_string(),
            })
    }

    /// No more messages; what is queued is still written.
    fn close(&self) {
        lock(&self.queue).take();
    }
}

/// Runs something once, the first time it is asked to.
#[derive(Default)]
struct Once(AtomicBool);

impl Once {
    /// `true` for the first call only.
    fn first(&self) -> bool {
        !self.0.swap(true, Ordering::SeqCst)
    }
}

/// A running engine.
pub struct EngineProcess {
    stdin: Option<StdinWriter>,
    pid: u32,
    exited: Arc<AtomicBool>,
    /// The grace-kill thread starts once, however often `stop` is called.
    grace: Once,
}

impl EngineProcess {
    /// Starts the engine in `cwd`. `on_line` gets each stdout line; `on_exit`
    /// runs once, after the last line.
    pub fn spawn(
        binary: &Path,
        cwd: &Path,
        launch: &Launch,
        env: &Env,
        on_line: impl Fn(String) + Send + 'static,
        on_exit: impl FnOnce(EngineExit) + Send + 'static,
    ) -> Result<Self, String> {
        let mut command = Command::new(binary);
        command
            .args(engine_args(launch))
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let vars = match env {
            Env::Inherit(vars) => vars,
            Env::Only(vars) => {
                command.env_clear();
                vars
            }
        };
        command.envs(vars.iter().map(|(key, value)| (key, value)));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // No console window for the engine.
            command.creation_flags(0x0800_0000);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Its own process group, so the whole tree can be signalled.
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .map_err(|error| crate::diagnostics::classify_spawn_error(&error, binary).render())?; // L5
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            job::assign(child.as_raw_handle());
        }
        let pid = child.id();
        let stdin = child.stdin.take().map(StdinWriter::new);
        let stdout = child.stdout.take().ok_or("the engine has no stdout")?;
        let stderr = child.stderr.take().ok_or("the engine has no stderr")?;
        let exited = Arc::new(AtomicBool::new(false));

        // Some launch checks print `error: ...` on stderr and exit without an event.
        let last_error = Arc::new(Mutex::new(None::<String>));
        let stderr_error = Arc::clone(&last_error);
        let stderr_thread = thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Some(message) = line.trim().strip_prefix("error:") {
                    *lock(&stderr_error) = Some(message.trim().to_string());
                }
            }
        });

        let done = Arc::clone(&exited);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let trimmed = line.trim_end_matches(['\r', '\n']);
                        if !trimmed.is_empty() {
                            on_line(trimmed.to_string());
                        }
                    }
                }
            }
            // Drain whatever is left so the child never blocks on a full pipe.
            let _ = reader.read_to_end(&mut Vec::new());
            let code = child.wait().ok().and_then(|status| status.code());
            done.store(true, Ordering::SeqCst);
            // An MCP server the engine started can keep stderr open; don't wait for it.
            for _ in 0..10 {
                if stderr_thread.is_finished() {
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
            let error = crate::diagnostics::exit_error(code, lock(&last_error).as_deref()); // L5
            on_exit(EngineExit { code, error });
        });

        Ok(Self {
            stdin,
            pid,
            exited,
            grace: Once::default(),
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Writes one client message (a JSON line) to the engine.
    pub fn send(&self, line: &str) -> Result<(), String> {
        self.stdin
            .as_ref()
            .ok_or("The Neuron engine is not running.")?
            .send(line)
    }

    /// Asks the engine to save and stop (a shutdown and the end of its input),
    /// and ends it, with what it started, if it hasn't exited after a grace period.
    pub fn stop(&self) {
        let _ = self.send(r#"{"kind":"command","type":"shutdown"}"#);
        if let Some(stdin) = &self.stdin {
            stdin.close();
        }
        if !self.grace.first() {
            return;
        }
        let (pid, exited) = (self.pid, Arc::clone(&self.exited));
        thread::spawn(move || {
            thread::sleep(SHUTDOWN_GRACE);
            if !exited.load(Ordering::SeqCst) {
                kill_tree_pid(pid);
            }
        });
    }

    /// Ends the engine and its whole process tree at once.
    pub fn kill_tree(&self) {
        if let Some(stdin) = &self.stdin {
            stdin.close();
        }
        if !self.exited.load(Ordering::SeqCst) {
            kill_tree_pid(self.pid);
        }
    }

    pub fn running(&self) -> bool {
        !self.exited.load(Ordering::SeqCst)
    }
}

impl EngineHandle for EngineProcess {
    fn send(&self, line: &str) -> Result<(), String> {
        Self::send(self, line)
    }
    fn stop(&self) {
        Self::stop(self);
    }
    fn kill_tree(&self) {
        Self::kill_tree(self);
    }
    fn running(&self) -> bool {
        Self::running(self)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// A pipe nobody reads: every write waits for the flag.
    struct Stalled(Arc<AtomicBool>);

    impl Write for Stalled {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            while !self.0.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(5));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_stalled_pipe_never_blocks_send_or_close() {
        let release = Arc::new(AtomicBool::new(false));
        let writer = StdinWriter::new(Stalled(Arc::clone(&release)));
        let started = std::time::Instant::now();
        let mut refused = 0;
        for _ in 0..(WRITE_QUEUE * 3) {
            if writer.send(r#"{"kind":"command"}"#).is_err() {
                refused += 1;
            }
        }
        writer.close();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(refused > 0, "the queue is bounded");
        assert!(writer.send("{}").unwrap_err().contains("not running"));
        release.store(true, Ordering::SeqCst);
    }

    #[test]
    fn queued_messages_are_written_in_order_then_the_pipe_closes() {
        #[derive(Clone, Default)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                lock(&self.0).extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let sink = Sink::default();
        let writer = StdinWriter::new(sink.clone());
        writer.send("one").unwrap();
        writer.send("two").unwrap();
        assert!(writer.send("a\nb").is_err());
        writer.close();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while lock(&sink.0).len() < 8 && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(lock(&sink.0).as_slice(), b"one\ntwo\n");
    }

    #[test]
    fn the_grace_kill_starts_once() {
        let once = Once::default();
        assert!(once.first());
        assert!(!once.first());
        assert!(!once.first());
    }

    #[test]
    fn launch_flags_carry_the_reasoning_effort() {
        let launch = Launch {
            reasoning_effort: Some("high".into()),
            ..Launch::default()
        };
        assert_eq!(
            engine_args(&launch),
            ["engine", "--stdio", "--reasoning-effort", "high"]
        );
    }

    #[test]
    fn launch_flags() {
        assert_eq!(engine_args(&Launch::default()), ["engine", "--stdio"]);
        let launch = Launch {
            model: Some("sonnet".into()),
            permission_mode: Some("read-only".into()),
            resume: None,
            ..Launch::default()
        };
        assert_eq!(
            engine_args(&launch),
            [
                "engine",
                "--stdio",
                "--model",
                "sonnet",
                "--permission-mode",
                "read-only"
            ]
        );
    }

    fn process_alive(pid: u32) -> bool {
        #[cfg(windows)]
        {
            let out = Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
                .output()
                .expect("tasklist");
            String::from_utf8_lossy(&out.stdout).contains(&format!(",\"{pid}\","))
        }
        #[cfg(unix)]
        {
            Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        }
    }

    /// A process that starts a child of its own: ending the parent's tree ends
    /// the child too (a plain `Child::kill` would leave it running).
    #[test]
    fn kill_tree_ends_the_children_too() {
        #[cfg(windows)]
        let mut parent = Command::new("powershell.exe");
        #[cfg(windows)]
        parent.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$p = Start-Process ping -ArgumentList '-n','120','127.0.0.1' -PassThru -WindowStyle Hidden; [Console]::Out.WriteLine($p.Id); [Console]::Out.Flush(); Start-Sleep 120",
        ]);
        #[cfg(unix)]
        let mut parent = Command::new("sh");
        #[cfg(unix)]
        parent.args(["-c", "sleep 120 & echo $!; wait"]);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            parent.process_group(0);
        }
        let mut parent = parent
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("a parent process");
        let mut first = String::new();
        BufReader::new(parent.stdout.take().unwrap())
            .read_line(&mut first)
            .unwrap();
        let child: u32 = first.trim().parse().expect("the child's pid");
        assert!(process_alive(child), "the child should be running");

        kill_tree_pid(parent.id());
        let _ = parent.wait();
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while process_alive(child) {
            assert!(
                std::time::Instant::now() < deadline,
                "the child survived kill_tree"
            );
            thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn a_failed_launch_reports_why() {
        let (tx, rx) = mpsc::channel();
        let result = EngineProcess::spawn(
            Path::new("definitely-not-a-neuron-binary"),
            &std::env::temp_dir(),
            &Launch::default(),
            &Env::Inherit(Vec::new()),
            |_| {},
            move |end| drop(tx.send(end)),
        );
        assert!(result.err().unwrap().starts_with("[engine:missing] "));
        assert!(rx.try_recv().is_err());
    }
}
