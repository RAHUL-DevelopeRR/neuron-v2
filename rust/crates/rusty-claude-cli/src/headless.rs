//! A `neuron prompt` run that nobody can answer: stdin is not a terminal (a script,
//! CI, `< /dev/null`). A tool call that needs approval is not asked about: it is denied
//! with a reason the model and the user both see. A question from the model fails, so
//! the model decides itself. The run ends with a warning naming what was not run, and
//! exit code [`DENIED_EXIT_CODE`] when tool calls were denied.
//!
//! The line REPL and an interactive `neuron prompt` (stdin a terminal) prompt as before.
//! Any `neuron prompt` run, attended or not, ends with its turn: recovery hints name CLI
//! flags there, not REPL commands ([`one_shot`]).

use std::io::{self, IsTerminal};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use runtime::PermissionMode;
use serde_json::json;

use crate::args::CliOutputFormat;

/// The exit code of a run that finished with tool calls denied for lack of a prompt.
pub(crate) const DENIED_EXIT_CODE: i32 = 2;

/// What `AskUserQuestion` answers in a non-interactive run (as its error).
pub(crate) const QUESTION_REFUSAL: &str =
    "no one can answer questions in a non-interactive run; decide yourself and say what you assumed";

static ONE_SHOT: AtomicBool = AtomicBool::new(false);
/// `--output-format json`: stderr carries JSON only, so denials are not noticed as text.
static JSON: AtomicBool = AtomicBool::new(false);
static DENIED: Mutex<Vec<String>> = Mutex::new(Vec::new());
static UNANSWERED: AtomicUsize = AtomicUsize::new(0);

/// Whether nobody can answer a prompt here: stdin is not a terminal.
pub(crate) fn stdin_is_unattended() -> bool {
    !io::stdin().is_terminal()
}

/// Starts a one-shot `neuron prompt` run ([`one_shot`] is true from now on). When it
/// is `unattended`, questions from the model fail.
pub(crate) fn start(unattended: bool, output_format: CliOutputFormat) {
    ONE_SHOT.store(true, Ordering::SeqCst);
    JSON.store(output_format == CliOutputFormat::Json, Ordering::SeqCst);
    if unattended {
        runtime::ui_hooks::set_ask_handler(|_question| {
            UNANSWERED.fetch_add(1, Ordering::SeqCst);
            Err(io::Error::other(QUESTION_REFUSAL))
        });
    }
}

/// Whether this is a one-shot `neuron prompt` run: there is no REPL to type a slash
/// command into afterwards.
pub(crate) fn one_shot() -> bool {
    ONE_SHOT.load(Ordering::SeqCst)
}

/// Why `tool_name` was not run: nobody could approve it. What would let it run is
/// the mode it needs, or a settings rule when the mode is not the reason.
pub(crate) fn denial_reason(
    tool_name: &str,
    current_mode: PermissionMode,
    required_mode: PermissionMode,
) -> String {
    let fix = if required_mode > current_mode && required_mode != PermissionMode::Prompt {
        format!(
            "re-run with --permission-mode {}, or allow the tool in settings (`permissions.allow`)",
            required_mode.as_str()
        )
    } else {
        "allow the tool in settings (`permissions.allow`)".to_string()
    };
    format!("tool '{tool_name}' was not run: non-interactive run: {fix}")
}

/// Records a tool call denied because nobody could approve it, and tells the user now
/// (the run's last warning names it again; in JSON mode only that warning, as JSON).
pub(crate) fn record_denial(tool_name: &str, reason: &str) {
    DENIED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(tool_name.to_string());
    if !JSON.load(Ordering::SeqCst) {
        runtime::ui_hooks::notice(reason);
    }
}

/// What a non-interactive run could not do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Shortfall {
    /// The tools of the denied calls, in order (a tool denied twice is listed twice).
    pub denied: Vec<String>,
    /// Questions from the model that nobody answered.
    pub unanswered: usize,
}

impl Shortfall {
    /// The exit code the run ends with, when not 0.
    pub(crate) fn exit_code(&self) -> Option<i32> {
        (!self.denied.is_empty()).then_some(DENIED_EXIT_CODE)
    }

    /// The end-of-run warning, or `None` when nothing was left undone.
    pub(crate) fn warning(&self) -> Option<String> {
        let mut parts = Vec::new();
        if !self.denied.is_empty() {
            let calls = self.denied.len();
            parts.push(format!(
                "{calls} tool call{} denied because nobody could approve {} ({}). Re-run with --permission-mode danger-full-access, or allow the tools in settings (`permissions.allow`).",
                if calls == 1 { " was" } else { "s were" },
                if calls == 1 { "it" } else { "them" },
                tool_counts(&self.denied),
            ));
        }
        if self.unanswered > 0 {
            parts.push(format!(
                "{} question{} from the model went unanswered; it was told to decide itself.",
                self.unanswered,
                if self.unanswered == 1 { "" } else { "s" },
            ));
        }
        (!parts.is_empty()).then(|| format!("warning: non-interactive run: {}", parts.join(" ")))
    }

