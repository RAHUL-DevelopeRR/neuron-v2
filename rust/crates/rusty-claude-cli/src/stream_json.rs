//! `neuron prompt --output-format stream-json`: the run as newline-delimited JSON on
//! stdout while it happens, for CI, the GitHub Action and cloud runners.
//!
//! The lines are, in order (schema: `docs/stream-json.md`):
//!
//! 1. one `{"type":"init", ...}`: protocol, version, session id, model, cwd, permission
//!    mode and the built-in tools the run may call;
//! 2. the engine protocol's own events ([`UiEvent`], the same `type`-tagged JSON the
//!    full-screen UI reads): `text_delta`, `text_block_end`, `reasoning_started`,
//!    `tool_input_started`, `tool_call_started`, `tool_result` (output cut to
//!    [`TOOL_RESULT_PREVIEW_CHARS`], with `truncated` and `output_chars`),
//!    `request_usage`, `todos`, `subagent_started`, `subagent_finished`, `notice`, ...
//!    High-rate progress (`output_progress`, `tool_input_progress`, reasoning text) is
//!    left out;
//! 3. one `{"type":"result", ...}` with exactly the fields `--output-format json`
//!    prints, success or failure.
//!
//! Notices from the runtime (retries) and errors before the run starts stay on stderr
//! as JSON lines, as in `json` mode.

