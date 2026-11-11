//! Notices a turn going round in circles after its tool calls ran: the same few calls
//! coming back with the same results (`A, B, A, B, A, B`), or the same lines of a file
//! read again and again and coming back unchanged. [`super::LoopGuard`] catches the
//! plainest loop, one call repeated with identical arguments, before it runs; this
//! watch sees what the calls returned, so polling that gets new output is not a loop.
//!
//! A read counts as a re-read only when most of the lines it returned were already
//! returned, word for word, by earlier reads. A read that shows changed lines starts
//! the file's count again, and so does any call that may change files (a shell, a
//! sub-agent, an MCP tool, ...): what was read before it may be out of date.
//!
//! The answer is graduated: the first time, a note on the tool result asks the model
//! to change its approach; when it keeps going, the turn is paused (it ends normally,
//! the work so far is kept, and the user can say "continue" or give guidance).

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};

use serde_json::Value;

use super::{tool_call_signature, FILE_EDIT_TOOLS};
use crate::file_ops::DEFAULT_READ_LINE_LIMIT;
use crate::parallel_tools::CONCURRENCY_SAFE_TOOLS;
use crate::session::{ContentBlock, ConversationMessage};

/// How many full rounds of a short cycle of calls (with the same results) count as
/// going round in circles.
const CYCLE_ROUNDS: usize = 3;
/// The cycle lengths looked for. A cycle of one call is [`super::LoopGuard`]'s.
const CYCLE_LENGTHS: [usize; 2] = [2, 3];
/// Calls remembered for cycle detection.
const HISTORY_LEN: usize = 12;
/// Reads of a part of a file already read this turn (and unchanged since) before the
/// model is told it is re-reading.
const REREAD_WARN: usize = 3;
/// Such re-reads before the turn is paused.
const REREAD_PAUSE: usize = 5;

/// What the watch makes of a call that ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WatchVerdict {
    Proceed,
    /// Keep going, with this note added to the call's result.
    Warn(String),
    /// Pause the turn after this call. `reason` says what happened, in the past tense
    /// and without a trailing period ("the model read ... 6 times ..."); `note` goes on
    /// the call's result.
    Pause {
        reason: String,
        note: String,
    },
}

/// One turn's record of the calls that ran (see the module docs).
#[derive(Debug, Default)]
pub(super) struct ProgressWatch {
    /// A hash of each recent call with its result, oldest first.
    history: VecDeque<u64>,
    cycle_warned: bool,
    /// What was read of each file since it last changed (keyed by [`FilePath::key`]),
    /// and how many reads showed nothing new.
    reads: HashMap<String, FileReads>,
    /// Set when a verdict paused the turn, until the turn loop takes it.
    pending_pause: Option<String>,
}

#[derive(Debug, Default)]
struct FileReads {
    /// A hash of each line's text as last read, by line number (1-based).
    lines: HashMap<usize, u64>,
    /// The file's length in lines as last read.
    total_lines: Option<usize>,
    /// A hash of the whole result by requested line range, for results whose lines
    /// could not be read out (not the usual JSON).
    results: HashMap<(usize, usize), u64>,
    rereads: usize,
}

impl FileReads {
    /// Records the lines a read showed; whether most of them were shown before,
    /// unchanged. Changed lines (or a changed length) start the count again.
    fn record_lines(&mut self, shown: &ShownLines) -> bool {
        let line_number = |index: usize| shown.start_line.saturating_add(index);
        let changed = self
            .total_lines
            .is_some_and(|total| total != shown.total_lines)
            || shown.hashes.iter().enumerate().any(|(index, hash)| {
                self.lines
                    .get(&line_number(index))
                    .is_some_and(|old| old != hash)
            });
        if changed {
            *self = Self::default();
        }
        let seen = shown
            .hashes
            .iter()
            .enumerate()
            .filter(|(index, hash)| self.lines.get(&line_number(*index)) == Some(*hash))
            .count();
        for (index, hash) in shown.hashes.iter().enumerate() {
            self.lines.insert(line_number(index), *hash);
        }
        self.total_lines = Some(shown.total_lines);
        // More than half of it already seen: a window that shares a line or two with
        // the one before is still reading on.
        !shown.hashes.is_empty() && seen * 2 > shown.hashes.len()
    }

