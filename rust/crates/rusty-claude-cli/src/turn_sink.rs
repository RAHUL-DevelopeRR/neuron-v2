//! Where a turn's visible output goes. The agent loop calls a [`TurnSink`] instead of
//! printing, so the same turn can render to the line terminal or a file ([`AnsiSink`]),
//! to nowhere (one-shot JSON output, sub-agents), or to
//! the full-screen UI as structured [`UiEvent`]s ([`ChannelSink`]) that it draws itself.

// A turn's output goes through a TurnSink or runtime::ui_hooks, so a full-screen UI can own
// the terminal.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// Whether the last thing a sink printed on stdout was a tool's result. Results come
/// from the tool sink, the answer after them from the next response's sink: stdout is
/// one stream, so they share this, and the answer starts after a blank line.
static STDOUT_AFTER_TOOL_RESULT: AtomicBool = AtomicBool::new(false);

use neuron_tui::protocol::UiEvent;

use crate::render::{MarkdownStreamState, TerminalRenderer};
use crate::stream_buffer::SyncRenderBuffer;
use crate::tool_ui::{format_tool_call_start, format_tool_result};

/// The visible parts of a turn, in the order they happen.
pub(crate) trait TurnSink: Send {
    /// A streamed piece of assistant text (Markdown).
    fn text_delta(&mut self, text: &str) -> io::Result<()>;
    /// A complete, non-streamed text block.
    fn text_block(&mut self, text: &str) -> io::Result<()>;
    /// The current text block ended: render anything still buffered.
    fn end_block(&mut self) -> io::Result<()>;
    /// A reasoning block started (`chars` when the block is complete, `redacted` when the
    /// provider hides it).
    fn reasoning_started(&mut self, chars: Option<usize>, redacted: bool) -> io::Result<()>;
    /// Streamed reasoning text. The line terminal keeps it hidden.
    fn reasoning_delta(&mut self, text: &str) -> io::Result<()>;
    /// A tool call with its complete input; `cut_off` when the output limit truncated it.
    fn tool_call(&mut self, id: &str, name: &str, input: &str, cut_off: bool) -> io::Result<()>;
    /// A tool's result or error, as the tool returned it (calls that ran only).
    fn tool_result(&mut self, name: &str, output: &str, is_error: bool) -> io::Result<()>;
    /// A tool call's final result as the model sees it, for every call: ran, denied,
    /// blocked or never started. The line terminal shows results through
    /// [`Self::tool_result`] and ignores this.
    fn tool_call_finished(
        &mut self,
        _id: &str,
        _name: &str,
        _output: &str,
        _is_error: bool,
    ) -> io::Result<()> {
        Ok(())
    }
}

/// How a runtime shows its turns.
#[derive(Debug, Clone, Default)]
pub(crate) enum TurnOutput {
    /// Nothing is shown (JSON output, compaction, sub-agents).
    #[default]
    Silent,
    /// ANSI to the terminal, as the line REPL always did.
    Ansi,
    /// Structured events for the full-screen UI.
    Channel(Arc<Sender<UiEvent>>),
}

/// Two outputs are the same when a runtime built for one can serve the other: the same
/// mode and, for the UI, the same channel.
impl PartialEq for TurnOutput {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Silent, Self::Silent) | (Self::Ansi, Self::Ansi) => true,
            (Self::Channel(left), Self::Channel(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }
}

impl From<bool> for TurnOutput {
    /// `emit_output`: `true` is the terminal, `false` is silent.
    fn from(emit_output: bool) -> Self {
        if emit_output {
            Self::Ansi
        } else {
            Self::Silent
        }
    }
}

impl TurnOutput {
    pub(crate) fn channel(events: Sender<UiEvent>) -> Self {
        Self::Channel(Arc::new(events))
    }

    pub(crate) fn is_visible(&self) -> bool {
        !matches!(self, Self::Silent)
    }

