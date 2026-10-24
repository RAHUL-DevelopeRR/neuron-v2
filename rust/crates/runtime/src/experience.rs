//! What Neuron has learned about each model from earlier turns: how often its tool
//! arguments needed repair, its edits missed, it stopped on a promise instead of a
//! tool call, it repeated itself, or its changes failed the project's checks.
//!
//! [`ConversationRuntime`](crate::ConversationRuntime) records a turn's
//! [`HarnessStats`] under the model that answered and, once a model has enough
//! history, adds the matching [`notes_from`] to its system prompt, so a model that
//! keeps making one mistake is told about it up front. The file is
//! `experience.json` in Neuron's config directory; `NEURON_EXPERIENCE=0` turns
//! this off.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `0`/`false` turns the experience library off.
pub const EXPERIENCE_ENV_VAR: &str = "NEURON_EXPERIENCE";
/// Heading of the system prompt section the notes go in.
pub const MODEL_NOTES_HEADING: &str = "# Notes for this model";
const FILE_NAME: &str = "experience.json";
/// History a model needs before any note is given: enough calls that a rate means
/// something.
const MIN_TOOL_CALLS: u64 = 20;
const MIN_TURNS: u64 = 3;

/// Harness events in one turn, or summed over a model's turns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HarnessStats {
    pub turns: u64,
    pub tool_calls: u64,
    pub tool_errors: u64,
    /// `edit_file` calls, and those whose `old_string` did not match.
    pub edits: u64,
    pub edit_misses: u64,
    /// Tool arguments that were repaired, or were not JSON at all.
    pub repaired_inputs: u64,
    pub invalid_inputs: u64,
    /// Calls to tools that do not exist.
    pub unknown_tools: u64,
    pub loop_warnings: u64,
    /// Turns that ended on a promise of a next step (see the action nudge).
    pub nudges: u64,
    /// Tool calls cut off by the output token limit.
    pub truncated_calls: u64,
    pub completion_checks: u64,
    /// Project checks that failed during completion checks.
    pub failed_checks: u64,
}

impl HarnessStats {
    pub fn add(&mut self, other: &Self) {
        self.turns += other.turns;
        self.tool_calls += other.tool_calls;
        self.tool_errors += other.tool_errors;
        self.edits += other.edits;
        self.edit_misses += other.edit_misses;
        self.repaired_inputs += other.repaired_inputs;
        self.invalid_inputs += other.invalid_inputs;
        self.unknown_tools += other.unknown_tools;
        self.loop_warnings += other.loop_warnings;
        self.nudges += other.nudges;
        self.truncated_calls += other.truncated_calls;
        self.completion_checks += other.completion_checks;
        self.failed_checks += other.failed_checks;
    }

    /// Counts one tool result: the call, whether it failed and, from the text
    /// Neuron puts in results, which harness rule it hit.
    pub fn observe_tool_result(&mut self, tool_name: &str, output: &str, is_error: bool) {
        self.tool_calls += 1;
        if is_error {
            self.tool_errors += 1;
        }
        if tool_name == "edit_file" {
            self.edits += 1;
            if is_error && output.contains("old_string") {
                self.edit_misses += 1;
            }
        }
        if output.contains("[note: the arguments were not valid JSON") {
            self.repaired_inputs += 1;
        }
        if is_error && output.starts_with("invalid tool input JSON") {
            self.invalid_inputs += 1;
        }
        if is_error && output.starts_with("unsupported tool:") {
            self.unknown_tools += 1;
        }
    }

    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The notes a model's history calls for, most important first. Empty until the
/// model has [`MIN_TURNS`] turns and [`MIN_TOOL_CALLS`] tool calls on record.
#[must_use]
pub fn notes_from(stats: &HarnessStats) -> Vec<&'static str> {
    if stats.turns < MIN_TURNS || stats.tool_calls < MIN_TOOL_CALLS {
        return Vec::new();
    }
    let percent = |part: u64, whole: u64| (part * 100).checked_div(whole).unwrap_or(0);
    let mut notes = Vec::new();
    if percent(
        stats.repaired_inputs + stats.invalid_inputs + stats.unknown_tools,
        stats.tool_calls,
    ) >= 5
    {
        notes.push("Send each tool call's arguments as one valid JSON object: no code fences or comments, newlines inside strings escaped as \\n, and only the tool names you were given.");
    }
    if stats.edits >= 5 && percent(stats.edit_misses, stats.edits) >= 20 {
        notes.push("Before edit_file, read the lines you will replace and copy old_string from that output exactly, including indentation; include enough surrounding lines to make it unique.");
    }
    if percent(stats.nudges, stats.turns) >= 20 {
        notes.push("Never end a reply with a plan or a promise such as \"I'll now…\": make the tool call in the same reply, or give the final answer.");
    }
    if percent(stats.loop_warnings, stats.turns) >= 15 {
        notes.push("When a tool call fails twice, do not repeat it: read the error, then change the arguments or the approach.");
    }
    if stats.completion_checks >= 3 && percent(stats.failed_checks, stats.completion_checks) >= 30 {
        notes.push("Run the project's tests or type checker after each change, not only at the end, and fix failures before moving on.");
    }
    if stats.truncated_calls >= 2 {
        notes.push("Keep each tool call small: change existing files with edit_file and write large new files in parts, so no call hits the output limit.");
    }
    notes
}