    /// Records a result whose lines could not be read out; whether the same range
    /// returned exactly this before. A different result starts the count again.
    fn record_result(&mut self, range: (usize, usize), output: &str) -> bool {
        let hash = hash_of(output);
        match self.results.insert(range, hash) {
            Some(old) if old == hash => true,
            Some(_) => {
                *self = Self::default();
                self.results.insert(range, hash);
                false
            }
            None => false,
        }
    }
}

/// Tools that cannot change a file the turn reads. Any other call (a shell, a
/// sub-agent, an MCP tool, ...) may have, so what was read before it is out of date.
fn leaves_files_alone(tool_name: &str) -> bool {
    CONCURRENCY_SAFE_TOOLS.contains(&tool_name) || tool_name == "TodoWrite"
}

impl ProgressWatch {
    /// Looks at a call that ran and its result `message`; a [`WatchVerdict::Pause`]
    /// is also kept for [`Self::take_pause`].
    pub(super) fn observe(
        &mut self,
        tool_name: &str,
        input: &str,
        message: &ConversationMessage,
    ) -> WatchVerdict {
        let (output, is_error) = result_of(message);
        let input_value = serde_json::from_str::<Value>(input).ok();
        if !is_error && FILE_EDIT_TOOLS.contains(&tool_name) {
            // An edit is progress: what was read of the file is out of date, and the
            // calls before it are not a cycle with the ones after.
            if let Some(path) = input_value.as_ref().and_then(path_of) {
                self.reads.remove(&path.key);
            }
            self.history.clear();
            self.cycle_warned = false;
            return WatchVerdict::Proceed;
        }
        if !leaves_files_alone(tool_name) {
            // It may have changed any file, even when it failed part way.
            self.reads.clear();
        }
        let verdict = self.observe_cycle(tool_name, input, output, is_error);
        let verdict = match verdict {
            WatchVerdict::Proceed if !is_error && tool_name == "read_file" => {
                input_value.as_ref().map_or(WatchVerdict::Proceed, |value| {
                    self.observe_read(value, output)
                })
            }
            verdict => verdict,
        };
        if let WatchVerdict::Pause { reason, .. } = &verdict {
            self.pending_pause = Some(reason.clone());
        }
        verdict
    }

    /// Why the turn should pause now, once.
    pub(super) fn take_pause(&mut self) -> Option<String> {
        self.pending_pause.take()
    }

    fn observe_cycle(
        &mut self,
        tool_name: &str,
        input: &str,
        output: &str,
        is_error: bool,
    ) -> WatchVerdict {
        let mut hasher = DefaultHasher::new();
        tool_call_signature(tool_name, input).hash(&mut hasher);
        output.hash(&mut hasher);
        is_error.hash(&mut hasher);
        self.history.push_back(hasher.finish());
        while self.history.len() > HISTORY_LEN {
            self.history.pop_front();
        }
        let Some(length) = CYCLE_LENGTHS
            .into_iter()
            .find(|length| repeats_cycle(&self.history, *length))
        else {
            self.cycle_warned = false;
            return WatchVerdict::Proceed;
        };
        if !std::mem::replace(&mut self.cycle_warned, true) {
            return WatchVerdict::Warn(format!(
                "[progress check] Your last {} tool calls repeat the same {length} calls {CYCLE_ROUNDS} times, and they returned the same results each time. Repeating them will not change anything: use what they returned, try a different approach, or tell the user what is blocking you. If you keep repeating them, the turn will be paused.",
                length * CYCLE_ROUNDS
            ));
        }
        WatchVerdict::Pause {
            reason: format!(
                "the model kept repeating the same {length} tool calls, which returned the same results each time"
            ),
            note: "[progress check] The turn has been paused: these calls keep repeating with the same results.".to_string(),
        }
    }