    /// A sink for one model response. The terminal one renders through a synchronized
    /// output buffer so a frame is never drawn half-written.
    pub(crate) fn stream_sink(&self) -> Box<dyn TurnSink> {
        match self {
            Self::Silent => Box::new(SilentSink),
            Self::Ansi if !io::stdout().is_terminal() => Box::new(
                AnsiSink::new(StripAnsi::new(io::stdout()))
                    .spacing_tool_results(&STDOUT_AFTER_TOOL_RESULT),
            ),
            Self::Ansi => Box::new(
                AnsiSink::new(SyncRenderBuffer::new())
                    .clearing_spinner()
                    .for_stdout_terminal()
                    .spacing_tool_results(&STDOUT_AFTER_TOOL_RESULT),
            ),
            Self::Channel(events) => Box::new(ChannelSink::new(Sender::clone(events))),
        }
    }

    /// A sink for tool results, which the terminal prints straight to stdout. Built once
    /// per executor: the terminal renderer loads its syntax themes when created.
    pub(crate) fn tool_sink(&self) -> Box<dyn TurnSink> {
        match self {
            Self::Silent => Box::new(SilentSink),
            Self::Ansi if !io::stdout().is_terminal() => Box::new(
                AnsiSink::new(StripAnsi::new(io::stdout()))
                    .spacing_tool_results(&STDOUT_AFTER_TOOL_RESULT),
            ),
            Self::Ansi => Box::new(
                AnsiSink::new(io::stdout())
                    .for_stdout_terminal()
                    .spacing_tool_results(&STDOUT_AFTER_TOOL_RESULT),
            ),
            Self::Channel(events) => Box::new(ChannelSink::new(Sender::clone(events))),
        }
    }
}

/// Writes to `inner` without terminal escape sequences (colours, cursor moves, titles):
/// a turn printed to a file or a pipe should be text.
pub(crate) struct StripAnsi<W: Write> {
    inner: W,
    state: StripState,
}

#[derive(Clone, Copy)]
enum StripState {
    Text,
    Escape,
    Csi,
    Osc,
    OscEscape,
    Charset,
}

impl<W: Write> StripAnsi<W> {
    pub(crate) fn new(inner: W) -> Self {
        Self {
            inner,
            state: StripState::Text,
        }
    }

    pub(crate) fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for StripAnsi<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut text = Vec::with_capacity(buf.len());
        for &byte in buf {
            self.state = match (self.state, byte) {
                (StripState::Text, 0x1b) => StripState::Escape,
                (StripState::Text, byte) => {
                    text.push(byte);
                    StripState::Text
                }
                (StripState::Escape, b'[') => StripState::Csi,
                (StripState::Escape, b']') => StripState::Osc,
                (StripState::Escape, b'(' | b')') => StripState::Charset,
                // ESC 7, ESC 8 and the other two-byte sequences.
                (StripState::Escape | StripState::Charset | StripState::OscEscape, _)
                | (StripState::Csi, 0x40..=0x7e)
                | (StripState::Osc, 0x07) => StripState::Text,
                (StripState::Csi, _) => StripState::Csi,
                (StripState::Osc, 0x1b) => StripState::OscEscape,
                (StripState::Osc, _) => StripState::Osc,
            };
        }
        self.inner.write_all(&text)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Renders Markdown to ANSI on `out`, as the line REPL prints a turn. Everything it
/// prints ends its line, so the turn's end marker (`✔ Done`) gets a line of its own.
pub(crate) struct AnsiSink<W: Write> {
    out: W,
    renderer: TerminalRenderer,
    markdown: MarkdownStreamState,
    /// The reasoning block being received; its summary is printed when it ends.
    reasoning: Option<HiddenReasoning>,
    /// The spinner's label may still be on the current line: the first write clears it.
    clear_spinner: bool,
    /// Shared with the other sinks on the same stream ([`STDOUT_AFTER_TOOL_RESULT`]).
    after_tool_result: Option<&'static AtomicBool>,
    /// `out` is stdout's terminal: text goes through [`crate::console::stdout_text`]
    /// (plain text, or a classic console's marks, where that terminal needs them).
    stdout_terminal: bool,
}

/// A reasoning block the terminal keeps hidden, counted as it arrives.
struct HiddenReasoning {
    /// The size `reasoning_started` gave (a complete block), else 0.
    announced: usize,
    /// Characters received so far.
    received: usize,
    redacted: bool,
}

impl HiddenReasoning {
    fn summary(&self) -> String {
        let chars = self.announced.max(self.received);
        if self.redacted {
            "\n\u{25b6} Thinking block hidden by provider\n".to_string()
        } else if chars > 0 {
            format!("\n\u{25b6} Thinking ({chars} chars hidden)\n")
        } else {
            "\n\u{25b6} Thinking hidden\n".to_string()
        }
    }
}

impl<W: Write> AnsiSink<W> {
    pub(crate) fn new(out: W) -> Self {
        Self {
            out,
            renderer: TerminalRenderer::new(),
            markdown: MarkdownStreamState::default(),
            reasoning: None,
            clear_spinner: false,
            after_tool_result: None,
            stdout_terminal: false,
        }
    }

