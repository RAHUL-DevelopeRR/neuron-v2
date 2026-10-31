//! Where a turn's visible output goes. The agent loop calls a [`TurnSink`] instead of
//! printing, so the same turn can render to the line terminal or a file ([`AnsiSink`]),
//! to nowhere (one-shot JSON output, sub-agents), or to
//! the full-screen UI as structured [`UiEvent`]s ([`ChannelSink`]) that it draws itself.

// A turn's output goes through a TurnSink or runtime::ui_hooks, so a full-screen UI can own
// the terminal.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Whether the last thing a sink printed on stdout was a tool's result. Results come
/// from the tool sink, the answer after them from the next response's sink: stdout is
/// one stream, so they share this, and the answer starts after a blank line.
static STDOUT_AFTER_TOOL_RESULT: AtomicBool = AtomicBool::new(false);

use neuron_tui::protocol::{TokenCounts, UiEvent, CAPABILITY_GIT_VIEWS, CAPABILITY_LIVE_PROGRESS};
use runtime::context_manager::TextTokenEstimate;
use runtime::TokenUsage;

use crate::render::{MarkdownStreamState, TerminalRenderer};
use crate::stream_buffer::SyncRenderBuffer;
use crate::tool_input_preview::preview_tool_input;
use crate::tool_ui::{format_tool_call_start, format_tool_result};

/// How often, at most, the UI hears that a tool call's input grew.
pub(crate) const TOOL_INPUT_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// How often, at most, the UI hears the output estimate of the response.
pub(crate) const OUTPUT_PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// The notice for a stream that ended without any of the reply.
pub(crate) const REPLY_NOT_STREAMED_NOTICE: &str =
    "\u{26a0} The reply stream sent nothing \u{2014} asking again; the reply will arrive all at once\u{2026}";

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
    /// The model started a tool call and is writing its input: [`Self::tool_input_delta`]
    /// follows as it grows, [`Self::tool_input_finished`] when it is written, and
    /// [`Self::tool_call`] once the response says whether it runs. Only the UI listens.
    fn tool_input_started(&mut self, _id: &str, _name: &str) -> io::Result<()> {
        Ok(())
    }
    /// A tool call's input grew: `input` is all of it so far (JSON text, likely
    /// incomplete).
    fn tool_input_delta(&mut self, _id: &str, _name: &str, _input: &str) -> io::Result<()> {
        Ok(())
    }
    /// The model finished writing a tool call's input, or the output limit cut it off.
    fn tool_input_finished(&mut self, _id: &str, _name: &str, _input: &str) -> io::Result<()> {
        Ok(())
    }
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
    /// The reply shown so far is dropped and the request sent again. The line
    /// terminal keeps the text it printed, so it marks it; the UI drops the attempt
    /// on the retry notice itself.
    fn reply_discarded(&mut self) -> io::Result<()> {
        Ok(())
    }
    /// One model request of the turn ended: its tokens as the provider reported them.
    /// Only the UI listens; the turn's totals come when it ends.
    fn request_usage(&mut self, _usage: TokenUsage) -> io::Result<()> {
        Ok(())
    }
    /// The stream ended without any of the reply: the request goes again without
    /// streaming, and the reply arrives all at once. Only the UI listens.
    fn reply_not_streamed(&mut self) -> io::Result<()> {
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
    Channel(Arc<UiChannel>),
    /// What a file or a pipe receives (`neuron prompt ... > out.txt`), kept for a test.
    #[cfg(test)]
    Captured(Arc<std::sync::Mutex<Vec<u8>>>),
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

/// The full-screen UI's event channel, and what that UI declared it handles.
#[derive(Debug)]
pub(crate) struct UiChannel {
    pub(crate) events: Sender<UiEvent>,
    pub(crate) capabilities: Arc<UiCapabilities>,
}

/// What the UI declared it handles ([`EngineCommand::Capabilities`]). The engine's
/// command thread sets it, at any time; sinks read it for each event.
///
/// [`EngineCommand::Capabilities`]: neuron_tui::protocol::EngineCommand::Capabilities
#[derive(Debug, Default)]
pub(crate) struct UiCapabilities {
    live_progress: AtomicBool,
    git_views: AtomicBool,
}

impl UiCapabilities {
    /// Replaces what an earlier declaration said.
    pub(crate) fn set(&self, features: &[String]) {
        let has = |name: &str| features.iter().any(|feature| feature == name);
        self.live_progress
            .store(has(CAPABILITY_LIVE_PROGRESS), Ordering::SeqCst);
        self.git_views
            .store(has(CAPABILITY_GIT_VIEWS), Ordering::SeqCst);
    }

    /// Tool-input progress, output estimates and per-request usage.
    pub(crate) fn live_progress(&self) -> bool {
        self.live_progress.load(Ordering::SeqCst)
    }

    /// Changes, diffs, commit proposals, checkpoints and rewinds as events.
    pub(crate) fn git_views(&self) -> bool {
        self.git_views.load(Ordering::SeqCst)
    }
}

impl TurnOutput {
    /// A UI that declared nothing yet.
    pub(crate) fn channel(events: Sender<UiEvent>) -> Self {
        Self::channel_with_capabilities(events, Arc::default())
    }

    pub(crate) fn channel_with_capabilities(
        events: Sender<UiEvent>,
        capabilities: Arc<UiCapabilities>,
    ) -> Self {
        Self::Channel(Arc::new(UiChannel {
            events,
            capabilities,
        }))
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
            Self::Channel(channel) => Box::new(ChannelSink::with_capabilities(
                Sender::clone(&channel.events),
                Arc::clone(&channel.capabilities),
            )),
            #[cfg(test)]
            Self::Captured(buffer) => {
                Box::new(AnsiSink::new(StripAnsi::new(Captured(Arc::clone(buffer)))))
            }
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
            Self::Channel(channel) => Box::new(ChannelSink::with_capabilities(
                Sender::clone(&channel.events),
                Arc::clone(&channel.capabilities),
            )),
            #[cfg(test)]
            Self::Captured(buffer) => {
                Box::new(AnsiSink::new(StripAnsi::new(Captured(Arc::clone(buffer)))))
            }
        }
    }
}

