//! Windows Job Objects for sandboxed shell commands and MCP servers.
//!
//! A [`Job`] is created with `KILL_ON_JOB_CLOSE`, so dropping it kills every
//! process still in it, and optionally an active-process limit (fork-bomb
//! guard) and a job-wide memory limit. Children of a process in a job are in
//! the job too, so a shell's whole tree is contained.
//!
//! To leave no window in which a child can start processes outside the job,
//! spawn it with [`CREATE_SUSPENDED`], [`Job::assign`] it, then
//! [`resume_process`] it.
//!
//! On non-Windows targets this crate exports nothing.

#![deny(unsafe_code)]

/// `CREATE_SUSPENDED` process creation flag (Windows).
pub const CREATE_SUSPENDED: u32 = 0x0000_0004;

/// Limits applied to a [`Job`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JobLimits {
    /// Most processes alive in the job at once; starting another fails.
    pub max_processes: Option<u32>,
    /// Most committed memory, in bytes, for all processes in the job.
    pub memory_limit_bytes: Option<u64>,
}

#[cfg(windows)]
pub use imp::{resume_process, Job};

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::{AsRawHandle, RawHandle};
    use std::process::Child;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
        JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation, QueryInformationJobObject,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
        JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    use super::JobLimits;

    /// An owned handle closed on drop.
    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            // SAFETY: the handle is owned, valid and closed exactly once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    /// A kill-on-close Job Object. Dropping it kills every process in it.
    #[derive(Debug)]
    pub struct Job {
        handle: HANDLE,
    }

    // SAFETY: a job handle is a kernel object handle usable from any thread.
    unsafe impl Send for Job {}
    // SAFETY: every method only passes the handle to thread-safe Win32 calls.
    unsafe impl Sync for Job {}

    impl Job {
        /// Creates an unnamed job with kill-on-close and `limits`.
        pub fn new(limits: &JobLimits) -> io::Result<Self> {
            // SAFETY: null attributes and name create an unnamed job.
            let handle = unsafe { CreateJobObjectW(null(), null()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self { handle };
            // SAFETY: plain-old-data struct; all-zero is a valid value.
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
            let mut flags =
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
            if let Some(max) = limits.max_processes {
                flags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
                info.BasicLimitInformation.ActiveProcessLimit = max.max(1);
            }
            if let Some(bytes) = limits.memory_limit_bytes {
                flags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
                info.JobMemoryLimit = usize::try_from(bytes).unwrap_or(usize::MAX);
            }
            info.BasicLimitInformation.LimitFlags = flags;
            let size = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                .expect("struct size fits u32");
            // SAFETY: `info` is a valid struct of the declared class and size.
            let ok = unsafe {
                SetInformationJobObject(
                    job.handle,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_ref(&info).cast(),
                    size,
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }

        /// Puts `child` (and every process it starts from now on) in the job.
        pub fn assign(&self, child: &Child) -> io::Result<()> {
            self.assign_raw(child.as_raw_handle())
        }

        /// [`Job::assign`] for a process handle from elsewhere (an async runtime's
        /// child). A handle that is not a live process makes the call fail; it is
        /// never closed or kept.
        pub fn assign_raw(&self, process: RawHandle) -> io::Result<()> {
            // SAFETY: the job handle is valid; Windows checks the process handle and
            // fails the call when it is not one.
            let ok = unsafe { AssignProcessToJobObject(self.handle, process.cast()) };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        /// The ids of the processes in the job (at most the first 1024).
        pub fn process_ids(&self) -> io::Result<Vec<u32>> {
            const MAX_IDS: usize = 1024;
            // A `usize` buffer keeps the list aligned: two `u32` counts, then the ids.
            let mut buffer = vec![0_usize; MAX_IDS + 2];
            let size =
                u32::try_from(buffer.len() * size_of::<usize>()).expect("buffer size fits u32");
            let list = buffer
                .as_mut_ptr()
                .cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
            // SAFETY: `list` points at `size` writable, suitably aligned bytes.
            let ok = unsafe {
                QueryInformationJobObject(
                    self.handle,
                    JobObjectBasicProcessIdList,
                    list.cast(),
                    size,
                    null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: Windows filled in the header and `NumberOfProcessIdsInList` ids,
            // which fit the buffer it was given.
            let ids = unsafe {
                let count = ((*list).NumberOfProcessIdsInList as usize).min(MAX_IDS);
                let first = std::ptr::addr_of!((*list).ProcessIdList).cast::<usize>();
                std::slice::from_raw_parts(first, count).to_vec()
            };
            Ok(ids
                .into_iter()
                .filter_map(|id| u32::try_from(id).ok())
                .collect())
        }

        /// How many processes are alive in the job.
        pub fn active_processes(&self) -> io::Result<u32> {
            // SAFETY: plain-old-data struct; all-zero is a valid value.
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { zeroed() };
            let size = u32::try_from(size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>())
                .expect("struct size fits u32");
            // SAFETY: `info` is writable and of the declared class and size.
            let ok = unsafe {
                QueryInformationJobObject(
                    self.handle,
                    JobObjectBasicAccountingInformation,
                    std::ptr::from_mut(&mut info).cast(),
                    size,
                    null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(info.ActiveProcesses)
        }

        /// Kills every process in the job now.
        pub fn terminate(&self, exit_code: u32) -> io::Result<()> {
            // SAFETY: the job handle is valid.
            if unsafe { TerminateJobObject(self.handle, exit_code) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is owned and closed exactly once; closing the
            // last handle kills the job's processes (kill-on-close).
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }

    /// Resumes every thread of process `pid` (one started with
    /// `CREATE_SUSPENDED`). Returns how many threads were resumed.
    pub fn resume_process(pid: u32) -> io::Result<usize> {
        // SAFETY: a thread snapshot of the whole system; checked below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
            return Err(io::Error::last_os_error());
        }
        let snapshot = OwnedHandle(snapshot);
        // SAFETY: plain-old-data struct; all-zero is a valid value.
        let mut entry: THREADENTRY32 = unsafe { zeroed() };
        entry.dwSize = u32::try_from(size_of::<THREADENTRY32>()).expect("struct size fits u32");
        let mut resumed = 0;
        // SAFETY: `entry` is writable with `dwSize` set, as the API requires.
        let mut more = unsafe { Thread32First(snapshot.0, &raw mut entry) } != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                // SAFETY: opening a thread id from the snapshot; checked below.
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if !thread.is_null() {
                    let thread = OwnedHandle(thread);
                    // SAFETY: a valid thread handle with suspend/resume access.
                    if unsafe { ResumeThread(thread.0) } != u32::MAX {
                        resumed += 1;
                    }
                }
            }
            // SAFETY: as for `Thread32First`.
            more = unsafe { Thread32Next(snapshot.0, &raw mut entry) } != 0;
        }
        if resumed == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no thread of process {pid} could be resumed"),
            ));
        }
        Ok(resumed)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::{resume_process, Job, JobLimits, CREATE_SUSPENDED};
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    fn spawn_suspended(script: &str) -> Child {
        Command::new("cmd.exe")
            .args(["/D", "/S", "/C"])
            .raw_arg(format!("\"{script}\""))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_SUSPENDED)
            .spawn()
            .expect("spawn cmd")
    }

    fn wait_with_deadline(child: &mut Child, limit: Duration) -> bool {
        let started = Instant::now();
        while started.elapsed() < limit {
            if child.try_wait().expect("try_wait").is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_suspended_child_runs_after_assign_and_resume() {
        let job = Job::new(&JobLimits::default()).expect("job");
        let child = spawn_suspended("echo contained");
        job.assign(&child).expect("assign");
        resume_process(child.id()).expect("resume");
        let output = child.wait_with_output().expect("wait");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "contained");
    }

    /// Grandchildren are in the job, and dropping the job kills the tree.
    #[test]
    fn grandchildren_are_contained_and_killed_on_close() {
        let job = Job::new(&JobLimits::default()).expect("job");
        let mut child = spawn_suspended("ping -n 30 127.0.0.1 >NUL");
        job.assign(&child).expect("assign");
        resume_process(child.id()).expect("resume");
        let started = Instant::now();
        let mut active = 0;
        while started.elapsed() < Duration::from_secs(10) {
            active = job.active_processes().expect("query");
            if active >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            active >= 2,
            "cmd and its ping should both be in the job: {active}"
        );
        drop(job);
        assert!(
            wait_with_deadline(&mut child, Duration::from_secs(5)),
            "closing the job must kill the tree"
        );
    }

    #[test]
    fn process_ids_lists_the_tree() {
        let job = Job::new(&JobLimits::default()).expect("job");
        let mut child = spawn_suspended("ping -n 30 127.0.0.1 >NUL");
        job.assign_raw(std::os::windows::io::AsRawHandle::as_raw_handle(&child))
            .expect("assign");
        resume_process(child.id()).expect("resume");
        let started = Instant::now();
        let mut ids = Vec::new();
        while started.elapsed() < Duration::from_secs(10) {
            ids = job.process_ids().expect("query");
            if ids.len() >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(ids.contains(&child.id()), "{ids:?}");
        assert!(ids.len() >= 2, "cmd and its ping: {ids:?}");
        drop(job);
        assert!(wait_with_deadline(&mut child, Duration::from_secs(5)));
    }

    #[test]
    fn the_active_process_limit_stops_a_second_process() {
        let job = Job::new(&JobLimits {
            max_processes: Some(1),
            memory_limit_bytes: None,
        })
        .expect("job");
        let mut child = spawn_suspended("ping -n 30 127.0.0.1 >NUL");
        job.assign(&child).expect("assign");
        resume_process(child.id()).expect("resume");
        assert!(
            wait_with_deadline(&mut child, Duration::from_secs(10)),
            "ping could not start, so cmd ends at once"
        );
        let status = child.wait().expect("status");
        assert!(!status.success(), "starting ping must fail: {status:?}");
    }

    #[test]
    fn terminate_kills_everything_now() {
        let job = Job::new(&JobLimits {
            max_processes: None,
            memory_limit_bytes: Some(512 * 1024 * 1024),
        })
        .expect("job with memory limit");
        let mut child = spawn_suspended("ping -n 30 127.0.0.1 >NUL");
        job.assign(&child).expect("assign");
        resume_process(child.id()).expect("resume");
        job.terminate(7).expect("terminate");
        assert!(wait_with_deadline(&mut child, Duration::from_secs(5)));
        assert_eq!(child.wait().expect("status").code(), Some(7));
    }
}