    /// For stdout's terminal (see [`Self::stdout_terminal`]).
    #[must_use]
    fn for_stdout_terminal(mut self) -> Self {
        self.stdout_terminal = true;
        self
    }

    /// For the terminal, where the turn's spinner was drawn on the line the output starts on.
    #[must_use]
    fn clearing_spinner(mut self) -> Self {
        self.clear_spinner = true;
        self
    }

    /// Text printed after a tool's result, by this sink or another one writing to the
    /// same stream (`after_tool_result`), starts after a blank line.
    #[must_use]
    fn spacing_tool_results(mut self, after_tool_result: &'static AtomicBool) -> Self {
        self.after_tool_result = Some(after_tool_result);
        self
    }

    fn write_flushed(&mut self, text: &str) -> io::Result<()> {
        let mut shown = String::with_capacity(text.len() + 5);
        if std::mem::take(&mut self.clear_spinner) {
            shown.push_str("\r\x1b[2K");
        }
        shown.push_str(text);
        let shown = if self.stdout_terminal {
            crate::console::stdout_text(&shown)
        } else {
            std::borrow::Cow::Borrowed(shown.as_str())
        };
        self.out.write_all(shown.as_bytes())?;
        self.out.flush()
    }

    /// Assistant text, or a reasoning summary (which starts with its own blank line).
    fn write_text(&mut self, text: &str, starts_blank: bool) -> io::Result<()> {
        let after_tool_result = self
            .after_tool_result
            .is_some_and(|after| after.swap(false, Ordering::Relaxed));
        if after_tool_result && !starts_blank {
            self.write_flushed(&format!("\n{text}"))
        } else {
            self.write_flushed(text)
        }
    }

    /// Prints the summary of a reasoning block that ended.
    fn finish_reasoning(&mut self) -> io::Result<()> {
        match self.reasoning.take() {
            Some(reasoning) => self.write_text(&reasoning.summary(), true),
            None => Ok(()),
        }
    }

    /// Prints what is still buffered of the text block.
    fn finish_text(&mut self) -> io::Result<()> {
        match self.markdown.flush(&self.renderer) {
            Some(rendered) => self.write_text(&rendered, false),
            None => Ok(()),
        }
    }

    fn finish_blocks(&mut self) -> io::Result<()> {
        self.finish_text()?;
        self.finish_reasoning()
    }
}

/// A response cut short (an interrupt, a failed stream) still prints what it buffered and
/// ends its line.
impl<W: Write> Drop for AnsiSink<W> {
    fn drop(&mut self) {
        let _ = self.finish_blocks();
    }
}

impl<W: Write + Send> TurnSink for AnsiSink<W> {
    fn text_delta(&mut self, text: &str) -> io::Result<()> {
        self.finish_reasoning()?;
        match self.markdown.push(&self.renderer, text) {
            Some(rendered) => self.write_text(&rendered, false),
            None => Ok(()),
        }
    }

    fn text_block(&mut self, text: &str) -> io::Result<()> {
        self.finish_reasoning()?;
        match self.markdown.render_block(&self.renderer, text) {
            Some(rendered) => self.write_text(&rendered, false),
            None => Ok(()),
        }
    }

    fn end_block(&mut self) -> io::Result<()> {
        self.finish_blocks()
    }

    fn reasoning_started(&mut self, chars: Option<usize>, redacted: bool) -> io::Result<()> {
        self.finish_blocks()?;
        self.reasoning = Some(HiddenReasoning {
            announced: chars.unwrap_or(0),
            received: 0,
            redacted,
        });
        Ok(())
    }

