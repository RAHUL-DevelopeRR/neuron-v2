//! Live output of a running shell command (`bash`, `PowerShell`).
//!
//! Whoever runs a tool call can install an [`OutputObserver`] around it with
//! [`with_output_observer`]; the shell tools then run their command through
//! [`run_bounded_live`], which hands the observer what the command prints while it
//! runs, coalesced to at most one piece per stream every [`LIVE_OUTPUT_INTERVAL`]
//! and at most [`MAX_LIVE_OUTPUT_CHUNK`] bytes each. The tool's result is unchanged:
//! the model still gets the whole output when the command ends.
//!
//! The observer is per thread: a sub-agent's commands, which run on threads of their
//! own, are not reported to its parent's observer.

use std::cell::RefCell;
use std::io;
use std::process::{Child, Command};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use plugins::subprocess::OutputPipe;
use plugins::subprocess::{self, BoundedOutput};

/// How often, at most, an observer hears more of one command's output.
pub const LIVE_OUTPUT_INTERVAL: Duration = Duration::from_millis(100);

/// The most one piece carries, per stream. A command that prints more between two
/// pieces is reported by its last [`MAX_LIVE_OUTPUT_CHUNK`] bytes: a live view shows
/// the tail anyway, and the result still holds everything.
pub const MAX_LIVE_OUTPUT_CHUNK: usize = 8 * 1024;

/// Hears a running command's output: the stream and the text printed since the last
/// call (UTF-8; invalid bytes become U+FFFD).
pub type OutputObserver = Arc<dyn Fn(OutputPipe, &str) + Send + Sync>;

thread_local! {
    static OBSERVER: RefCell<Option<OutputObserver>> = const { RefCell::new(None) };
}

/// Runs `run` with `observer` hearing the output of the shell commands it runs on
/// this thread. The previous observer, if any, is back once `run` returns (or
/// panics).
pub fn with_output_observer<R>(observer: OutputObserver, run: impl FnOnce() -> R) -> R {
    struct Restore(Option<OutputObserver>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            OBSERVER.with(|slot| *slot.borrow_mut() = previous);
        }
    }
    let previous = OBSERVER.with(|slot| slot.borrow_mut().replace(observer));
    let _restore = Restore(previous);
    run()
}

/// The observer [`with_output_observer`] installed on this thread, if any.
#[must_use]
pub fn output_observer() -> Option<OutputObserver> {
    OBSERVER.with(|slot| slot.borrow().clone())
}

/// [`subprocess::run_bounded`], reporting the output to this thread's observer while
/// the command runs. Without an observer it is exactly `run_bounded`.
pub fn run_bounded_live(
    command: &mut Command,
    stdin: Option<&[u8]>,
    timeout: Option<Duration>,
    is_cancelled: &dyn Fn() -> bool,
) -> io::Result<BoundedOutput> {
    let Some(observer) = output_observer() else {
        return subprocess::run_bounded(command, stdin, timeout, is_cancelled);
    };
    let mut live = LiveOutput::new(observer);
    let result = subprocess::run_bounded_observed(
        command,
        stdin,
        timeout,
        is_cancelled,
        &mut |pipe, bytes| live.push(pipe, bytes, Instant::now()),
    );
    live.finish();
    result
}

/// [`run_bounded_live`] with a hook on the child right after it started (see
/// [`subprocess::run_bounded_with_spawn_hook`]).
pub fn run_bounded_live_with_spawn_hook(
    command: &mut Command,
    stdin: Option<&[u8]>,
    timeout: Option<Duration>,
    is_cancelled: &dyn Fn() -> bool,
    after_spawn: &mut dyn FnMut(&Child) -> io::Result<()>,
) -> io::Result<BoundedOutput> {
    let Some(observer) = output_observer() else {
        return subprocess::run_bounded_with_spawn_hook(
            command,
            stdin,
            timeout,
            is_cancelled,
            after_spawn,
        );
    };
    let mut live = LiveOutput::new(observer);
    let result = subprocess::run_bounded_hooked(
        command,
        stdin,
        timeout,
        is_cancelled,
        after_spawn,
        &mut |pipe, bytes| live.push(pipe, bytes, Instant::now()),
    );
    live.finish();
    result
}

