//! The working directory tools resolve paths against, per thread.
//!
//! A sub-agent that runs in a git worktree of its own (the `Agent` tool's
//! `isolation: "worktree"`) runs on a thread of its own in the same process, so it
//! cannot change the process's directory without moving the parent too. Its thread
//! sets a directory here instead; the file tools, the shell tools and the permission
//! checks read [`current_dir`], which is that directory on such a thread and the
//! process's directory everywhere else.

use std::cell::RefCell;
use std::io;
use std::path::PathBuf;

thread_local! {
    static THREAD_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// The directory paths resolve against on this thread: the one set with
/// [`set_thread_dir`], else the process's current directory.
pub fn current_dir() -> io::Result<PathBuf> {
    match thread_dir() {
        Some(dir) => Ok(dir),
        None => std::env::current_dir(),
    }
}

/// The directory set for this thread, if any.
#[must_use]
pub fn thread_dir() -> Option<PathBuf> {
    THREAD_DIR.with(|dir| dir.borrow().clone())
}

/// Sets this thread's directory until the guard drops, which puts back the one
/// before.
#[must_use = "the directory is reset when the guard drops"]
pub fn set_thread_dir(dir: PathBuf) -> ThreadDirGuard {
    let previous = THREAD_DIR.with(|slot| slot.replace(Some(dir)));
    ThreadDirGuard { previous }
}

/// Puts back the thread's earlier directory when dropped.
#[derive(Debug)]
pub struct ThreadDirGuard {
    previous: Option<PathBuf>,
}

impl Drop for ThreadDirGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        THREAD_DIR.with(|slot| {
            *slot.borrow_mut() = previous;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{current_dir, set_thread_dir, thread_dir};
    use std::path::PathBuf;

    #[test]
    fn a_thread_directory_applies_to_its_thread_only_and_resets() {
        let process = std::env::current_dir().expect("cwd");
        assert_eq!(thread_dir(), None);
        {
            let _outer = set_thread_dir(PathBuf::from("outer"));
            assert_eq!(current_dir().expect("dir"), PathBuf::from("outer"));
            {
                let _inner = set_thread_dir(PathBuf::from("inner"));
                assert_eq!(current_dir().expect("dir"), PathBuf::from("inner"));
            }
            assert_eq!(current_dir().expect("dir"), PathBuf::from("outer"));
            let other = std::thread::spawn(|| current_dir().expect("dir"))
                .join()
                .expect("thread");
            assert_eq!(other, process);
        }
        assert_eq!(current_dir().expect("dir"), process);
    }
}