    fn reasoning_delta(&mut self, text: &str) -> io::Result<()> {
        let reasoning = self.reasoning.get_or_insert(HiddenReasoning {
            announced: 0,
            received: 0,
            redacted: false,
        });
        reasoning.received += text.chars().count();
        Ok(())
    }

    fn tool_call(&mut self, _id: &str, name: &str, input: &str, cut_off: bool) -> io::Result<()> {
        self.finish_blocks()?;
        let line = if cut_off {
            crate::api_client::format_cut_off_tool_call(name, input)
        } else {
            format_tool_call_start(name, input)
        };
        // It starts with a blank line of its own.
        if let Some(after) = self.after_tool_result {
            after.store(false, Ordering::Relaxed);
        }
        self.write_flushed(&format!("\n{line}\n"))
    }

    /// Tool output is shown as the tool wrote it, never read as Markdown.
    fn tool_result(&mut self, name: &str, output: &str, is_error: bool) -> io::Result<()> {
        self.finish_blocks()?;
        let mut shown = format_tool_result(name, output, is_error);
        if !shown.ends_with('\n') {
            shown.push('\n');
        }
        self.write_flushed(&shown)?;
        if let Some(after) = self.after_tool_result {
            after.store(true, Ordering::Relaxed);
        }
        Ok(())
    }
}

/// Shows nothing.
pub(crate) struct SilentSink;

impl TurnSink for SilentSink {
    fn text_delta(&mut self, _text: &str) -> io::Result<()> {
        Ok(())
    }
    fn text_block(&mut self, _text: &str) -> io::Result<()> {
        Ok(())
    }
    fn end_block(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn reasoning_started(&mut self, _chars: Option<usize>, _redacted: bool) -> io::Result<()> {
        Ok(())
    }
    fn reasoning_delta(&mut self, _text: &str) -> io::Result<()> {
        Ok(())
    }
    fn tool_call(
        &mut self,
        _id: &str,
        _name: &str,
        _input: &str,
        _cut_off: bool,
    ) -> io::Result<()> {
        Ok(())
    }
    fn tool_result(&mut self, _name: &str, _output: &str, _is_error: bool) -> io::Result<()> {
        Ok(())
    }
}

/// Sends structured events to the full-screen UI. A closed UI is an error, so the turn
/// stops instead of running on with nobody watching.
pub(crate) struct ChannelSink {
    events: Sender<UiEvent>,
    /// Text was sent since the last [`UiEvent::TextBlockEnd`].
    text_open: bool,
}

impl ChannelSink {
    pub(crate) fn new(events: Sender<UiEvent>) -> Self {
        Self {
            events,
            text_open: false,
        }
    }

    fn send(&self, event: UiEvent) -> io::Result<()> {
        self.events
            .send(event)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the Neuron UI closed"))
    }
}

/// A response cut short (an interrupt, a failed stream) still ends its text block, so
/// the UI never glues the next block onto it.
impl Drop for ChannelSink {
    fn drop(&mut self) {
        let _ = self.end_block();
    }
}

impl TurnSink for ChannelSink {
    fn text_delta(&mut self, text: &str) -> io::Result<()> {
        self.text_open = true;
        self.send(UiEvent::TextDelta {
            text: text.to_string(),
        })
    }

    fn text_block(&mut self, text: &str) -> io::Result<()> {
        self.text_delta(text)?;
        self.end_block()
    }

    /// Blocks of every kind end with this call; the UI hears only about text blocks.
    fn end_block(&mut self) -> io::Result<()> {
        if !std::mem::take(&mut self.text_open) {
            return Ok(());
        }
        self.send(UiEvent::TextBlockEnd)
    }

    fn reasoning_started(&mut self, chars: Option<usize>, redacted: bool) -> io::Result<()> {
        self.send(UiEvent::ReasoningStarted { chars, redacted })
    }

    fn reasoning_delta(&mut self, text: &str) -> io::Result<()> {
        self.send(UiEvent::ReasoningDelta {
            text: text.to_string(),
        })
    }

    fn tool_call(&mut self, id: &str, name: &str, input: &str, cut_off: bool) -> io::Result<()> {
        self.send(UiEvent::ToolCallStarted {
            id: id.to_string(),
            name: name.to_string(),
            input: input.to_string(),
            cut_off,
        })
    }