/// The writer of [`TurnOutput::Captured`].
#[cfg(test)]
struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

#[cfg(test)]
impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
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

    fn reply_discarded(&mut self) -> io::Result<()> {
        self.finish_blocks()?;
        self.write_flushed("\x1b[2m(partial reply discarded \u{2014} retrying)\x1b[0m\n")
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

/// Where a sink reads the time for its throttles: the real clock, or a test's.
pub(crate) type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// Sends structured events to the full-screen UI. A closed UI is an error, so the turn
/// stops instead of running on with nobody watching.
pub(crate) struct ChannelSink {
    events: Sender<UiEvent>,
    /// What the UI handles, as it declared it.
    capabilities: Arc<UiCapabilities>,
    /// Text was sent since the last [`UiEvent::TextBlockEnd`].
    text_open: bool,
    /// The response as the model writes it, for the live-progress events.
    live: LiveProgress,
    clock: Clock,
}

/// What a sink knows of the response the model is writing, for the live-progress
/// events. Kept also while the UI did not ask for them, so a UI that asks in the middle
/// of a response hears the right numbers.
#[derive(Default)]
struct LiveProgress {
    /// Tool calls whose input the model is writing, by id.
    tool_inputs: HashMap<String, ToolInputState>,
    /// The response's text, reasoning and tool input so far.
    output: TextTokenEstimate,
    /// When the UI last heard the estimate, and what it heard.
    output_sent: Option<(Instant, u64)>,
}

#[derive(Default)]
struct ToolInputState {
    /// Bytes of the input counted in [`LiveProgress::output`].
    counted: usize,
    /// The UI heard [`UiEvent::ToolInputStarted`].
    announced: bool,
    /// When the UI last heard [`UiEvent::ToolInputProgress`].
    progress_sent_at: Option<Instant>,
}

impl ChannelSink {
    /// For a UI that declared nothing.
    pub(crate) fn new(events: Sender<UiEvent>) -> Self {
        Self::with_capabilities(events, Arc::default())
    }

    pub(crate) fn with_capabilities(
        events: Sender<UiEvent>,
        capabilities: Arc<UiCapabilities>,
    ) -> Self {
        Self {
            events,
            capabilities,
            text_open: false,
            live: LiveProgress::default(),
            clock: Arc::new(Instant::now),
        }
    }

    /// The throttles read the time from `clock`.
    #[cfg(test)]
    fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    fn send(&self, event: UiEvent) -> io::Result<()> {
        self.events
            .send(event)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the Neuron UI closed"))
    }

    fn live_progress(&self) -> bool {
        self.capabilities.live_progress()
    }

    /// The response grew by `text` (answer or reasoning): the estimate, when it is due.
    fn output_grew(&mut self, text: &str) -> io::Result<()> {
        self.live.output.push(text);
        if !self.live_progress() {
            return Ok(());
        }
        let now = (self.clock)();
        self.send_output_progress(now)
    }

    /// [`UiEvent::OutputProgress`], at most every [`OUTPUT_PROGRESS_INTERVAL`] and only
    /// with a new number.
    fn send_output_progress(&mut self, now: Instant) -> io::Result<()> {
        let tokens = u64::try_from(self.live.output.tokens()).unwrap_or(u64::MAX);
        if tokens == 0 {
            return Ok(());
        }
        if let Some((sent_at, sent)) = self.live.output_sent {
            if sent == tokens || now.saturating_duration_since(sent_at) < OUTPUT_PROGRESS_INTERVAL {
                return Ok(());
            }
        }
        self.live.output_sent = Some((now, tokens));
        self.send(UiEvent::OutputProgress {
            output_tokens: tokens,
        })
    }

    /// Counts the part of a tool call's `input` not counted yet in the estimate.
    fn count_tool_input(&mut self, id: &str, input: &str) {
        let state = self.live.tool_inputs.entry(id.to_string()).or_default();
        // The input only grows, so what was counted is a prefix of it.
        let new = input.get(state.counted..).unwrap_or(input);
        self.live.output.push(new);
        state.counted = input.len();
    }

    /// [`UiEvent::ToolInputStarted`] once per call.
    fn announce_tool_input(&mut self, id: &str, name: &str) -> io::Result<()> {
        let state = self.live.tool_inputs.entry(id.to_string()).or_default();
        if std::mem::replace(&mut state.announced, true) {
            return Ok(());
        }
        self.send(UiEvent::ToolInputStarted {
            id: id.to_string(),
            name: name.to_string(),
        })
    }

    /// [`UiEvent::ToolInputProgress`] now, after the call's start if the UI has not
    /// heard it.
    fn send_tool_input_progress(
        &mut self,
        id: &str,
        name: &str,
        input: &str,
        now: Instant,
    ) -> io::Result<()> {
        self.announce_tool_input(id, name)?;
        if let Some(state) = self.live.tool_inputs.get_mut(id) {
            state.progress_sent_at = Some(now);
        }
        let preview = preview_tool_input(name, input);
        self.send(UiEvent::ToolInputProgress {
            id: id.to_string(),
            name: name.to_string(),
            bytes: u64::try_from(input.len()).unwrap_or(u64::MAX),
            lines: preview.lines,
            target: preview.target,
            preview: preview.preview,
        })
    }
}

/// `usage` as the protocol carries it.
fn token_counts(usage: TokenUsage) -> TokenCounts {
    TokenCounts {
        input: u64::from(usage.input_tokens),
        output: u64::from(usage.output_tokens),
        cache_read: u64::from(usage.cache_read_input_tokens),
        cache_write: u64::from(usage.cache_creation_input_tokens),
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
        })?;
        self.output_grew(text)
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
        })?;
        self.output_grew(text)
    }

    fn tool_input_started(&mut self, id: &str, name: &str) -> io::Result<()> {
        self.live
            .tool_inputs
            .insert(id.to_string(), ToolInputState::default());
        if !self.live_progress() {
            return Ok(());
        }
        self.announce_tool_input(id, name)
    }

    /// Progress at most every [`TOOL_INPUT_PROGRESS_INTERVAL`] per call.
    fn tool_input_delta(&mut self, id: &str, name: &str, input: &str) -> io::Result<()> {
        self.count_tool_input(id, input);
        if !self.live_progress() {
            return Ok(());
        }
        let now = (self.clock)();
        let due = self
            .live
            .tool_inputs
            .get(id)
            .and_then(|state| state.progress_sent_at)
            .is_none_or(|sent_at| {
                now.saturating_duration_since(sent_at) >= TOOL_INPUT_PROGRESS_INTERVAL
            });
        if due {
            self.send_tool_input_progress(id, name, input, now)?;
        }
        self.send_output_progress(now)
    }

    /// The call's last progress, whatever the throttle says.
    fn tool_input_finished(&mut self, id: &str, name: &str, input: &str) -> io::Result<()> {
        self.count_tool_input(id, input);
        let result = if self.live_progress() {
            let now = (self.clock)();
            self.send_tool_input_progress(id, name, input, now)
        } else {
            Ok(())
        };
        self.live.tool_inputs.remove(id);
        result
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

    fn request_usage(&mut self, usage: TokenUsage) -> io::Result<()> {
        if !self.live_progress() {
            return Ok(());
        }
        self.send(UiEvent::RequestUsage {
            usage: token_counts(usage),
        })
    }

    fn reply_not_streamed(&mut self) -> io::Result<()> {
        if !self.live_progress() {
            return Ok(());
        }
        self.send(UiEvent::Notice {
            message: REPLY_NOT_STREAMED_NOTICE.to_string(),
            kind: None,
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

    /// A clock the test moves by hand.
    fn manual_clock() -> (Clock, Arc<std::sync::Mutex<Instant>>) {
        let now = Arc::new(std::sync::Mutex::new(Instant::now()));
        let read = Arc::clone(&now);
        (Arc::new(move || *read.lock().unwrap()), now)
    }

    fn advance(now: &std::sync::Mutex<Instant>, millis: u64) {
        *now.lock().unwrap() += Duration::from_millis(millis);
    }

    fn live_capabilities() -> Arc<UiCapabilities> {
        let capabilities = Arc::new(UiCapabilities::default());
        capabilities.set(&[CAPABILITY_LIVE_PROGRESS.to_string()]);
        capabilities
    }

    /// A response with text, reasoning and a `write_file` call, its usage and a reply
    /// that came without streaming, through `sink`.
    fn write_a_response(sink: &mut ChannelSink, now: &std::sync::Mutex<Instant>) {
        sink.text_delta("I will write it.").unwrap();
        sink.end_block().unwrap();
        sink.reasoning_started(None, false).unwrap();
        sink.reasoning_delta("the css file").unwrap();
        sink.tool_input_started("call_1", "write_file").unwrap();
        let mut input = String::new();
        for piece in [
            r#"{"path":"#,
            r#""shop/style.css","#,
            r#""content":"body {\n"#,
            r"  margin: 0;\n",
            r#"}\n"}"#,
        ] {
            advance(now, 60);
            input.push_str(piece);
            sink.tool_input_delta("call_1", "write_file", &input)
                .unwrap();
        }
        sink.tool_input_finished("call_1", "write_file", &input)
            .unwrap();
        sink.request_usage(TokenUsage {
            input_tokens: 1200,
            output_tokens: 40,
            cache_creation_input_tokens: 3,
            cache_read_input_tokens: 1000,
        })
        .unwrap();
        sink.tool_call("call_1", "write_file", &input, false)
            .unwrap();
        sink.reply_not_streamed().unwrap();
        sink.tool_call_finished("call_1", "write_file", "written", false)
            .unwrap();
    }

    /// The events a UI that declared nothing hears: exactly those from before live
    /// progress.
    fn events_without_live_progress() -> Vec<UiEvent> {
        vec![
            UiEvent::TextDelta {
                text: "I will write it.".into(),
            },
            UiEvent::TextBlockEnd,
            UiEvent::ReasoningStarted {
                chars: None,
                redacted: false,
            },
            UiEvent::ReasoningDelta {
                text: "the css file".into(),
            },
            UiEvent::ToolCallStarted {
                id: "call_1".into(),
                name: "write_file".into(),
                input: "{\"path\":\"shop/style.css\",\"content\":\"body {\\n  margin: 0;\\n}\\n\"}"
                    .into(),
                cut_off: false,
            },
            UiEvent::ToolResult {
                id: "call_1".into(),
                name: "write_file".into(),
                output: "written".into(),
                is_error: false,
            },
        ]
    }

    #[test]
    fn without_live_progress_the_ui_hears_what_it_always_did() {
        let (clock, now) = manual_clock();
        let (tx, rx) = mpsc::channel();
        let mut sink = ChannelSink::new(tx).with_clock(clock);
        write_a_response(&mut sink, &now);
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            events_without_live_progress()
        );

        // A UI that declared other capabilities only.
        let capabilities = Arc::new(UiCapabilities::default());
        capabilities.set(&[CAPABILITY_GIT_VIEWS.to_string()]);
        let (clock, now) = manual_clock();
        let (tx, rx) = mpsc::channel();
        let mut sink = ChannelSink::with_capabilities(tx, capabilities).with_clock(clock);
        write_a_response(&mut sink, &now);
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            events_without_live_progress()
        );
    }

    /// The call shows at once; its progress at most every 100 ms and once more when it
    /// is written; the output estimate at most every 250 ms; the request's usage as
    /// reported. The events from before come in the same order around them.
    #[test]
    fn live_progress_shows_tool_input_output_estimates_and_request_usage() {
        let (clock, now) = manual_clock();
        let (tx, rx) = mpsc::channel();
        let mut sink = ChannelSink::with_capabilities(tx, live_capabilities()).with_clock(clock);
        write_a_response(&mut sink, &now);
        let events = rx.try_iter().collect::<Vec<_>>();

        let older = events
            .iter()
            .filter(|event| {
                !matches!(
                    event,
                    UiEvent::ToolInputStarted { .. }
                        | UiEvent::ToolInputProgress { .. }
                        | UiEvent::OutputProgress { .. }
                        | UiEvent::RequestUsage { .. }
                        | UiEvent::Notice { .. }
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(older, events_without_live_progress());

        let progress = |input: &str, target: Option<&str>, lines, preview: Option<&str>| {
            UiEvent::ToolInputProgress {
                id: "call_1".into(),
                name: "write_file".into(),
                bytes: input.len() as u64,
                lines,
                target: target.map(Into::into),
                preview: preview.map(Into::into),
            }
        };
        let full = "{\"path\":\"shop/style.css\",\"content\":\"body {\\n  margin: 0;\\n}\\n\"}";
        let live = events
            .iter()
            .filter(|event| !older.contains(event))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            live,
            vec![
                // "I will write it." is 16 characters: 6 tokens.
                UiEvent::OutputProgress { output_tokens: 6 },
                UiEvent::ToolInputStarted {
                    id: "call_1".into(),
                    name: "write_file".into(),
                },
                // 60 ms: the call's first progress; the estimate waits for 250 ms.
                progress(r#"{"path":"#, None, 0, None),
                // 120 ms: too soon. 180 ms: due, and the path is complete.
                progress(
                    r#"{"path":"shop/style.css","content":"body {\n"#,
                    Some("shop/style.css"),
                    1,
                    Some("body {"),
                ),
                // 240 ms: too soon. 300 ms: due, and so is the estimate: 16 + 12 + 63
                // characters.
                progress(
                    full,
                    Some("shop/style.css"),
                    3,
                    Some("body {\n  margin: 0;\n}")
                ),
                UiEvent::OutputProgress { output_tokens: 31 },
                // Written: the last progress, whatever the throttle says.
                progress(
                    full,
                    Some("shop/style.css"),
                    3,
                    Some("body {\n  margin: 0;\n}")
                ),
                UiEvent::RequestUsage {
                    usage: TokenCounts {
                        input: 1200,
                        output: 40,
                        cache_read: 1000,
                        cache_write: 3,
                    }
                },
                UiEvent::Notice {
                    message: REPLY_NOT_STREAMED_NOTICE.into(),
                    kind: None,
                },
            ]
        );
        // The notice says what happens in plain words.
        assert!(REPLY_NOT_STREAMED_NOTICE.contains("all at once"));
    }

    /// Each call has its own throttle; a UI that asks in the middle of a call hears its
    /// start before its progress, and the estimate counts what came before.
    #[test]
    fn live_progress_throttles_each_call_and_can_start_mid_call() {
        let capabilities = Arc::new(UiCapabilities::default());
        let (clock, now) = manual_clock();
        let (tx, rx) = mpsc::channel();
        let mut sink =
            ChannelSink::with_capabilities(tx, Arc::clone(&capabilities)).with_clock(clock);
        sink.tool_input_started("a", "bash").unwrap();
        sink.tool_input_delta("a", "bash", r#"{"command":"ls"#)
            .unwrap();
        assert_eq!(rx.try_iter().count(), 0, "not asked for yet");

        capabilities.set(&[CAPABILITY_LIVE_PROGRESS.to_string()]);
        sink.tool_input_started("b", "read_file").unwrap();
        sink.tool_input_delta("a", "bash", r#"{"command":"ls -la"}"#)
            .unwrap();
        sink.tool_input_delta("b", "read_file", r#"{"path":"a.txt"}"#)
            .unwrap();
        advance(&now, 20);
        sink.tool_input_delta("a", "bash", r#"{"command":"ls -la"} "#)
            .unwrap();
        let events = rx.try_iter().collect::<Vec<_>>();
        let kinds = events
            .iter()
            .map(|event| match event {
                UiEvent::ToolInputStarted { id, .. } => format!("started {id}"),
                UiEvent::ToolInputProgress { id, target, .. } => {
                    format!("progress {id} {}", target.as_deref().unwrap_or("-"))
                }
                UiEvent::OutputProgress { output_tokens } => format!("output {output_tokens}"),
                other => format!("{other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            [
                "started b",
                "started a",
                "progress a ls -la",
                // The call's input, also what came before the UI asked: 20 characters.
                "output 7",
                // Its own throttle; the estimate's waits.
                "progress b a.txt",
            ]
        );
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

    /// The text a failed attempt printed stays on the terminal: a dim line of its
    /// own under it says the retry replaces it. The UI and silent output show none.
    #[test]
    fn a_discarded_reply_is_marked_on_a_dim_line_of_its_own() {
        let mut out = Vec::new();
        {
            let mut sink = AnsiSink::new(&mut out);
            sink.text_delta("Server Implementation:\n\u{2022} Im")
                .unwrap();
            sink.reply_discarded().unwrap();
        }
        let output = String::from_utf8(out).unwrap();
        assert!(
            output.ends_with("\n\x1b[2m(partial reply discarded \u{2014} retrying)\x1b[0m\n"),
            "{output:?}"
        );
        let printed = plain_output(|sink| {
            sink.text_delta("Server Implementation:\n\u{2022} Im")
                .unwrap();
            sink.reply_discarded().unwrap();
        });
        assert!(
            printed.ends_with(" Im\n(partial reply discarded \u{2014} retrying)\n"),
            "{printed:?}"
        );

        let (tx, rx) = mpsc::channel();
        ChannelSink::new(tx).reply_discarded().unwrap();
        assert_eq!(rx.try_iter().count(), 0);
        SilentSink.reply_discarded().unwrap();
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