    fn observe_read(&mut self, input: &Value, output: &str) -> WatchVerdict {
        let Some(path) = path_of(input) else {
            return WatchVerdict::Proceed;
        };
        let reads = self.reads.entry(path.key).or_default();
        let reread = if let Some(shown) = shown_lines(output) {
            reads.record_lines(&shown)
        } else {
            let offset = number(input, "offset").unwrap_or(0);
            let limit = number(input, "limit")
                .filter(|limit| *limit > 0)
                .unwrap_or(DEFAULT_READ_LINE_LIMIT);
            reads.record_result((offset, offset.saturating_add(limit)), output)
        };
        if !reread {
            return WatchVerdict::Proceed;
        }
        reads.rereads += 1;
        let times = reads.rereads + 1;
        let shown = path.shown;
        if reads.rereads >= REREAD_PAUSE {
            return WatchVerdict::Pause {
                reason: format!(
                    "the model read the same part of {shown} {times} times and it had not changed"
                ),
                note: format!(
                    "[progress check] The turn has been paused: the same part of {shown} was read {times} times and came back unchanged each time."
                ),
            };
        }
        if reads.rereads == REREAD_WARN {
            return WatchVerdict::Warn(format!(
                "[progress check] You have now read this part of {shown} {times} times in this turn and it came back unchanged each time. Do not read it again: work from what you already have (write down the details you need in your reply if an earlier result was shortened), then take the next step. Reading it again while it is unchanged will pause the turn."
            ));
        }
        WatchVerdict::Proceed
    }
}

/// The lines a `read_file` result showed, hashed one by one.
struct ShownLines {
    /// The first line's number (1-based).
    start_line: usize,
    /// The file's length in lines.
    total_lines: usize,
    hashes: Vec<u64>,
}

/// The lines of a `read_file` result, or `None` when it is not the usual JSON (cut
/// short to fit the context, for one). Notes added after the JSON are ignored.
fn shown_lines(output: &str) -> Option<ShownLines> {
    let value = serde_json::Deserializer::from_str(output)
        .into_iter::<Value>()
        .next()?
        .ok()?;
    let file = value.get("file")?;
    let content = file.get("content")?.as_str()?;
    let hashes = if content.is_empty() {
        Vec::new()
    } else {
        content.split('\n').map(hash_of).collect()
    };
    Some(ShownLines {
        start_line: number(file, "startLine")?,
        total_lines: number(file, "totalLines")?,
        hashes,
    })
}

fn hash_of(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Whether the newest `CYCLE_ROUNDS * length` entries of `history` are one cycle of
/// `length` different-looking calls, repeated.
fn repeats_cycle(history: &VecDeque<u64>, length: usize) -> bool {
    let span = length * CYCLE_ROUNDS;
    if history.len() < span {
        return false;
    }
    let window = history
        .iter()
        .skip(history.len() - span)
        .collect::<Vec<_>>();
    let cycle = &window[..length];
    // One call over and over is not a cycle of `length`: the loop guard handles it.
    if cycle.iter().all(|entry| *entry == cycle[0]) {
        return false;
    }
    repeats_cycle_of(&window, length)
}

fn repeats_cycle_of(window: &[&u64], length: usize) -> bool {
    window
        .iter()
        .enumerate()
        .skip(length)
        .all(|(index, entry)| *entry == window[index - length])
}

/// A call result's text and whether it is an error.
fn result_of(message: &ConversationMessage) -> (&str, bool) {
    message
        .blocks
        .iter()
        .find_map(|block| match block {
            ContentBlock::ToolResult {
                output, is_error, ..
            } => Some((output.as_str(), *is_error)),
            _ => None,
        })
        .unwrap_or(("", false))
}

/// The file a call's input names.
struct FilePath {
    /// Written one way however the model spelled it: forward slashes, no leading `./`,
    /// relative to the working directory when inside it, and (on Windows) in lower case.
    key: String,
    /// As the model wrote it, with forward slashes and relative to the working
    /// directory when inside it: the form notes and pause reasons name it by, so the
    /// model never sees a wrong-case path to copy into an edit or an import.
    shown: String,
}

fn path_of(input: &Value) -> Option<FilePath> {
    let raw = ["path", "file_path", "notebook_path"]
        .iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))?
        .trim();
    if raw.is_empty() {
        return None;
    }
    let mut path = raw.replace('\\', "/");
    if let Ok(cwd) = std::env::current_dir() {
        let root = format!(
            "{}/",
            cwd.to_string_lossy()
                .replace('\\', "/")
                .trim_end_matches('/')
        );
        let inside = if cfg!(windows) {
            path.to_lowercase().starts_with(&root.to_lowercase())
        } else {
            path.starts_with(&root)
        };
        if inside {
            path = path.get(root.len()..).unwrap_or(&path).to_string();
        }
    }
    while let Some(rest) = path.strip_prefix("./") {
        path = rest.to_string();
    }
    let key = if cfg!(windows) {
        path.to_lowercase()
    } else {
        path.clone()
    };
    Some(FilePath { key, shown: path })
}