    /// The UI hears results from [`Self::tool_call_finished`], which covers every call.
    fn tool_result(&mut self, _name: &str, _output: &str, _is_error: bool) -> io::Result<()> {
        Ok(())
    }

    fn tool_call_finished(
        &mut self,
        id: &str,
        name: &str,
        output: &str,
        is_error: bool,
    ) -> io::Result<()> {
        self.send(UiEvent::ToolResult {
            id: id.to_string(),
            name: name.to_string(),
            output: output.to_string(),
            is_error,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// The ANSI sink renders with the line REPL's renderer, and every block it prints
    /// ends its line.
    #[test]
    fn ansi_sink_prints_each_block_on_lines_of_its_own() {
        let renderer = TerminalRenderer::new();
        let mut expected = String::new();
        let mut markdown = MarkdownStreamState::default();
        for chunk in ["Hello **wor", "ld**\n\n", "- item\n"] {
            if let Some(rendered) = markdown.push(&renderer, chunk) {
                expected.push_str(&rendered);
            }
        }
        if let Some(rendered) = markdown.flush(&renderer) {
            expected.push_str(&rendered);
        }
        expected.push_str("\n\u{25b6} Thinking (42 chars hidden)\n");
        expected.push_str(&format!(
            "\n{}\n",
            format_tool_call_start("read_file", r#"{"path":"src/main.rs"}"#)
        ));
        expected.push_str(&renderer.markdown_to_ansi("Done."));
        expected.push('\n');

        let mut out = Vec::new();
        {
            let mut sink = AnsiSink::new(&mut out);
            for chunk in ["Hello **wor", "ld**\n\n", "- item\n"] {
                sink.text_delta(chunk).unwrap();
            }
            sink.end_block().unwrap();
            sink.reasoning_started(Some(42), false).unwrap();
            sink.reasoning_delta("never shown on the terminal").unwrap();
            sink.tool_call("toolu_1", "read_file", r#"{"path":"src/main.rs"}"#, false)
                .unwrap();
            sink.text_block("Done.").unwrap();
        }
        let output = String::from_utf8(out).unwrap();
        assert_eq!(output, expected);
        assert!(
            crate::render::strip_ansi(&output).starts_with(
                "Hello world\n\n\u{2022} item\n\n\u{25b6} Thinking (42 chars hidden)\n"
            ),
            "{output}"
        );
    }

    /// What `neuron prompt ...` writes to a file or a pipe.
    fn plain_output(write: impl FnOnce(&mut dyn TurnSink)) -> String {
        let mut out = StripAnsi::new(Vec::new());
        {
            let mut sink = AnsiSink::new(&mut out);
            write(&mut sink);
        }
        String::from_utf8(out.inner).unwrap()
    }

    #[test]
    fn text_output_keeps_line_breaks_paragraphs_and_path_backslashes() {
        let answer = "**Location:**\nC:\\Users\\x\\.neuron\\a.txt\n\nFirst paragraph: this file holds a note.\nIt continues on a second line.\n\nSecond paragraph: backslashes separate folders on Windows.";
        let shown = "Location:\nC:\\Users\\x\\.neuron\\a.txt\n\nFirst paragraph: this file holds a note.\nIt continues on a second line.\n\nSecond paragraph: backslashes separate folders on Windows.\n";
        // However the stream splits the answer.
        let chunkings: [Vec<String>; 3] = [
            vec![answer.to_string()],
            answer.split_inclusive('\n').map(str::to_string).collect(),
            answer.chars().map(String::from).collect(),
        ];
        for chunks in chunkings {
            let printed = plain_output(|sink| {
                for chunk in &chunks {
                    sink.text_delta(chunk).unwrap();
                }
                sink.end_block().unwrap();
            });
            assert_eq!(printed, shown, "{} chunks", chunks.len());
        }
        assert_eq!(plain_output(|sink| sink.text_block(answer).unwrap()), shown);
    }

    #[test]
    fn the_end_marker_starts_a_line_of_its_own() {
        let theme = crate::render::ColorTheme::default();
        let mut out = StripAnsi::new(Vec::new());
        {
            let mut sink = AnsiSink::new(&mut out);
            sink.text_delta("ok").unwrap();
            sink.end_block().unwrap();
        }
        crate::render::Spinner::plain()
            .finish("Done", &theme, &mut out)
            .unwrap();
        assert_eq!(String::from_utf8(out.inner).unwrap(), "ok\n\u{2714} Done\n");

        // Cut short: what was buffered still prints, and its line ends.
        assert_eq!(
            plain_output(|sink| sink.text_delta("partial ans").unwrap()),
            "partial ans\n"
        );

        // On a terminal the spinner's label is cleared before the answer replaces it,
        // and the answer's line ends before the marker's "go to column 0 and clear".
        let mut out = Vec::new();
        {
            let mut sink = AnsiSink::new(&mut out).clearing_spinner();
            sink.text_delta("ok").unwrap();
            sink.end_block().unwrap();
            sink.text_delta("more").unwrap();
        }
        let printed = String::from_utf8(out).unwrap();
        assert!(printed.starts_with("\r\x1b[2K"), "{printed:?}");
        assert_eq!(printed.matches("\r\x1b[2K").count(), 1, "{printed:?}");
        assert!(printed.ends_with('\n'), "{printed:?}");
    }

    #[test]
    fn the_answer_after_a_tool_result_starts_after_a_blank_line() {
        static AFTER_TOOL_RESULT: AtomicBool = AtomicBool::new(false);
        let mut out = StripAnsi::new(Vec::new());
        // The result comes from the tool sink, the answer from the next response's.
        {
            let mut tools = AnsiSink::new(&mut out).spacing_tool_results(&AFTER_TOOL_RESULT);
            tools
                .tool_result("bash", r#"{"stdout":"hi\n","stderr":""}"#, false)
                .unwrap();
        }
        {
            let mut answer = AnsiSink::new(&mut out).spacing_tool_results(&AFTER_TOOL_RESULT);
            answer.text_delta("All done.").unwrap();
            answer.end_block().unwrap();
            // Only the first text after the result.
            answer.text_delta("More.").unwrap();
            answer.end_block().unwrap();
            // A tool call has its own blank line.
            answer
                .tool_call("t1", "bash", r#"{"command":"ls"}"#, false)
                .unwrap();
        }
        let printed = String::from_utf8(out.inner).unwrap();
        assert!(printed.contains("hi\n\nAll done."), "{printed:?}");
        assert!(!printed.contains("\n\n\n"), "{printed:?}");
        assert!(!AFTER_TOOL_RESULT.load(Ordering::Relaxed));
    }

    #[test]
    fn tool_output_is_shown_as_written_not_as_markdown() {
        let content = "/**\n * Adds two numbers.\n * @returns {number} the *sum*, e.g. C:\\tmp\\x.js\n */\nfunction sum(a, b) {\n  return a + b; // **not** bold\n}";
        let read = serde_json::json!({"file": {
            "filePath": "sum.js", "content": content,
            "numLines": 7, "startLine": 1, "totalLines": 7
        }})
        .to_string();
        let printed = plain_output(|sink| sink.tool_result("read_file", &read, false).unwrap());
        assert!(printed.ends_with(&format!("\n{content}\n")), "{printed}");
        assert!(!printed.contains('\u{2022}'), "no bullets: {printed}");

        let bash = serde_json::json!({"stdout": format!("{content}\n"), "stderr": ""}).to_string();
        let printed = plain_output(|sink| sink.tool_result("bash", &bash, false).unwrap());
        assert!(printed.contains(content), "{printed}");

        let sent = r#"{"attachments":null,"message":"done"}"#;
        let printed =
            plain_output(|sink| sink.tool_result("SendUserMessage", sent, false).unwrap());
        assert!(
            printed.contains("{\n  \"attachments\": null,\n  \"message\": \"done\"\n}\n"),
            "{printed}"
        );
    }

    #[test]
    fn the_thinking_summary_counts_the_reasoning_received() {
        // Streamed: the block starts empty and the text arrives in pieces.
        let printed = plain_output(|sink| {
            sink.reasoning_started(Some(0), false).unwrap();
            sink.reasoning_delta("abc").unwrap();
            sink.reasoning_delta("dé").unwrap();
            sink.end_block().unwrap();
            sink.text_delta("answer\n").unwrap();
        });
        assert_eq!(printed, "\n\u{25b6} Thinking (5 chars hidden)\nanswer\n");
        // A complete block is not counted twice.
        let printed = plain_output(|sink| {
            sink.reasoning_started(Some(4), false).unwrap();
            sink.reasoning_delta("abcd").unwrap();
        });
        assert_eq!(printed, "\n\u{25b6} Thinking (4 chars hidden)\n");
        let printed = plain_output(|sink| sink.reasoning_started(None, false).unwrap());
        assert_eq!(printed, "\n\u{25b6} Thinking hidden\n");
        let printed = plain_output(|sink| sink.reasoning_started(None, true).unwrap());
        assert_eq!(printed, "\n\u{25b6} Thinking block hidden by provider\n");
    }

    #[test]
    fn channel_sink_sends_structured_events_and_fails_when_the_ui_closes() {
        let (tx, rx) = mpsc::channel();
        let mut sink = ChannelSink::new(tx);
        sink.text_delta("Hi").unwrap();
        sink.reasoning_started(None, false).unwrap();
        sink.reasoning_delta("thinking out loud").unwrap();
        sink.tool_call("toolu_1", "bash", r#"{"command":"ls"}"#, true)
            .unwrap();
        sink.tool_result("bash", "a.txt", false).unwrap();
        sink.tool_call_finished("toolu_1", "bash", "a.txt", false)
            .unwrap();
        sink.end_block().unwrap();
        sink.end_block().unwrap();
        let events: Vec<UiEvent> = rx.try_iter().collect();
        assert_eq!(
            events,
            vec![
                UiEvent::TextDelta { text: "Hi".into() },
                UiEvent::ReasoningStarted {
                    chars: None,
                    redacted: false
                },
                UiEvent::ReasoningDelta {
                    text: "thinking out loud".into()
                },
                UiEvent::ToolCallStarted {
                    id: "toolu_1".into(),
                    name: "bash".into(),
                    input: r#"{"command":"ls"}"#.into(),
                    cut_off: true,
                },
                UiEvent::ToolResult {
                    id: "toolu_1".into(),
                    name: "bash".into(),
                    output: "a.txt".into(),
                    is_error: false
                },
                UiEvent::TextBlockEnd,
            ]
        );
        drop(rx);
        let error = sink.text_delta("anyone?").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn output_to_a_file_or_pipe_has_no_escape_sequences() {
        let mut out = StripAnsi::new(Vec::new());
        write!(
            out,
            "\x1b[38;2;65;105;195m\u{25E6}\x1b[0m Thinking\x1b7\x1b[2K\x1b8 done\x1b]0;title\x07 \x1b(Bok"
        )
        .unwrap();
        // Split across writes, as a stream arrives.
        out.write_all(b"\x1b[1").unwrap();
        out.write_all(b"m bold\x1b[0m").unwrap();
        assert_eq!(
            String::from_utf8(out.inner).unwrap(),
            "\u{25E6} Thinking done ok bold"
        );
    }

    #[test]
    fn a_response_cut_short_still_ends_its_text_block() {
        let (tx, rx) = mpsc::channel();
        let mut sink = ChannelSink::new(tx);
        sink.text_delta("partial ans").unwrap();
        drop(sink);
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            vec![
                UiEvent::TextDelta {
                    text: "partial ans".into()
                },
                UiEvent::TextBlockEnd,
            ]
        );
    }

    #[test]
    fn output_modes_map_from_the_old_emit_flag() {
        assert!(matches!(TurnOutput::from(true), TurnOutput::Ansi));
        assert!(matches!(TurnOutput::from(false), TurnOutput::Silent));
        assert!(!TurnOutput::Silent.is_visible());
        let (tx, _rx) = mpsc::channel();
        let channel = TurnOutput::channel(tx.clone());
        assert!(channel.is_visible());
        assert_eq!(
            channel,
            channel.clone(),
            "a runtime keeps serving its own UI"
        );
        assert_ne!(
            channel,
            TurnOutput::channel(tx),
            "another UI gets a new runtime"
        );
        assert_ne!(channel, TurnOutput::Ansi);
    }
}