use std::cell::Cell;
use std::io::{self, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use neuron_tui::protocol::{UiEvent, CAPABILITY_LIVE_PROGRESS, PROTOCOL_VERSION};
use serde_json::{json, Value};

/// The `--output-format` value that selects this mode.
pub(crate) const FORMAT_NAME: &str = "stream-json";
/// A `tool_result` event's output is cut to this many characters; the full output is
/// in the session (and in the final `result` line).
pub(crate) const TOOL_RESULT_PREVIEW_CHARS: usize = 4_000;
/// The schema version of the `init` and `result` lines.
pub(crate) const STREAM_SCHEMA_VERSION: u32 = 1;

thread_local! {
    /// `--output-format stream-json` was given. Parsed on the main thread, read there
    /// when the prompt runs; a thread-local keeps parallel tests apart.
    static REQUESTED: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn set_requested(requested: bool) {
    REQUESTED.with(|cell| cell.set(requested));
}

pub(crate) fn requested() -> bool {
    REQUESTED.with(Cell::get)
}

/// The capabilities the stream asks the sinks for: per-request usage and tool-input
/// starts come with live progress.
pub(crate) fn capabilities() -> Vec<String> {
    vec![CAPABILITY_LIVE_PROGRESS.to_string()]
}

/// The first line of a run.
pub(crate) fn init_event(
    session_id: &str,
    model: &str,
    cwd: &str,
    permission_mode: &str,
    tools: &[String],
) -> Value {
    json!({
        "type": "init",
        "schema": STREAM_SCHEMA_VERSION,
        "protocol": PROTOCOL_VERSION,
        "version": env!("CARGO_PKG_VERSION"),
        "session_id": session_id,
        "model": model,
        "cwd": cwd,
        "permission_mode": permission_mode,
        "tools": tools,
    })
}

/// The last line of a run: `answer` is what `--output-format json` prints. A failed
/// run's answer is `"type": "error"`; here the line stays `result` with `is_error`.
pub(crate) fn result_event(answer: Value) -> Value {
    let is_error = answer.get("type").and_then(Value::as_str) == Some("error");
    let mut result = json!({
        "type": "result",
        "schema": STREAM_SCHEMA_VERSION,
        "is_error": is_error,
    });
    if let (Some(line), Value::Object(fields)) = (result.as_object_mut(), answer) {
        for (key, value) in fields {
            if key != "type" {
                line.insert(key, value);
            }
        }
    }
    result
}

/// The line for `event`, or `None` for events the stream leaves out.
pub(crate) fn encode(event: &UiEvent) -> Option<Value> {
    match event {
        UiEvent::OutputProgress { .. }
        | UiEvent::ToolInputProgress { .. }
        | UiEvent::ReasoningDelta { .. }
        | UiEvent::TurnFinished => None,
        UiEvent::ToolResult {
            id,
            name,
            output,
            is_error,
        } => {
            let chars = output.chars().count();
            let truncated = chars > TOOL_RESULT_PREVIEW_CHARS;
            let shown = if truncated {
                output.chars().take(TOOL_RESULT_PREVIEW_CHARS).collect()
            } else {
                output.clone()
            };
            Some(json!({
                "type": "tool_result",
                "id": id,
                "name": name,
                "output": shown,
                "is_error": is_error,
                "truncated": truncated,
                "output_chars": chars,
            }))
        }
        other => serde_json::to_value(other).ok(),
    }
}

/// Writes `value` as one line and flushes, so a reader sees it at once.
pub(crate) fn write_line(out: &mut impl Write, value: &Value) -> io::Result<()> {
    writeln!(out, "{value}")?;
    out.flush()
}

/// The thread that writes the run's events to stdout as they come.
pub(crate) struct EventWriter {
    sender: Sender<UiEvent>,
    handle: Option<JoinHandle<()>>,
}

impl EventWriter {
    /// Starts writing the events sent to [`Self::sender`] to `out`.
    pub(crate) fn start(out: impl Write + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("neuron-stream-json".to_string())
            .spawn(move || pump(&receiver, out))
            .ok();
        Self { sender, handle }
    }

    pub(crate) fn sender(&self) -> Sender<UiEvent> {
        Sender::clone(&self.sender)
    }

    /// Writes what was sent so far and stops. The runtime keeps its own senders, so the
    /// end is a [`UiEvent::TurnFinished`] (which sinks never send).
    pub(crate) fn finish(mut self) {
        let _ = self.sender.send(UiEvent::TurnFinished);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn pump(receiver: &Receiver<UiEvent>, mut out: impl Write) {
    while let Ok(event) = receiver.recv() {
        if event == UiEvent::TurnFinished {
            return;
        }
        if let Some(line) = encode(&event) {
            if write_line(&mut out, &line).is_err() {
                // stdout closed (the reader went away): drain without writing.
                while receiver.recv().is_ok_and(|event| event != UiEvent::TurnFinished) {}
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn lines(shared: &Shared) -> Vec<Value> {
        String::from_utf8(shared.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn the_requested_flag_is_per_thread_and_resettable() {
        set_requested(true);
        assert!(requested());
        let other = thread::spawn(requested).join().unwrap();
        assert!(!other);
        set_requested(false);
        assert!(!requested());
    }

    #[test]
    fn init_names_session_model_cwd_and_tools() {
        let line = init_event(
            "session-1",
            "auto",
            "/repo",
            "read-only",
            &["read_file".to_string(), "bash".to_string()],
        );
        assert_eq!(line["type"], "init");
        assert_eq!(line["session_id"], "session-1");
        assert_eq!(line["model"], "auto");
        assert_eq!(line["cwd"], "/repo");
        assert_eq!(line["permission_mode"], "read-only");
        assert_eq!(line["tools"], json!(["read_file", "bash"]));
        assert_eq!(line["protocol"], PROTOCOL_VERSION);
    }

    #[test]
    fn result_keeps_every_json_output_field() {
        let answer = json!({
            "message": "done",
            "session_id": "s",
            "usage": { "input_tokens": 3 },
            "estimated_cost": "$0.00",
        });
        let line = result_event(answer.clone());
        assert_eq!(line["type"], "result");
        assert_eq!(line["is_error"], false);
        let failed = result_event(json!({ "type": "error", "error": "boom", "message": "" }));
        assert_eq!(failed["type"], "result");
        assert_eq!(failed["is_error"], true);
        assert_eq!(failed["error"], "boom");
        for (key, value) in answer.as_object().unwrap() {
            assert_eq!(&line[key], value, "{key}");
        }
    }

    #[test]
    fn protocol_events_pass_through_and_noise_is_left_out() {
        let delta = encode(&UiEvent::TextDelta {
            text: "hi".to_string(),
        })
        .unwrap();
        assert_eq!(delta, json!({ "type": "text_delta", "text": "hi" }));
        let call = encode(&UiEvent::ToolCallStarted {
            id: "t1".to_string(),
            name: "bash".to_string(),
            input: "{}".to_string(),
            cut_off: false,
        })
        .unwrap();
        assert_eq!(call["type"], "tool_call_started");
        assert_eq!(call["id"], "t1");
        assert_eq!(encode(&UiEvent::OutputProgress { output_tokens: 3 }), None);
        assert_eq!(
            encode(&UiEvent::ReasoningDelta {
                text: "x".to_string()
            }),
            None
        );
        assert_eq!(encode(&UiEvent::TurnFinished), None);
    }

    #[test]
    fn long_tool_results_are_truncated_with_their_size() {
        let output = "é".repeat(TOOL_RESULT_PREVIEW_CHARS + 10);
        let line = encode(&UiEvent::ToolResult {
            id: "t".to_string(),
            name: "read_file".to_string(),
            output,
            is_error: false,
        })
        .unwrap();
        assert_eq!(line["truncated"], true);
        assert_eq!(line["output_chars"], TOOL_RESULT_PREVIEW_CHARS + 10);
        assert_eq!(
            line["output"].as_str().unwrap().chars().count(),
            TOOL_RESULT_PREVIEW_CHARS
        );
        let short = encode(&UiEvent::ToolResult {
            id: "t".to_string(),
            name: "read_file".to_string(),
            output: "ok".to_string(),
            is_error: true,
        })
        .unwrap();
        assert_eq!(short["truncated"], false);
        assert_eq!(short["output"], "ok");
        assert_eq!(short["is_error"], true);
    }

    #[test]
    fn the_writer_streams_events_in_order_and_stops_at_finish() {
        let out = Shared::default();
        let writer = EventWriter::start(out.clone());
        let sender = writer.sender();
        sender
            .send(UiEvent::TextDelta {
                text: "a".to_string(),
            })
            .unwrap();
        sender
            .send(UiEvent::OutputProgress { output_tokens: 1 })
            .unwrap();
        sender.send(UiEvent::TextBlockEnd).unwrap();
        writer.finish();
        // Sends after the end go nowhere and do not panic.
        let _ = sender.send(UiEvent::TextBlockEnd);
        let seen = lines(&out);
        assert_eq!(
            seen,
            vec![
                json!({ "type": "text_delta", "text": "a" }),
                json!({ "type": "text_block_end" }),
            ]
        );
    }
}
