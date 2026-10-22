//! Terminal panes: the user's shell in a PTY (ConPTY on Windows), started in
//! the workspace. Output goes to `on_output` as UTF-8 text, split only on
//! character boundaries. A thread waits for the shell: on Windows the output
//! pipe only ends when the pseudoconsole closes, not when the shell exits.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};

const MAX_TERMINALS: usize = 8;

struct Terminal {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

#[derive(Default)]
pub struct Terminals {
    next: AtomicU32,
    open: Arc<Mutex<HashMap<u32, Terminal>>>,
}

fn shell() -> CommandBuilder {
    if cfg!(windows) {
        let mut command = CommandBuilder::new("powershell.exe");
        command.arg("-NoLogo");
        command
    } else {
        let mut command =
            CommandBuilder::new(std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into()));
        command.arg("-l");
        command
    }
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.clamp(2, 1000),
        cols: cols.clamp(2, 1000),
        pixel_width: 0,
        pixel_height: 0,
    }
}

impl Terminals {
    pub fn open(
        &self,
        cwd: &Path,
        cols: u16,
        rows: u16,
        on_output: impl Fn(u32, String) + Send + 'static,
        on_exit: impl FnOnce(u32, Option<u32>) + Send + 'static,
    ) -> Result<u32, String> {
        if lock(&self.open).len() >= MAX_TERMINALS {
            return Err("Too many terminals open.".into());
        }
        let pair = native_pty_system()
            .openpty(size(cols, rows))
            .map_err(|error| error.to_string())?;
        let mut command = shell();
        command.cwd(cwd);
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| error.to_string())?;
        drop(pair.slave);
        let killer = child.clone_killer();
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| error.to_string())?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| error.to_string())?;
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        lock(&self.open).insert(
            id,
            Terminal {
                master: pair.master,
                writer,
                killer,
            },
        );

        let output = thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            let mut pending = Vec::new();
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        pending.extend_from_slice(&buffer[..read]);
                        let complete = utf8_prefix_len(&pending);
                        if complete > 0 {
                            on_output(
                                id,
                                String::from_utf8_lossy(&pending[..complete]).into_owned(),
                            );
                            pending.drain(..complete);
                        }
                    }
                }
            }
        });
        let open = Arc::clone(&self.open);
        thread::spawn(move || {
            let code = child.wait().ok().map(|status| status.exit_code());
            // Closing the pseudoconsole ends the output; the last of it goes out first.
            drop(lock(&open).remove(&id));
            let _ = output.join();
            on_exit(id, code);
        });
        Ok(id)
    }

    pub fn write(&self, id: u32, data: &str) -> Result<(), String> {
        let mut open = lock(&self.open);
        let terminal = open.get_mut(&id).ok_or("No such terminal.")?;
        terminal
            .writer
            .write_all(data.as_bytes())
            .and_then(|()| terminal.writer.flush())
            .map_err(|error| error.to_string())
    }

    pub fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<(), String> {
        let open = lock(&self.open);
        let terminal = open.get(&id).ok_or("No such terminal.")?;
        terminal
            .master
            .resize(size(cols, rows))
            .map_err(|error| error.to_string())
    }

    pub fn close(&self, id: u32) {
        if let Some(terminal) = lock(&self.open).get_mut(&id) {
            let _ = terminal.killer.kill();
        }
    }

    pub fn close_all(&self) {
        for terminal in lock(&self.open).values_mut() {
            let _ = terminal.killer.kill();
        }
    }
}

/// How many leading bytes of `bytes` are whole UTF-8 characters (a split
/// character waits for the next read; invalid bytes go through).
fn utf8_prefix_len(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Ok(_) => bytes.len(),
        Err(error) if error.error_len().is_none() => error.valid_up_to(),
        Err(_) => bytes.len(),
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
    use std::time::{Duration, Instant};

    #[test]
    fn split_characters_wait_for_their_rest() {
        let euro = "€".as_bytes();
        assert_eq!(utf8_prefix_len(b"abc"), 3);
        assert_eq!(utf8_prefix_len(&[b'a', euro[0], euro[1]]), 1);
        assert_eq!(utf8_prefix_len(&[0xff, b'a']), 2);
    }

    #[test]
    fn runs_a_shell_command_in_the_workspace() {
        let terminals = Terminals::default();
        let (out_tx, out) = mpsc::channel::<String>();
        let (exit_tx, exit) = mpsc::channel::<Option<u32>>();
        let cwd = std::env::temp_dir();
        let id = terminals
            .open(
                &cwd,
                100,
                30,
                move |_, text| drop(out_tx.send(text)),
                move |_, code| drop(exit_tx.send(code)),
            )
            .expect("a terminal");
        terminals.resize(id, 120, 40).unwrap();
        terminals.write(id, "echo neuron-pty-$((40+2))\r").unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut seen = String::new();
        let mut answered = false;
        // PowerShell evaluates $((40+2)) as 42 too.
        while !seen.contains("neuron-pty-42") {
            assert!(Instant::now() < deadline, "no echo in: {seen:?}");
            if let Ok(text) = out.recv_timeout(Duration::from_millis(200)) {
                seen.push_str(&text);
            }
            // ConPTY asks where the cursor is and waits for the answer; xterm.js gives it
            // in the app, the test does here.
            if !answered && seen.contains("\x1b[6n") {
                terminals.write(id, "\x1b[1;1R").unwrap();
                answered = true;
            }
        }
        terminals.write(id, "exit\r").unwrap();
        exit.recv_timeout(Duration::from_secs(20))
            .expect("the shell exits");
        assert!(terminals.write(id, "x").is_err());
    }
}
