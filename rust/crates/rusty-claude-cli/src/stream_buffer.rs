//! Buffered token renderer for NeuronCLI
//!
//! Batches terminal output at 30ms intervals to prevent rerender storms
//! and cursor jitter. Also provides backpressure: if the renderer falls
//! behind, old frames are coalesced.

use std::io::{self, Write};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

/// Frame interval for batched rendering. 30ms = ~33 FPS.
const FRAME_INTERVAL_MS: u64 = 30;

/// Maximum buffer size before emergency flush (bytes).
const BUFFER_FLUSH_THRESHOLD: usize = 2048;

/// Buffered renderer that batches writes to stdout.
pub struct StreamBuffer {
    sender: Sender<String>,
    _thread: thread::JoinHandle<()>,
}

impl StreamBuffer {
    /// Spawn a background thread that batches stdout writes.
    pub fn new() -> Self {
        let (sender, receiver): (Sender<String>, Receiver<String>) = channel();

        let thread = thread::spawn(move || {
            let mut stdout = io::stdout();
            let mut buffer = String::with_capacity(1024);
            let mut last_flush = Instant::now();
            let frame_interval = Duration::from_millis(FRAME_INTERVAL_MS);

            loop {
                // Try to collect all pending tokens within the frame interval
                let deadline = last_flush + frame_interval;
                let now = Instant::now();

                let wait = if deadline > now {
                    deadline - now
                } else {
                    Duration::ZERO
                };

                match receiver.recv_timeout(wait) {
                    Ok(token) => {
                        buffer.push_str(&token);
                    }
                    Err(_) => {
                        // Timeout or channel closed — flush what we have
                    }
                }

                // Drain any additional queued messages
                while let Ok(token) = receiver.try_recv() {
                    buffer.push_str(&token);
                }

                let should_flush = !buffer.is_empty()
                    && (last_flush.elapsed() >= frame_interval
                        || buffer.len() >= BUFFER_FLUSH_THRESHOLD);

                if should_flush {
                    stdout.write_all(buffer.as_bytes()).ok();
                    stdout.flush().ok();
                    buffer.clear();
                    last_flush = Instant::now();
                }

                // Check if sender dropped and buffer empty
                if receiver.try_recv().is_err() && buffer.is_empty() {
                    // Verify channel is actually disconnected
                    if let Err(TryRecvError::Disconnected) = receiver.try_recv() {
                        break;
                    }
                }
            }
        });

        Self {
            sender,
            _thread: thread,
        }
    }

    /// Send a token chunk to be rendered.
    pub fn send(&self, token: impl Into<String>) {
        let _ = self.sender.send(token.into());
    }

    /// Flush remaining buffer and shut down.
    pub fn shutdown(self) {
        drop(self.sender);
        // Thread will exit after draining remaining buffer
        let _ = self._thread.join();
    }
}

/// Synchronous wrapper that buffers markdown-rendered output.
/// Used inside the existing streaming loop to batch `write_all` calls.
///
/// Bytes written through [`io::Write`] may split a UTF-8 sequence across two
/// calls; the incomplete tail is held back until the rest arrives so
/// multi-byte characters (emoji, CJK) are never garbled.
pub struct SyncRenderBuffer {
    buffer: String,
    pending_bytes: Vec<u8>,
    last_flush: Instant,
    frame_interval: Duration,
    stdout: io::Stdout,
}

impl SyncRenderBuffer {
    pub fn new() -> Self {
        Self {
            buffer: String::with_capacity(1024),
            pending_bytes: Vec::new(),
            last_flush: Instant::now(),
            frame_interval: Duration::from_millis(FRAME_INTERVAL_MS),
            stdout: io::stdout(),
        }
    }

    /// Appends raw bytes, decoding complete UTF-8 sequences and keeping an
    /// incomplete trailing sequence for the next call. Invalid bytes become
    /// U+FFFD.
    fn push_bytes(&mut self, bytes: &[u8]) {
        self.pending_bytes.extend_from_slice(bytes);
        let mut consumed = 0;
        loop {
            match std::str::from_utf8(&self.pending_bytes[consumed..]) {
                Ok(text) => {
                    self.buffer.push_str(text);
                    consumed = self.pending_bytes.len();
                    break;
                }
                Err(error) => {
                    let valid_end = consumed + error.valid_up_to();
                    self.buffer.push_str(
                        std::str::from_utf8(&self.pending_bytes[consumed..valid_end])
                            .unwrap_or_default(),
                    );
                    match error.error_len() {
                        // Incomplete sequence at the end: wait for more bytes.
                        None => {
                            consumed = valid_end;
                            break;
                        }
                        Some(invalid_len) => {
                            self.buffer.push(char::REPLACEMENT_CHARACTER);
                            consumed = valid_end + invalid_len;
                        }
                    }
                }
            }
        }
        self.pending_bytes.drain(..consumed);
    }

    /// Append content. Flushes automatically based on timer or size.
    pub fn append(&mut self, content: &str) -> io::Result<()> {
        self.buffer.push_str(content);

        let should_flush = self.last_flush.elapsed() >= self.frame_interval
            || self.buffer.len() >= BUFFER_FLUSH_THRESHOLD;

        if should_flush {
            self.flush()?;
        }

        Ok(())
    }

    /// Force flush remaining buffer.
    pub fn flush(&mut self) -> io::Result<()> {
        if !self.buffer.is_empty() {
            self.stdout.write_all(self.buffer.as_bytes())?;
            self.stdout.flush()?;
            self.buffer.clear();
            self.last_flush = Instant::now();
        }
        Ok(())
    }
}

impl io::Write for SyncRenderBuffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.push_bytes(buf);
        let should_flush = self.last_flush.elapsed() >= self.frame_interval
            || self.buffer.len() >= BUFFER_FLUSH_THRESHOLD;
        if should_flush {
            SyncRenderBuffer::flush(self)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        SyncRenderBuffer::flush(self)
    }
}

impl Drop for SyncRenderBuffer {
    fn drop(&mut self) {
        if !self.pending_bytes.is_empty() {
            // A sequence that never completed: render it lossily.
            let tail = String::from_utf8_lossy(&self.pending_bytes).into_owned();
            self.buffer.push_str(&tail);
            self.pending_bytes.clear();
        }
        let _ = SyncRenderBuffer::flush(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn buffer_batches_writes() {
        let buf = StreamBuffer::new();
        buf.send("hello ");
        buf.send("world");
        thread::sleep(Duration::from_millis(60));
        // Should have flushed by now
        buf.shutdown();
    }

    #[test]
    fn sync_buffer_flushes_on_size() {
        let mut buf = SyncRenderBuffer::new();
        let big = "x".repeat(BUFFER_FLUSH_THRESHOLD + 100);
        buf.append(&big).unwrap();
        assert_eq!(buf.buffer, ""); // flushed due to size
    }

    #[test]
    fn utf8_sequences_split_across_writes_are_reassembled() {
        let mut buf = SyncRenderBuffer::new();
        // Keep the timer from flushing mid-test.
        buf.frame_interval = Duration::from_secs(3_600);
        let text = "é🙂日";
        let bytes = text.as_bytes();
        for chunk in bytes.chunks(1) {
            buf.write_all(chunk).expect("write");
        }
        assert_eq!(buf.buffer, text);
        assert_eq!(buf.pending_bytes, Vec::<u8>::new());

        buf.buffer.clear();
        buf.write_all(&[b'a', 0xFF, b'b']).expect("write");
        assert_eq!(buf.buffer, "a\u{FFFD}b");
        buf.buffer.clear();
    }
}
