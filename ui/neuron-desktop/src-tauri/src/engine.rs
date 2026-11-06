//! `neuron engine --stdio` as a child of the app: client messages go to its
//! stdin, and every stdout line (one engine event) goes to the window as is.
//! The bridge reads nothing into the protocol; the UI parses the lines. It has
//! no Tauri types, so it is tested on its own against the mock service.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// How long a shutdown waits for the engine to save and exit before killing it.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Where the `neuron` binary is: `NEURON_BIN`, beside the app's executable,
/// then the PATH.
pub fn resolve_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("NEURON_BIN").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let name = if cfg!(windows) {
        "neuron.exe"
    } else {
        "neuron"
    };
    if let Some(sibling) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
    {
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}

#[derive(Debug, Clone, Default)]
pub struct Launch {
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub resume: Option<String>,
}

pub fn engine_args(launch: &Launch) -> Vec<String> {
    let mut args = vec!["engine".to_string(), "--stdio".to_string()];
    for (flag, value) in [
        ("--model", &launch.model),
        ("--permission-mode", &launch.permission_mode),
        ("--resume", &launch.resume),
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

/// A running engine.
pub struct EngineProcess {
    stdin: Mutex<Option<ChildStdin>>,
    pid: u32,
    exited: Arc<AtomicBool>,
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
            .map_err(|error| format!("Could not start {}: {error}", binary.display()))?;
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            job::assign(child.as_raw_handle());
        }
        let pid = child.id();
        let stdin = child.stdin.take();
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
            let error = lock(&last_error).clone().or_else(|| {
                (code != Some(0)).then(|| {
                    format!(
                        "The Neuron engine stopped (exit code {}).",
                        code.map_or("unknown".into(), |c| c.to_string())
                    )
                })
            });
            on_exit(EngineExit { code, error });
        });

        Ok(Self {
            stdin: Mutex::new(stdin),
            pid,
            exited,
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Writes one client message (a JSON line) to the engine.
    pub fn send(&self, line: &str) -> Result<(), String> {
        if line.contains('\n') {
            return Err("a client message is one line".into());
        }
        let mut stdin = lock(&self.stdin);
        let pipe = stdin.as_mut().ok_or("The Neuron engine is not running.")?;
        pipe.write_all(line.as_bytes())
            .and_then(|()| pipe.write_all(b"\n"))
            .and_then(|()| pipe.flush())
            .map_err(|error| format!("The Neuron engine is not running ({error})."))
    }

    /// Asks the engine to save and stop (a shutdown and the end of its input),
    /// and ends it, with what it started, if it hasn't exited after a grace period.
    pub fn stop(&self) {
        let _ = self.send(r#"{"kind":"command","type":"shutdown"}"#);
        lock(&self.stdin).take();
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
        lock(&self.stdin).take();
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

    fn debug_binary(name: &str) -> PathBuf {
        let exe = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_string()
        };
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../rust/target/debug")
            .join(exe)
    }

    #[test]
    fn launch_flags() {
        assert_eq!(engine_args(&Launch::default()), ["engine", "--stdio"]);
        let launch = Launch {
            model: Some("sonnet".into()),
            permission_mode: Some("read-only".into()),
            resume: None,
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

    /// The real engine against the mock Anthropic service: a turn whose bash
    /// call needs approval. Skipped unless `cargo build -p rusty-claude-cli -p
    /// mock-anthropic-service` ran in rust/.
    #[test]
    fn runs_a_turn_with_an_approval_against_the_mock_service() {
        let (neuron, mock) = (
            debug_binary("neuron"),
            debug_binary("mock-anthropic-service"),
        );
        if !neuron.is_file() || !mock.is_file() {
            eprintln!("skipped: build the engine and the mock service in rust/ first");
            return;
        }
        let mut service = Command::new(&mock)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("mock");
        let mut first = String::new();
        let mut out = BufReader::new(service.stdout.take().unwrap());
        while !first.contains("MOCK_ANTHROPIC_BASE_URL=") {
            first.clear();
            assert!(out.read_line(&mut first).unwrap() > 0, "the mock exited");
        }
        let base_url = first.trim().split_once('=').unwrap().1.to_string();

        let home =
            std::env::temp_dir().join(format!("neuron-desktop-bridge-{}", std::process::id()));
        let workspace = home.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(home.join("config")).unwrap();
        // npx would download the bundled MCP servers into this fresh home.
        std::fs::write(
            home.join("config/settings.json"),
            r#"{"defaultMcpServers":false}"#,
        )
        .unwrap();
        let home_str = home.to_string_lossy().to_string();
        let mut vars: Vec<(String, String)> = [
            "PATH",
            "SystemRoot",
            "SystemDrive",
            "TEMP",
            "TMP",
            "PATHEXT",
            "ComSpec",
        ]
        .iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| (name.to_string(), value))
        })
        .collect();
        vars.extend([
            ("HOME".into(), home_str.clone()),
            ("USERPROFILE".into(), home_str.clone()),
            ("NEURON_CONFIG_HOME".into(), format!("{home_str}/config")),
            ("NEURON_HOME".into(), format!("{home_str}/neuron")),
            ("NEURON_CREDENTIAL_STORE".into(), "file".into()),
            ("NEURON_NO_BROWSER".into(), "1".into()),
            ("ANTHROPIC_API_KEY".into(), "mock-service-key".into()),
            ("ANTHROPIC_BASE_URL".into(), base_url),
        ]);
        let env = Env::Only(vars);

        let (lines_tx, lines) = mpsc::channel::<String>();
        let (exit_tx, exit) = mpsc::channel::<EngineExit>();
        let launch = Launch {
            model: Some("sonnet".into()),
            permission_mode: Some("workspace-write".into()),
            resume: None,
        };
        let engine = EngineProcess::spawn(
            &neuron,
            &workspace,
            &launch,
            &env,
            move |line| drop(lines_tx.send(line)),
            move |end| drop(exit_tx.send(end)),
        )
        .expect("spawn");

        let next = || {
            let line = lines
                .recv_timeout(Duration::from_secs(60))
                .expect("an event");
            serde_json::from_str::<serde_json::Value>(&line).expect("a JSON line")
        };
        assert_eq!(next()["type"], "ready");
        engine
            .send(r#"{"kind":"command","type":"submit","text":"PARITY_SCENARIO:bash_permission_prompt_approved","attachments":[]}"#)
            .unwrap();
        let mut seen = Vec::new();
        loop {
            let event = next();
            let kind = event["type"].as_str().unwrap().to_string();
            if kind == "permission_requested" {
                let reply = format!(
                    r#"{{"kind":"permission_reply","id":{},"decision":{{"kind":"allow_once"}}}}"#,
                    event["id"]
                );
                engine.send(&reply).unwrap();
            }
            if kind == "tool_result" {
                assert_eq!(event["is_error"], false);
            }
            seen.push(kind.clone());
            if kind == "turn_finished" {
                break;
            }
        }
        assert!(
            seen.contains(&"permission_requested".to_string()),
            "{seen:?}"
        );
        assert!(seen.contains(&"tool_result".to_string()), "{seen:?}");
        assert_eq!(
            std::fs::read_to_string(workspace.join("approved.txt")).unwrap(),
            "approved via prompt"
        );

        engine.stop();
        let end = exit
            .recv_timeout(Duration::from_secs(30))
            .expect("the engine exits");
        assert_eq!(
            end,
            EngineExit {
                code: Some(0),
                error: None
            }
        );
        let _ = service.kill();
        let _ = service.wait();
        let _ = std::fs::remove_dir_all(&home);
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
        assert!(result.err().unwrap().starts_with("Could not start"));
        assert!(rx.try_recv().is_err());
    }
}