/// Coalesces a command's output into pieces for an [`OutputObserver`].
pub struct LiveOutput {
    observer: OutputObserver,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    last_sent: Option<Instant>,
}

impl LiveOutput {
    #[must_use]
    pub fn new(observer: OutputObserver) -> Self {
        Self {
            observer,
            stdout: Vec::new(),
            stderr: Vec::new(),
            last_sent: None,
        }
    }

    /// More output at `now`: sent at once when the last piece went out at least
    /// [`LIVE_OUTPUT_INTERVAL`] ago (or none did), kept for later otherwise.
    pub fn push(&mut self, pipe: OutputPipe, bytes: &[u8], now: Instant) {
        let pending = match pipe {
            OutputPipe::Stdout => &mut self.stdout,
            OutputPipe::Stderr => &mut self.stderr,
        };
        pending.extend_from_slice(bytes);
        // Only the tail can be sent: keep what is pending small.
        let keep = MAX_LIVE_OUTPUT_CHUNK + 3;
        if pending.len() > keep {
            pending.drain(..pending.len() - keep);
        }
        let due = self
            .last_sent
            .is_none_or(|sent| now.saturating_duration_since(sent) >= LIVE_OUTPUT_INTERVAL);
        if due {
            self.flush(now, false);
        }
    }

    /// Sends whatever is pending: the command ended.
    pub fn finish(&mut self) {
        self.flush(Instant::now(), true);
    }

    fn flush(&mut self, now: Instant, last: bool) {
        let mut sent = false;
        for pipe in [OutputPipe::Stdout, OutputPipe::Stderr] {
            let pending = match pipe {
                OutputPipe::Stdout => &mut self.stdout,
                OutputPipe::Stderr => &mut self.stderr,
            };
            let text = take_text(pending, last);
            if !text.is_empty() {
                (self.observer)(pipe, &text);
                sent = true;
            }
        }
        if sent {
            self.last_sent = Some(now);
        }
    }
}