    /// The warning as `--output-format json` prints it on stderr.
    pub(crate) fn warning_json(&self) -> Option<serde_json::Value> {
        let message = self.warning()?;
        Some(json!({
            "type": "warning",
            "warning": message,
            "denied_tools": self.denied,
            "unanswered_questions": self.unanswered,
        }))
    }
}

/// `bash ×2, write_file`: each tool once, with its count when above one.
fn tool_counts(tools: &[String]) -> String {
    let mut counted: Vec<(&str, usize)> = Vec::new();
    for tool in tools {
        match counted.iter_mut().find(|(name, _)| name == tool) {
            Some((_, count)) => *count += 1,
            None => counted.push((tool, 1)),
        }
    }
    counted
        .iter()
        .map(|(name, count)| {
            if *count == 1 {
                (*name).to_string()
            } else {
                format!("{name} \u{00d7}{count}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Ends the run's bookkeeping: what it could not do.
pub(crate) fn finish() -> Shortfall {
    Shortfall {
        denied: std::mem::take(&mut *DENIED.lock().unwrap_or_else(PoisonError::into_inner)),
        unanswered: UNANSWERED.swap(0, Ordering::SeqCst),
    }
}

/// Prints the end-of-run warning on stderr (JSON in JSON mode) and returns the exit
/// code the run ends with, when not 0.
pub(crate) fn report(output_format: CliOutputFormat) -> Option<i32> {
    let shortfall = finish();
    match output_format {
        CliOutputFormat::Json => {
            if let Some(warning) = shortfall.warning_json() {
                eprintln!("{warning}");
            }
        }
        CliOutputFormat::Text => {
            if let Some(warning) = shortfall.warning() {
                eprintln!("{warning}");
            }
        }
    }
    shortfall.exit_code()
}

#[cfg(test)]
mod tests {
    use super::{denial_reason, Shortfall, DENIED_EXIT_CODE};
    use runtime::PermissionMode;

    #[test]
    fn a_denial_says_what_would_let_the_tool_run() {
        let reason = denial_reason(
            "bash",
            PermissionMode::WorkspaceWrite,
            PermissionMode::DangerFullAccess,
        );
        assert_eq!(
            reason,
            "tool 'bash' was not run: non-interactive run: re-run with --permission-mode danger-full-access, or allow the tool in settings (`permissions.allow`)"
        );
        // An ask rule or a hook asked for approval: the mode is not the reason.
        let reason = denial_reason(
            "write_file",
            PermissionMode::DangerFullAccess,
            PermissionMode::WorkspaceWrite,
        );
        assert!(!reason.contains("--permission-mode"), "{reason}");
        assert!(reason.contains("permissions.allow"), "{reason}");
    }

    #[test]
    fn the_end_of_run_warning_names_the_denied_tools() {
        let none = Shortfall::default();
        assert_eq!(none.warning(), None);
        assert_eq!(none.exit_code(), None);

        let shortfall = Shortfall {
            denied: vec!["bash".into(), "write_file".into(), "bash".into()],
            unanswered: 3,
        };
        let warning = shortfall.warning().expect("a warning");
        assert!(
            warning.starts_with("warning: non-interactive run: 3 tool calls were denied"),
            "{warning}"
        );
        assert!(
            warning.contains("(bash \u{00d7}2, write_file)"),
            "{warning}"
        );
        assert!(
            warning.contains("--permission-mode danger-full-access"),
            "{warning}"
        );
        assert!(
            warning.contains("3 questions from the model went unanswered"),
            "{warning}"
        );
        assert_eq!(shortfall.exit_code(), Some(DENIED_EXIT_CODE));

        let json = shortfall.warning_json().expect("json");
        assert_eq!(json["type"], "warning");
        assert_eq!(json["denied_tools"][1], "write_file");
        assert_eq!(json["unanswered_questions"], 3);

        // Questions alone are reported, and the run still succeeds.
        let questions = Shortfall {
            denied: Vec::new(),
            unanswered: 1,
        };
        assert!(questions
            .warning()
            .is_some_and(|warning| warning.contains("1 question from the model went unanswered")));
        assert_eq!(questions.exit_code(), None);
    }
}
