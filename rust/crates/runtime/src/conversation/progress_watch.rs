//! Notices a turn going round in circles after its tool calls ran: the same few calls
//! coming back with the same results (`A, B, A, B, A, B`), or the same part of a file
//! read again and again without anything changing it. [`super::LoopGuard`] catches the
//! plainest loop, one call repeated with identical arguments, before it runs; this
//! watch sees what the calls returned, so polling that gets new output is not a loop.
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
use crate::session::{ContentBlock, ConversationMessage};

/// How many full rounds of a short cycle of calls (with the same results) count as
/// going round in circles.
const CYCLE_ROUNDS: usize = 3;
/// The cycle lengths looked for. A cycle of one call is [`super::LoopGuard`]'s.
const CYCLE_LENGTHS: [usize; 2] = [2, 3];
/// Calls remembered for cycle detection.
const HISTORY_LEN: usize = 12;
/// Reads of a part of a file already read this turn (with no edit to it since) before
/// the model is told it is re-reading.
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
    /// Line ranges read per file since it was last edited, and how many reads overlapped
    /// an earlier one.
    reads: HashMap<String, FileReads>,
    /// Set when a verdict paused the turn, until the turn loop takes it.
    pending_pause: Option<String>,
}

#[derive(Debug, Default)]
struct FileReads {
    ranges: Vec<(usize, usize)>,
    rereads: usize,
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
                self.reads.remove(&path);
            }
            self.history.clear();
            self.cycle_warned = false;
            return WatchVerdict::Proceed;
        }
        let verdict = self.observe_cycle(tool_name, input, output, is_error);
        let verdict = match verdict {
            WatchVerdict::Proceed if !is_error && tool_name == "read_file" => input_value
                .as_ref()
                .map_or(WatchVerdict::Proceed, |value| self.observe_read(value)),
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

    fn observe_read(&mut self, input: &Value) -> WatchVerdict {
        let Some(path) = path_of(input) else {
            return WatchVerdict::Proceed;
        };
        let offset = number(input, "offset").unwrap_or(0);
        let limit = number(input, "limit")
            .filter(|limit| *limit > 0)
            .unwrap_or(DEFAULT_READ_LINE_LIMIT);
        let range = (offset, offset.saturating_add(limit));
        let reads = self.reads.entry(path.clone()).or_default();
        let overlaps = reads
            .ranges
            .iter()
            .any(|(start, end)| range.0 < *end && *start < range.1);
        reads.ranges.push(range);
        if !overlaps {
            return WatchVerdict::Proceed;
        }
        reads.rereads += 1;
        let times = reads.rereads + 1;
        if reads.rereads >= REREAD_PAUSE {
            return WatchVerdict::Pause {
                reason: format!(
                    "the model read the same part of {path} {times} times without changing the file"
                ),
                note: format!(
                    "[progress check] The turn has been paused: {path} was read {times} times without being changed."
                ),
            };
        }
        if reads.rereads == REREAD_WARN {
            return WatchVerdict::Warn(format!(
                "[progress check] You have now read this part of {path} {times} times in this turn without changing the file, so its content is the same as before. Do not read it again: work from what you already have (write down the details you need in your reply if an earlier result was shortened), then take the next step. Reading it again without changing it will pause the turn."
            ));
        }
        WatchVerdict::Proceed
    }
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

/// The file a call's input names, written one way however the model spelled it:
/// forward slashes, no leading `./`, relative to the working directory when inside it,
/// and (on Windows) in lower case.
fn path_of(input: &Value) -> Option<String> {
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
            path = path[root.len()..].to_string();
        }
    }
    while let Some(rest) = path.strip_prefix("./") {
        path = rest.to_string();
    }
    Some(if cfg!(windows) {
        path.to_lowercase()
    } else {
        path
    })
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

    fn read(watch: &mut ProgressWatch, input: &str) -> WatchVerdict {
        watch.observe("read_file", input, &result("same content"))
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
        for _ in REREAD_WARN + 1..REREAD_PAUSE {
            assert_eq!(read(&mut watch, input), WatchVerdict::Proceed);
        }
        let WatchVerdict::Pause { reason, .. } = read(&mut watch, input) else {
            panic!("expected a pause");
        };
        assert_eq!(
            reason,
            "the model read the same part of src/app.py 6 times without changing the file"
        );
        assert_eq!(watch.take_pause(), Some(reason));
        assert_eq!(watch.take_pause(), None);
    }

    #[test]
    fn overlapping_ranges_and_spellings_of_one_path_count_as_the_same_read() {
        let mut watch = ProgressWatch::default();
        let inputs = [
            r#"{"path":"src/app.py","offset":0,"limit":100}"#,
            r#"{"path":"./src/app.py","offset":50,"limit":100}"#,
            r#"{"path":"src\\app.py","offset":90,"limit":20}"#,
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