/// The text `pending` holds, at most its last [`MAX_LIVE_OUTPUT_CHUNK`] bytes. A
/// character cut at the end stays pending until its other bytes come, unless this is
/// the `last` piece.
fn take_text(pending: &mut Vec<u8>, last: bool) -> String {
    let complete = if last {
        pending.len()
    } else {
        match std::str::from_utf8(pending) {
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            // Valid, or invalid bytes that more output cannot complete.
            _ => pending.len(),
        }
    };
    let bytes: Vec<u8> = pending.drain(..complete).collect();
    let text = String::from_utf8_lossy(&bytes);
    if text.len() <= MAX_LIVE_OUTPUT_CHUNK {
        return text.into_owned();
    }
    let mut start = text.len() - MAX_LIVE_OUTPUT_CHUNK;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        output_observer, run_bounded_live, with_output_observer, LiveOutput, OutputObserver,
        OutputPipe, LIVE_OUTPUT_INTERVAL, MAX_LIVE_OUTPUT_CHUNK,
    };
    use std::process::Command;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    type Heard = Arc<Mutex<Vec<(OutputPipe, String)>>>;

    fn recorder() -> (OutputObserver, Heard) {
        let heard: Heard = Arc::default();
        let sink = Arc::clone(&heard);
        let observer: OutputObserver = Arc::new(move |pipe, text: &str| {
            sink.lock().unwrap().push((pipe, text.to_string()));
        });
        (observer, heard)
    }

    #[test]
    fn coalesces_output_to_one_piece_per_interval() {
        let (observer, heard) = recorder();
        let mut live = LiveOutput::new(observer);
        let start = Instant::now();
        live.push(OutputPipe::Stdout, b"one\n", start);
        live.push(
            OutputPipe::Stdout,
            b"two\n",
            start + Duration::from_millis(10),
        );
        live.push(
            OutputPipe::Stderr,
            b"warn\n",
            start + Duration::from_millis(50),
        );
        assert_eq!(
            heard.lock().unwrap().len(),
            1,
            "only the first piece is sent at once"
        );
        live.push(
            OutputPipe::Stdout,
            b"three\n",
            start + LIVE_OUTPUT_INTERVAL + Duration::from_millis(1),
        );
        live.push(
            OutputPipe::Stdout,
            b"four\n",
            start + Duration::from_millis(120),
        );
        live.finish();
        assert_eq!(
            *heard.lock().unwrap(),
            vec![
                (OutputPipe::Stdout, "one\n".to_string()),
                (OutputPipe::Stdout, "two\nthree\n".to_string()),
                (OutputPipe::Stderr, "warn\n".to_string()),
                (OutputPipe::Stdout, "four\n".to_string()),
            ]
        );
    }

    #[test]
    fn caps_a_piece_to_its_tail_and_keeps_split_characters_whole() {
        let (observer, heard) = recorder();
        let mut live = LiveOutput::new(observer);
        let start = Instant::now();
        let big = "x".repeat(MAX_LIVE_OUTPUT_CHUNK * 3) + "END";
        live.push(OutputPipe::Stdout, big.as_bytes(), start);
        // "é" split across two reads.
        let later = start + LIVE_OUTPUT_INTERVAL * 2;
        live.push(OutputPipe::Stdout, &[b'a', 0xC3], later);
        live.push(
            OutputPipe::Stdout,
            &[0xA9, b'\n'],
            later + LIVE_OUTPUT_INTERVAL * 2,
        );
        live.finish();
        let heard = heard.lock().unwrap();
        assert_eq!(heard[0].1.len(), MAX_LIVE_OUTPUT_CHUNK);
        assert!(heard[0].1.ends_with("END"));
        assert_eq!(heard[1].1, "a");
        assert_eq!(heard[2].1, "é\n");
    }

    #[test]
    fn the_observer_is_scoped_to_the_call_and_thread() {
        let (observer, _heard) = recorder();
        assert!(output_observer().is_none());
        with_output_observer(observer, || {
            assert!(output_observer().is_some());
            std::thread::spawn(|| assert!(output_observer().is_none()))
                .join()
                .unwrap();
        });
        assert!(output_observer().is_none());
    }

    #[test]
    fn a_running_command_is_heard_before_it_ends_and_in_full() {
        let script = if cfg!(windows) {
            "echo first & ping -n 2 127.0.0.1 >NUL & echo second"
        } else {
            "echo first; sleep 1; echo second"
        };
        let mut command = if cfg!(windows) {
            let mut command = Command::new("cmd.exe");
            command.args(["/D", "/C", script]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", script]);
            command
        };
        let started = Instant::now();
        let first_at: Arc<Mutex<Option<Duration>>> = Arc::default();
        let heard: Arc<Mutex<String>> = Arc::default();
        let observer: OutputObserver = {
            let first_at = Arc::clone(&first_at);
            let heard = Arc::clone(&heard);
            Arc::new(move |_pipe, text: &str| {
                first_at.lock().unwrap().get_or_insert(started.elapsed());
                heard.lock().unwrap().push_str(text);
            })
        };
        let output = with_output_observer(observer, || {
            run_bounded_live(&mut command, None, Some(Duration::from_secs(20)), &|| false)
        })
        .expect("command runs");
        let total = started.elapsed();
        assert_eq!(
            *heard.lock().unwrap(),
            String::from_utf8_lossy(&output.stdout)
        );
        let first = first_at.lock().unwrap().expect("output heard");
        assert!(
            first + Duration::from_millis(500) < total,
            "first heard at {first:?} of {total:?}"
        );
    }
}