fn number(input: &Value, key: &str) -> Option<usize> {
    input
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

#[cfg(test)]
mod tests {
    use super::{ProgressWatch, WatchVerdict, REREAD_PAUSE, REREAD_WARN};
    use crate::session::ConversationMessage;

    fn result(output: &str) -> ConversationMessage {
        ConversationMessage::tool_result("id", "tool", output, false)
    }

    /// What `read_file` returns for `offset`/`limit` of a file of `total` lines whose
    /// line `n` reads `{text} n`.
    fn file_output(offset: usize, limit: usize, total: usize, text: &str) -> String {
        let start = offset.min(total);
        let end = (start + limit).min(total);
        let content = (start + 1..=end)
            .map(|number| format!("{text} {number}"))
            .collect::<Vec<_>>()
            .join("\n");
        serde_json::to_string_pretty(&serde_json::json!({
            "type": "text",
            "file": {
                "filePath": "file",
                "content": content,
                "numLines": end - start,
                "startLine": start + 1,
                "totalLines": total,
            }
        }))
        .unwrap()
    }

    /// A read of an unchanging 5000-line file, as `read_file` would answer it.
    fn read(watch: &mut ProgressWatch, input: &str) -> WatchVerdict {
        read_returning(watch, input, "line")
    }

    fn read_returning(watch: &mut ProgressWatch, input: &str, text: &str) -> WatchVerdict {
        let value = serde_json::from_str::<serde_json::Value>(input).unwrap();
        let field = |key: &str| value.get(key).and_then(serde_json::Value::as_u64);
        let offset = usize::try_from(field("offset").unwrap_or(0)).unwrap();
        let limit = usize::try_from(field("limit").unwrap_or(2_000)).unwrap();
        let output = file_output(offset, limit, 5_000, text);
        watch.observe("read_file", input, &result(&output))
    }

    fn assert_never_paused(verdicts: impl IntoIterator<Item = WatchVerdict>) {
        for verdict in verdicts {
            assert!(
                !matches!(verdict, WatchVerdict::Pause { .. }),
                "unexpected pause: {verdict:?}"
            );
        }
    }

    #[test]
    fn reading_the_same_lines_again_warns_then_pauses() {
        let mut watch = ProgressWatch::default();
        let input = r#"{"path":"src/app.py"}"#;
        assert_eq!(read(&mut watch, input), WatchVerdict::Proceed);
        for _ in 1..REREAD_WARN {
            assert_eq!(read(&mut watch, input), WatchVerdict::Proceed);
        }
        let WatchVerdict::Warn(note) = read(&mut watch, input) else {
            panic!("expected a warning");
        };
        assert!(note.contains("src/app.py 4 times"), "{note}");
        assert!(note.contains("came back unchanged"), "{note}");
        for _ in REREAD_WARN + 1..REREAD_PAUSE {
            assert_eq!(read(&mut watch, input), WatchVerdict::Proceed);
        }
        let WatchVerdict::Pause { reason, .. } = read(&mut watch, input) else {
            panic!("expected a pause");
        };
        assert_eq!(
            reason,
            "the model read the same part of src/app.py 6 times and it had not changed"
        );
        assert_eq!(watch.take_pause(), Some(reason));
        assert_eq!(watch.take_pause(), None);
    }

    #[test]
    fn mostly_covered_ranges_and_spellings_of_one_path_count_as_the_same_read() {
        let mut watch = ProgressWatch::default();
        let inputs = [
            r#"{"path":"src/app.py","offset":0,"limit":100}"#,
            r#"{"path":"./src/app.py","offset":10,"limit":100}"#,
            r#"{"path":"src\\app.py","offset":20,"limit":80}"#,
        ];
        for input in inputs {
            assert_eq!(read(&mut watch, input), WatchVerdict::Proceed);
        }
        assert!(matches!(
            read(&mut watch, r#"{"path":"src/app.py","offset":10,"limit":5}"#),
            WatchVerdict::Warn(_)
        ));
    }

    #[test]
    fn reading_a_file_in_chunks_is_not_rereading_it() {
        let mut watch = ProgressWatch::default();
        for chunk in 0..20 {
            let input = format!(
                r#"{{"path":"big.log","offset":{},"limit":100}}"#,
                chunk * 100
            );
            assert_eq!(read(&mut watch, &input), WatchVerdict::Proceed);
        }
    }

    #[test]
    fn chunks_that_share_a_line_are_not_rereading() {
        // A 1-based offset habit: each window repeats the last line of the one before.
        let mut watch = ProgressWatch::default();
        for chunk in 0..20 {
            let input = format!(
                r#"{{"path":"big.log","offset":{},"limit":100}}"#,
                chunk * 99
            );
            assert_eq!(read(&mut watch, &input), WatchVerdict::Proceed);
        }
    }

    #[test]
    fn sliding_windows_are_not_rereading() {
        let mut watch = ProgressWatch::default();
        for window in 0..20 {
            let input = format!(
                r#"{{"path":"big.log","offset":{},"limit":200}}"#,
                window * 180
            );
            assert_eq!(read(&mut watch, &input), WatchVerdict::Proceed);
        }
    }

    #[test]
    fn polling_a_file_that_changes_is_not_rereading() {
        // A sub-agent still writing its output file: each read shows something new.
        let mut watch = ProgressWatch::default();
        let input = r#"{"path":"agent-output.md"}"#;
        assert_never_paused((0..12).map(|round| {
            let verdict = read_returning(&mut watch, input, &format!("round {round}"));
            assert_eq!(verdict, WatchVerdict::Proceed);
            verdict
        }));
        // Once it stops changing, the count starts from the last read.
        for _ in 1..REREAD_WARN {
            assert_eq!(
                read_returning(&mut watch, input, "round 11"),
                WatchVerdict::Proceed
            );
        }
        assert!(matches!(
            read_returning(&mut watch, input, "round 11"),
            WatchVerdict::Warn(_)
        ));
    }

    #[test]
    fn reading_a_log_rewritten_by_a_shell_is_not_rereading() {
        // Edit one file, run the tests into a log, read the log: the log may well read
        // the same each time, but the shell wrote it again.
        let mut watch = ProgressWatch::default();
        let log = r#"{"path":"out.txt"}"#;
        for round in 0..10 {
            watch.observe(
                "edit_file",
                &format!(r#"{{"path":"src/a.rs","old_string":"{round}","new_string":"x"}}"#),
                &result("ok"),
            );
            watch.observe("bash", r#"{"command":"cargo test > out.txt"}"#, &result(""));
            assert_eq!(read(&mut watch, log), WatchVerdict::Proceed);
        }
    }

    #[test]
    fn any_call_that_may_change_files_starts_the_counts_again() {
        let mut watch = ProgressWatch::default();
        let input = r#"{"path":"notes.md"}"#;
        for tool in ["PowerShell", "Agent", "mcp__fs__write", "bash"] {
            for _ in 0..REREAD_WARN {
                assert_eq!(read(&mut watch, input), WatchVerdict::Proceed);
            }
            watch.observe(tool, &format!(r#"{{"x":"{tool}"}}"#), &result("done"));
        }
    }

    #[test]
    fn results_that_are_not_the_usual_json_are_compared_whole() {
        let mut watch = ProgressWatch::default();
        let input = r#"{"path":"src/app.py"}"#;
        let mut read_raw = |output: &str| watch.observe("read_file", input, &result(output));
        for _ in 0..REREAD_WARN {
            assert_eq!(read_raw("cut short"), WatchVerdict::Proceed);
        }
        assert_eq!(read_raw("something else"), WatchVerdict::Proceed);
        for _ in 0..REREAD_WARN - 1 {
            assert_eq!(read_raw("something else"), WatchVerdict::Proceed);
        }
        assert!(matches!(read_raw("something else"), WatchVerdict::Warn(_)));
    }

    #[test]
    fn notes_name_the_path_as_the_model_wrote_it() {
        let mut watch = ProgressWatch::default();
        let input = r#"{"path":"src/components/UserProfile.tsx"}"#;
        let verdicts = (0..=REREAD_PAUSE)
            .map(|_| read(&mut watch, input))
            .collect::<Vec<_>>();
        let Some(WatchVerdict::Pause { reason, note }) = verdicts.last() else {
            panic!("expected a pause: {verdicts:?}");
        };
        assert!(
            reason.contains("src/components/UserProfile.tsx"),
            "{reason}"
        );
        assert!(note.contains("src/components/UserProfile.tsx"), "{note}");
    }

    #[test]
    fn an_edit_to_the_file_starts_its_count_again() {
        let mut watch = ProgressWatch::default();
        let input = r#"{"path":"src/app.py"}"#;
        for _ in 0..REREAD_WARN {
            read(&mut watch, input);
        }
        watch.observe(
            "edit_file",
            r#"{"path":"src/app.py","old_string":"a","new_string":"b"}"#,
            &result("ok"),
        );
        for _ in 0..REREAD_WARN {
            assert_eq!(read(&mut watch, input), WatchVerdict::Proceed);
        }
    }

    #[test]
    fn a_cycle_of_calls_with_the_same_results_warns_then_pauses() {
        let mut watch = ProgressWatch::default();
        let mut call = |name: &str, input: &str| watch.observe(name, input, &result("unchanged"));
        for _ in 0..2 {
            assert_eq!(
                call("bash", r#"{"command":"cargo build"}"#),
                WatchVerdict::Proceed
            );
            assert_eq!(
                call("grep_search", r#"{"pattern":"fn main"}"#),
                WatchVerdict::Proceed
            );
        }
        assert_eq!(
            call("bash", r#"{"command":"cargo build"}"#),
            WatchVerdict::Proceed
        );
        assert!(matches!(
            call("grep_search", r#"{"pattern":"fn main"}"#),
            WatchVerdict::Warn(note) if note.contains("same 2 calls 3 times")
        ));
        assert!(matches!(
            call("bash", r#"{"command":"cargo build"}"#),
            WatchVerdict::Pause { reason, .. } if reason.contains("same 2 tool calls")
        ));
    }

    #[test]
    fn polling_that_gets_new_output_is_not_a_cycle() {
        let mut watch = ProgressWatch::default();
        for round in 0..6 {
            assert_eq!(
                watch.observe(
                    "bash",
                    r#"{"command":"tail build.log"}"#,
                    &result(&format!("line {round}"))
                ),
                WatchVerdict::Proceed
            );
            assert_eq!(
                watch.observe("bash", r#"{"command":"sleep 5"}"#, &result("")),
                WatchVerdict::Proceed
            );
        }
    }

    #[test]
    fn one_call_repeated_is_left_to_the_loop_guard() {
        let mut watch = ProgressWatch::default();
        for _ in 0..8 {
            assert_eq!(
                watch.observe("bash", r#"{"command":"ls"}"#, &result("a b")),
                WatchVerdict::Proceed
            );
        }
    }
}