/// The system prompt section for `notes`, or `None` when there are none.
#[must_use]
pub fn render_model_notes(model: &str, notes: &[&str]) -> Option<String> {
    if notes.is_empty() {
        return None;
    }
    let mut section = format!("{MODEL_NOTES_HEADING}\nFrom earlier sessions with `{model}`:");
    for note in notes {
        section.push_str("\n - ");
        section.push_str(note);
    }
    Some(section)
}

/// Per-model [`HarnessStats`], kept in a JSON file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperienceStore {
    path: PathBuf,
}

impl ExperienceStore {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `experience.json` in Neuron's config directory, unless
    /// [`EXPERIENCE_ENV_VAR`] turns the library off.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let disabled = std::env::var(EXPERIENCE_ENV_VAR).is_ok_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        });
        (!disabled).then(|| Self::new(crate::config::default_config_home().join(FILE_NAME)))
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every model's history; empty when the file is missing or unreadable.
    #[must_use]
    pub fn load(&self) -> BTreeMap<String, HarnessStats> {
        fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// `model`'s history.
    #[must_use]
    pub fn stats_for(&self, model: &str) -> HarnessStats {
        self.load()
            .get(&model_key(model))
            .copied()
            .unwrap_or_default()
    }

    /// Adds a turn's `stats` to `model`'s history. The file is re-read first, so
    /// concurrent sessions lose at most one turn's counts.
    pub fn record(&self, model: &str, stats: &HarnessStats) -> io::Result<()> {
        if stats.is_empty() || model.trim().is_empty() {
            return Ok(());
        }
        let mut all = self.load();
        all.entry(model_key(model)).or_default().add(stats);
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(&all).map_err(io::Error::other)?;
        let staging = self.path.with_extension("json.tmp");
        fs::write(&staging, text + "\n")?;
        fs::rename(&staging, &self.path)
    }
}

/// Models are recorded by their id, without the gateway transport prefix.
fn model_key(model: &str) -> String {
    let model = model.trim();
    model
        .strip_prefix("openai/")
        .unwrap_or(model)
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::{notes_from, render_model_notes, ExperienceStore, HarnessStats};

    fn temp_store(label: &str) -> ExperienceStore {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        ExperienceStore::new(
            std::env::temp_dir()
                .join(format!("neuron-experience-{label}-{nanos}"))
                .join("experience.json"),
        )
    }

    #[test]
    fn tool_results_are_classified_by_the_rule_they_hit() {
        let mut stats = HarnessStats::default();
        stats.observe_tool_result("edit_file", "old_string not found in file", true);
        stats.observe_tool_result("edit_file", "{\"ok\":true}", false);
        stats.observe_tool_result("read_file", "text\n\n[note: the arguments were not valid JSON; Neuron removed a trailing comma. Send them as valid JSON next time.]", false);
        stats.observe_tool_result(
            "bash",
            "invalid tool input JSON (EOF). Send the arguments as one JSON object",
            true,
        );
        stats.observe_tool_result(
            "raed_file",
            "unsupported tool: `raed_file` does not exist.",
            true,
        );
        assert_eq!(
            (
                stats.tool_calls,
                stats.tool_errors,
                stats.edits,
                stats.edit_misses
            ),
            (5, 3, 2, 1)
        );
        assert_eq!(
            (
                stats.repaired_inputs,
                stats.invalid_inputs,
                stats.unknown_tools
            ),
            (1, 1, 1)
        );
    }

    #[test]
    fn notes_need_enough_history_and_a_high_enough_rate() {
        let mut stats = HarnessStats {
            turns: 2,
            tool_calls: 40,
            edits: 10,
            edit_misses: 5,
            ..HarnessStats::default()
        };
        assert!(notes_from(&stats).is_empty(), "too few turns");
        stats.turns = 10;
        let notes = notes_from(&stats);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("copy old_string"));

        stats.edit_misses = 1;
        stats.nudges = 3;
        stats.repaired_inputs = 3;
        let notes = notes_from(&stats);
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].contains("valid JSON object"));
        assert!(notes[1].contains("Never end a reply"));

        let section = render_model_notes("nemotron", &notes).expect("section");
        assert!(section
            .starts_with("# Notes for this model\nFrom earlier sessions with `nemotron`:\n - "));
        assert!(render_model_notes("nemotron", &[]).is_none());
    }

    #[test]
    fn the_store_sums_turns_per_model() {
        let store = temp_store("sum");
        let turn = HarnessStats {
            turns: 1,
            tool_calls: 4,
            edits: 2,
            edit_misses: 1,
            ..HarnessStats::default()
        };
        store.record("openai/Nemotron-X", &turn).expect("record");
        store.record("nemotron-x", &turn).expect("record");
        store
            .record("other", &HarnessStats::default())
            .expect("empty turns are skipped");

        let all = store.load();
        assert_eq!(all.len(), 1, "{all:?}");
        let stats = store.stats_for("NEMOTRON-X");
        assert_eq!(
            (stats.turns, stats.tool_calls, stats.edit_misses),
            (2, 8, 2)
        );
        let text = std::fs::read_to_string(store.path()).expect("file");
        assert!(text.contains("\"editMisses\": 2"), "{text}");
    }
}
