//! A `neuron prompt` run that nobody can answer: stdin is not a terminal (a script,
//! CI, `< /dev/null`). A tool call that needs approval is not asked about: it is denied
//! with a reason the model and the user both see, naming the `permissions.allow` rule
//! that would let it run ([`Denial`]). A question from the model fails, so the model
//! decides itself. After [`DENIAL_LIMIT`] denied calls the turn stops. The run ends with
//! a warning naming what was not run, and exit code [`DENIED_EXIT_CODE`] when tool calls
//! were denied.
//!
//! What stdin carries (`cat log | neuron prompt "explain this log"`) is sent as context
//! with the prompt ([`stdin_context`]); a pipe that stays silent is not waited for.
//!
//! The line REPL and an interactive `neuron prompt` (stdin a terminal) prompt as before.
//! Any `neuron prompt` run, attended or not, ends with its turn: recovery hints name CLI
//! flags there, not REPL commands ([`one_shot`]).

use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use runtime::PermissionMode;
use serde_json::json;

use crate::args::CliOutputFormat;

/// The exit code of a run that finished with tool calls denied for lack of a prompt.
pub(crate) const DENIED_EXIT_CODE: i32 = 2;

/// Denied tool calls after which a non-interactive `neuron prompt` run stops. Models
/// kept calling tools nobody could approve, varying the tool or its arguments so the
/// repeated-call guard never fired, until the iteration cap or the shared quota ran out.
pub(crate) const DENIAL_LIMIT: usize = 5;

/// What `AskUserQuestion` answers in a non-interactive run (as its error).
pub(crate) const QUESTION_REFUSAL: &str =
    "no one can answer questions in a non-interactive run; decide yourself and say what you assumed";

/// How long a `neuron prompt` run waits for piped stdin to send something or end: a
/// pipe that nobody writes to or closes (a harness's) would otherwise hang the run.
pub(crate) const STDIN_WAIT: Duration = Duration::from_millis(500);

/// The most piped stdin a run sends to the model.
pub(crate) const STDIN_MAX_BYTES: usize = 128 * 1024;

static ONE_SHOT: AtomicBool = AtomicBool::new(false);
/// `--output-format json`: stderr carries JSON only, so denials are not noticed as text.
static JSON: AtomicBool = AtomicBool::new(false);
static DENIED: Mutex<DenialLog> = Mutex::new(DenialLog::new());
static UNANSWERED: AtomicUsize = AtomicUsize::new(0);

/// Whether nobody can answer a prompt here: stdin is not a terminal.
pub(crate) fn stdin_is_unattended() -> bool {
    !runtime::ui_hooks::stdin_is_terminal()
}

/// Starts a one-shot `neuron prompt` run ([`one_shot`] is true from now on). When it
/// is `unattended`, questions from the model fail. In JSON mode, notices (retries, for
/// one) reach stderr as JSON lines.
pub(crate) fn start(unattended: bool, output_format: CliOutputFormat) {
    ONE_SHOT.store(true, Ordering::SeqCst);
    let json_output = output_format == CliOutputFormat::Json;
    JSON.store(json_output, Ordering::SeqCst);
    if json_output {
        runtime::ui_hooks::set_notice_handler(|message| {
            eprintln!("{}", json!({ "type": "notice", "notice": message }));
        });
    }
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

/// Where an allow rule goes so that scripts get it: a script's workspace is often not
/// trusted (the trust question needs a terminal), and then its own allow rules are
/// ignored.
const USER_SETTINGS: &str = "~/.neuron/settings.json";

/// A tool call denied because nobody could approve it, and what would let it run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Denial {
    tool: String,
    /// `permissions.allow` rules that would let the call run ([`allow_rules`]).
    rules: Vec<String>,
    /// The mode that would let it run, when the mode is why it needed approval (not
    /// an ask rule or a hook).
    mode: Option<PermissionMode>,
}

impl Denial {
    /// The denial of `request` in `current_mode`.
    pub(crate) fn of(request: &runtime::PermissionRequest, current_mode: PermissionMode) -> Self {
        let required_mode = request.required_mode;
        Self {
            tool: request.tool_name.clone(),
            rules: allow_rules(
                &request.tool_name,
                &request.input,
                &request.session_prefixes,
            ),
            mode: (required_mode > current_mode && required_mode != PermissionMode::Prompt)
                .then_some(required_mode),
        }
    }

    /// Why the call was not run, and what would let it run: an allow rule first, the
    /// mode only for a disposable environment (Full access stays opt-in).
    pub(crate) fn reason(&self) -> String {
        format!(
            "tool '{}' was not run: nobody could approve it in a non-interactive run. To allow it in scripts, {}",
            self.tool,
            allow_hint(&self.rules, self.mode)
        )
    }
}

/// The `permissions.allow` rules that would let a call of `tool_name` run. A shell
/// tool gets one per program the command needs approval for (`bash(npm:*)`), from
/// the prefixes "allow for this session" would approve, else from the command's first
/// word; any other tool gets its name (`WebSearch`).
fn allow_rules(tool_name: &str, input: &str, session_prefixes: &[String]) -> Vec<String> {
    if !matches!(tool_name, "bash" | "PowerShell") {
        return vec![tool_name.to_string()];
    }
    let mut programs: Vec<String> = Vec::new();
    for prefix in session_prefixes {
        if let Some(program) = prefix.split_whitespace().next() {
            if !programs.iter().any(|known| known == program) {
                programs.push(program.to_string());
            }
        }
    }
    if programs.is_empty() {
        let command = serde_json::from_str::<serde_json::Value>(input)
            .ok()
            .and_then(|input| input["command"].as_str().map(str::to_string))
            .unwrap_or_default();
        programs.push(
            command
                .split_whitespace()
                .next()
                .unwrap_or("<program>")
                .to_string(),
        );
    }
    programs
        .iter()
        .map(|program| format!("{tool_name}({program}:*)"))
        .collect()
}

/// `add "bash(npm:*)", "WebSearch" to permissions.allow in your user settings (...)`,
/// then `mode`, when there is one, for a disposable environment only.
fn allow_hint(rules: &[String], mode: Option<PermissionMode>) -> String {
    let rules = rules
        .iter()
        .map(|rule| format!("\"{rule}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let rule = format!("add {rules} to permissions.allow in your user settings ({USER_SETTINGS})");
    match mode {
        Some(mode) => format!(
            "{rule}, or, only in a disposable environment, re-run with --permission-mode {}",
            mode.as_str()
        ),
        None => rule,
    }
}

/// The tool result the model gets for a denied call: the reason, and that the step
/// did not happen. A model summarised a run whose test runs were all denied with "All
/// tests pass".
pub(crate) fn denial_for_model(reason: &str) -> String {
    format!(
        "{reason}. Nothing ran, so there is no output: do not describe this step as done; tell the user it was not run."
    )
}

/// The calls a run denied, and whether that stopped it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DenialLog {
    /// The denied calls, in order.
    denied: Vec<Denial>,
    /// The run reached [`DENIAL_LIMIT`].
    stopped: bool,
}

impl DenialLog {
    const fn new() -> Self {
        Self {
            denied: Vec::new(),
            stopped: false,
        }
    }

    /// Adds a denied call; `true` when it stops a `one_shot` run, which happens once.
    fn record(&mut self, denial: Denial, one_shot: bool) -> bool {
        self.denied.push(denial);
        if one_shot && !self.stopped && self.denied.len() >= DENIAL_LIMIT {
            self.stopped = true;
            return true;
        }
        false
    }
}

/// Records a tool call denied because nobody could approve it, and tells the user now
/// why ([`Denial::reason`]; the run's last warning names it again; in JSON mode only
/// that warning, as JSON). Returns `true` when this denial stops the run
/// ([`DENIAL_LIMIT`]): the caller cancels the turn.
pub(crate) fn record_denial(denial: Denial, reason: &str) -> bool {
    let stop = DENIED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .record(denial, one_shot());
    if !JSON.load(Ordering::SeqCst) {
        runtime::ui_hooks::notice(reason);
    }
    stop
}

/// Whether the run stopped at [`DENIAL_LIMIT`] denied calls.
pub(crate) fn stopped() -> bool {
    DENIED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .stopped
}

/// What a run that stopped at [`DENIAL_LIMIT`] says, before [`finish`] ends its
/// bookkeeping; `None` when it did not stop.
pub(crate) fn stop_message() -> Option<String> {
    let log = DENIED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    Shortfall {
        denied: log.denied,
        unanswered: 0,
        stopped: log.stopped,
    }
    .stop_message()
}

/// What a non-interactive run could not do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Shortfall {
    /// The denied calls, in order (a tool denied twice is listed twice).
    pub denied: Vec<Denial>,
    /// Questions from the model that nobody answered.
    pub unanswered: usize,
    /// The run stopped at [`DENIAL_LIMIT`] denied calls.
    pub stopped: bool,
}

impl Shortfall {
    /// The exit code the run ends with, when not 0.
    pub(crate) fn exit_code(&self) -> Option<i32> {
        (!self.denied.is_empty()).then_some(DENIED_EXIT_CODE)
    }

    /// The tools of the denied calls, in order.
    fn denied_tools(&self) -> Vec<String> {
        self.denied
            .iter()
            .map(|denial| denial.tool.clone())
            .collect()
    }

    /// What would let the denied calls run: one example rule per denied tool (from its
    /// first denied call), then the highest mode any of them needed.
    fn allow_hint(&self) -> String {
        let mut tools: Vec<&str> = Vec::new();
        let mut rules = Vec::new();
        for denial in &self.denied {
            if !tools.contains(&denial.tool.as_str()) {
                tools.push(&denial.tool);
                rules.extend(denial.rules.first().cloned());
            }
        }
        let mode = self.denied.iter().filter_map(|denial| denial.mode).max();
        allow_hint(&rules, mode)
    }

    /// Why the run stopped early, or `None` when it did not.
    pub(crate) fn stop_message(&self) -> Option<String> {
        self.stopped.then(|| {
            format!(
                "stopped: {} tool calls were denied in a non-interactive run ({}); to allow them in scripts, {}",
                self.denied.len(),
                tool_counts(&self.denied_tools()),
                self.allow_hint(),
            )
        })
    }

    /// The end-of-run warning, or `None` when nothing was left undone. A run that
    /// stopped says so instead ([`Self::stop_message`]).
    pub(crate) fn warning(&self) -> Option<String> {
        let mut parts = Vec::new();
        if !self.denied.is_empty() && !self.stopped {
            let calls = self.denied.len();
            let them = if calls == 1 { "it" } else { "them" };
            parts.push(format!(
                "{calls} tool call{} denied because nobody could approve {them} ({}). To allow {them} in scripts, {}.",
                if calls == 1 { " was" } else { "s were" },
                tool_counts(&self.denied_tools()),
                self.allow_hint(),
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

    /// The end-of-run line as `--output-format json` prints it on stderr: an error when
    /// the run stopped, else the warning.
    pub(crate) fn warning_json(&self) -> Option<serde_json::Value> {
        if let Some(error) = self.stop_message() {
            return Some(json!({
                "type": "error",
                "error": error,
                "denied_tools": self.denied_tools(),
                "unanswered_questions": self.unanswered,
            }));
        }
        let message = self.warning()?;
        Some(json!({
            "type": "warning",
            "warning": message,
            "denied_tools": self.denied_tools(),
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
    let log = std::mem::take(&mut *DENIED.lock().unwrap_or_else(PoisonError::into_inner));
    Shortfall {
        denied: log.denied,
        unanswered: UNANSWERED.swap(0, Ordering::SeqCst),
        stopped: log.stopped,
    }
}

/// Prints the end-of-run warning, or why the run stopped, on stderr (JSON in JSON
/// mode) and returns the exit code the run ends with, when not 0.
pub(crate) fn report(output_format: CliOutputFormat) -> Option<i32> {
    let shortfall = finish();
    match output_format {
        CliOutputFormat::Json => {
            if let Some(warning) = shortfall.warning_json() {
                eprintln!("{warning}");
            }
        }
        CliOutputFormat::Text => {
            for line in [shortfall.stop_message(), shortfall.warning()]
                .into_iter()
                .flatten()
            {
                eprintln!("{line}");
            }
        }
    }
    shortfall.exit_code()
}

/// What piped stdin brought.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PipedStdin {
    /// It ended with nothing but whitespace, or could not be read.
    Empty,
    /// Neither data nor its end arrived within the wait.
    Silent,
    /// What it sent; `truncated` when it went on past the limit, which was kept.
    Text { text: String, truncated: bool },
}

/// Reads `reader` on a thread of its own, so a pipe that never closes cannot hang the
/// run. Gives up when neither data nor the end arrives within `wait` (`None` waits as
/// long as it takes); once data comes, reads to the end, or to `max_bytes`.
pub(crate) fn read_piped(
    reader: impl Read + Send + 'static,
    wait: Option<Duration>,
    max_bytes: usize,
) -> PipedStdin {
    // `None` is the end (or a read error).
    let (sender, receiver) = mpsc::sync_channel::<Option<Vec<u8>>>(16);
    let spawned = thread::Builder::new()
        .name("neuron-stdin".to_string())
        .spawn(move || {
            let mut reader = reader;
            let mut buffer = vec![0_u8; 8192];
            let mut total = 0_usize;
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        total += read;
                        if sender.send(Some(buffer[..read].to_vec())).is_err() || total > max_bytes
                        {
                            return;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            let _ = sender.send(None);
        });
    if spawned.is_err() {
        return PipedStdin::Empty;
    }

    let mut next = match wait {
        Some(wait) => match receiver.recv_timeout(wait) {
            Ok(chunk) => chunk,
            Err(RecvTimeoutError::Timeout) => return PipedStdin::Silent,
            Err(RecvTimeoutError::Disconnected) => None,
        },
        None => receiver.recv().ok().flatten(),
    };
    let mut bytes = Vec::new();
    while let Some(chunk) = next {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > max_bytes {
            break;
        }
        next = receiver.recv().ok().flatten();
    }
    let truncated = bytes.len() > max_bytes;
    if truncated {
        bytes.truncate(max_bytes);
        // Not inside a character.
        if let Err(error) = std::str::from_utf8(&bytes) {
            if error.error_len().is_none() {
                bytes.truncate(error.valid_up_to());
            }
        }
    }
    let text = String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned());
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if text.trim().is_empty() {
        PipedStdin::Empty
    } else {
        PipedStdin::Text {
            text: text.to_string(),
            truncated,
        }
    }
}

/// Piped stdin for a `neuron prompt` run, to send as context with the prompt; `None`
/// when stdin is a terminal or brought nothing. With a prompt on the command line a
/// silent pipe is waited for [`STDIN_WAIT`] only; without one, stdin is the prompt.
/// Says on stderr when it gave up waiting or cut the input.
pub(crate) fn stdin_context(has_prompt: bool, output_format: CliOutputFormat) -> Option<String> {
    // Asked before the read starts (and remembered): on Windows, asking while a read
    // is pending on the pipe would wait for that read.
    if runtime::ui_hooks::stdin_is_terminal() {
        return None;
    }
    let wait = has_prompt.then_some(STDIN_WAIT);
    match read_piped(io::stdin(), wait, STDIN_MAX_BYTES) {
        PipedStdin::Empty => None,
        PipedStdin::Silent => {
            note(
                output_format,
                &format!(
                    "note: stdin is not a terminal, but nothing arrived on it within {} ms; running without it",
                    STDIN_WAIT.as_millis()
                ),
            );
            None
        }
        PipedStdin::Text { text, truncated } => {
            if truncated {
                note(
                    output_format,
                    &format!(
                        "note: piped stdin is longer than {} KiB; only its first {} KiB is sent",
                        STDIN_MAX_BYTES / 1024,
                        STDIN_MAX_BYTES / 1024
                    ),
                );
            }
            Some(text)
        }
    }
}

/// A one-line note on stderr (a JSON line in JSON mode).
fn note(output_format: CliOutputFormat, message: &str) {
    match output_format {
        CliOutputFormat::Json => eprintln!("{}", json!({ "type": "notice", "notice": message })),
        CliOutputFormat::Text => eprintln!("{message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        denial_for_model, read_piped, Denial, DenialLog, PipedStdin, Shortfall, DENIAL_LIMIT,
        DENIED_EXIT_CODE, STDIN_WAIT,
    };
    use runtime::{PermissionMode, PermissionRequest};
    use std::io::{self, Cursor, Read};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// A call that needs `required_mode`, denied in workspace-write mode.
    fn denied(
        tool_name: &str,
        input: &str,
        session_prefixes: &[&str],
        required_mode: PermissionMode,
    ) -> Denial {
        let request = PermissionRequest {
            tool_name: tool_name.to_string(),
            input: input.to_string(),
            current_mode: PermissionMode::WorkspaceWrite,
            required_mode,
            reason: None,
            session_prefixes: session_prefixes.iter().map(ToString::to_string).collect(),
        };
        Denial::of(&request, PermissionMode::WorkspaceWrite)
    }

    fn denied_command(tool_name: &str, command: &str, session_prefixes: &[&str]) -> Denial {
        denied(
            tool_name,
            &serde_json::json!({ "command": command }).to_string(),
            session_prefixes,
            PermissionMode::DangerFullAccess,
        )
    }

    #[test]
    fn a_denial_says_what_would_let_the_tool_run() {
        let reason = denied_command("bash", "npm test", &["npm test"]).reason();
        assert_eq!(
            reason,
            "tool 'bash' was not run: nobody could approve it in a non-interactive run. To allow it in scripts, add \"bash(npm:*)\" to permissions.allow in your user settings (~/.neuron/settings.json), or, only in a disposable environment, re-run with --permission-mode danger-full-access"
        );
        // An ask rule or a hook asked for approval: the mode is not the reason.
        let reason = denied(
            "write_file",
            r#"{"path":"notes.md"}"#,
            &[],
            PermissionMode::ReadOnly,
        )
        .reason();
        assert!(!reason.contains("--permission-mode"), "{reason}");
        assert!(
            reason.contains("add \"write_file\" to permissions.allow"),
            "{reason}"
        );
    }

    /// A web search on a one-line question was answered with "re-run with
    /// --permission-mode danger-full-access" first, and the model repeated it.
    #[test]
    fn a_denied_web_search_names_the_allow_rule_before_full_access() {
        let reason = denied(
            "WebSearch",
            r#"{"query":"latest node lts"}"#,
            &["WebSearch"],
            PermissionMode::DangerFullAccess,
        )
        .reason();
        let rule = reason.find("\"WebSearch\"").expect(&reason);
        let full_access = reason.find("danger-full-access").expect(&reason);
        assert!(rule < full_access, "{reason}");
        assert!(
            reason.contains("only in a disposable environment"),
            "{reason}"
        );
    }

    /// A shell rule names the program (`bash(npm:*)` covers `npm test` and `npm run
    /// build`), one per program the command needs approval for.
    #[test]
    fn a_denied_command_names_a_rule_for_its_program() {
        let npm = denied_command("bash", "npm test", &["npm test"]);
        assert!(
            npm.reason()
                .contains("add \"bash(npm:*)\" to permissions.allow"),
            "{}",
            npm.reason()
        );
        let powershell = denied_command("PowerShell", "npm run build", &["npm run"]);
        assert!(
            powershell.reason().contains("\"PowerShell(npm:*)\""),
            "{}",
            powershell.reason()
        );
        let both = denied_command(
            "bash",
            "cargo test && cargo fmt --check && npm run build",
            &["cargo test", "cargo fmt", "npm run"],
        );
        assert!(
            both.reason()
                .contains("add \"bash(cargo:*)\", \"bash(npm:*)\" to permissions.allow"),
            "{}",
            both.reason()
        );
        // No prefix to approve: the command's first program.
        let opaque = denied_command("bash", "make release", &[]);
        assert!(
            opaque.reason().contains("\"bash(make:*)\""),
            "{}",
            opaque.reason()
        );
    }

    #[test]
    fn the_model_is_told_the_denied_step_did_not_happen() {
        let reason = denied_command("bash", "npm test", &["npm test"]).reason();
        let for_model = denial_for_model(&reason);
        assert!(for_model.starts_with(&reason), "{for_model}");
        assert!(
            for_model.contains("tell the user it was not run"),
            "{for_model}"
        );
    }

    #[test]
    fn the_end_of_run_warning_names_the_denied_tools() {
        let none = Shortfall::default();
        assert_eq!(none.warning(), None);
        assert_eq!(none.exit_code(), None);

        let shortfall = Shortfall {
            denied: vec![
                denied_command("bash", "npm test", &["npm test"]),
                denied(
                    "write_file",
                    r#"{"path":".git/config"}"#,
                    &[],
                    PermissionMode::DangerFullAccess,
                ),
                denied_command("bash", "cargo build", &["cargo build"]),
            ],
            unanswered: 3,
            stopped: false,
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
        // One example rule per denied tool, before the mode, which is a last resort.
        assert!(
            warning.contains("To allow them in scripts, add \"bash(npm:*)\", \"write_file\" to permissions.allow in your user settings (~/.neuron/settings.json), or, only in a disposable environment, re-run with --permission-mode danger-full-access."),
            "{warning}"
        );
        assert!(
            warning.contains("3 questions from the model went unanswered"),
            "{warning}"
        );
        assert_eq!(shortfall.exit_code(), Some(DENIED_EXIT_CODE));
        assert_eq!(shortfall.stop_message(), None);

        let json = shortfall.warning_json().expect("json");
        assert_eq!(json["type"], "warning");
        assert_eq!(json["denied_tools"][1], "write_file");
        assert_eq!(json["unanswered_questions"], 3);

        // Questions alone are reported, and the run still succeeds.
        let questions = Shortfall {
            denied: Vec::new(),
            unanswered: 1,
            stopped: false,
        };
        assert!(questions
            .warning()
            .is_some_and(|warning| warning.contains("1 question from the model went unanswered")));
        assert_eq!(questions.exit_code(), None);
    }

    /// The model varied the tool and its arguments, so the repeated-call guard never
    /// fired: the run stops at the limit, once, and only in a one-shot run.
    #[test]
    fn a_one_shot_run_stops_at_the_denial_limit() {
        let mut log = DenialLog::new();
        let tools = ["bash", "PowerShell", "bash", "PowerShell", "bash"];
        let stops = tools
            .iter()
            .enumerate()
            .map(|(index, tool)| {
                log.record(
                    denied_command(tool, &format!("make step{index}"), &[]),
                    true,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(DENIAL_LIMIT, 5);
        assert_eq!(stops, vec![false, false, false, false, true]);
        assert!(
            !log.record(denied_command("bash", "make", &[]), true),
            "it stops once"
        );
        assert!(log.stopped);

        // The REPL is not stopped: the user is there.
        let mut repl = DenialLog::new();
        assert!(
            (0..DENIAL_LIMIT * 2).all(|_| !repl.record(denied_command("bash", "make", &[]), false))
        );
        assert!(!repl.stopped);
    }

    #[test]
    fn a_stopped_run_says_why_and_fails_with_the_denial_exit_code() {
        let shortfall = Shortfall {
            denied: ["bash", "PowerShell", "bash", "PowerShell", "bash"]
                .map(|tool| denied_command(tool, "npm install", &["npm install"]))
                .to_vec(),
            unanswered: 0,
            stopped: true,
        };
        let message = shortfall.stop_message().expect("stopped");
        assert_eq!(
            message,
            "stopped: 5 tool calls were denied in a non-interactive run (bash \u{00d7}3, PowerShell \u{00d7}2); to allow them in scripts, add \"bash(npm:*)\", \"PowerShell(npm:*)\" to permissions.allow in your user settings (~/.neuron/settings.json), or, only in a disposable environment, re-run with --permission-mode danger-full-access"
        );
        // The stop replaces the end-of-run warning.
        assert_eq!(shortfall.warning(), None);
        assert_eq!(shortfall.exit_code(), Some(DENIED_EXIT_CODE));

        // JSON mode: one JSON error line that parses back.
        let json = shortfall.warning_json().expect("json");
        let parsed: serde_json::Value =
            serde_json::from_str(&json.to_string()).expect("valid JSON");
        assert_eq!(parsed["type"], "error");
        assert_eq!(parsed["error"], message);
        assert_eq!(parsed["denied_tools"].as_array().map(Vec::len), Some(5));
    }

    /// A reader that sends nothing and never ends, until the test lets it go.
    struct NeverSends(mpsc::Receiver<()>);

    impl Read for NeverSends {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            let _ = self.0.recv();
            Ok(0)
        }
    }

    #[test]
    fn a_silent_pipe_is_not_waited_for() {
        let (release, never) = mpsc::channel();
        let started = Instant::now();
        let read = read_piped(NeverSends(never), Some(STDIN_WAIT), 1024);
        let waited = started.elapsed();
        drop(release);
        assert_eq!(read, PipedStdin::Silent);
        assert!(waited >= STDIN_WAIT, "{waited:?}");
        assert!(waited < Duration::from_secs(5), "{waited:?}");
    }

    /// A reader that sends its first part at once and the rest after `pause`.
    struct SlowRest {
        parts: Vec<&'static [u8]>,
        pause: Duration,
        sent: usize,
    }

    impl Read for SlowRest {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let Some(part) = self.parts.get(self.sent) else {
                return Ok(0);
            };
            if self.sent > 0 {
                std::thread::sleep(self.pause);
            }
            self.sent += 1;
            buffer[..part.len()].copy_from_slice(part);
            Ok(part.len())
        }
    }

    /// The wait is for the first data only: a producer that pauses later is read to the
    /// end.
    #[test]
    fn once_data_arrives_the_pipe_is_read_to_its_end() {
        let read = read_piped(
            SlowRest {
                parts: vec![b"error: line 1\n", b"error: line 2\n"],
                pause: STDIN_WAIT + Duration::from_millis(200),
                sent: 0,
            },
            Some(STDIN_WAIT),
            1024,
        );
        assert_eq!(
            read,
            PipedStdin::Text {
                text: "error: line 1\nerror: line 2\n".to_string(),
                truncated: false,
            }
        );
    }

    #[test]
    fn piped_input_is_capped_and_empty_input_is_nothing() {
        let read = read_piped(Cursor::new("é".repeat(10)), Some(STDIN_WAIT), 5);
        // Cut at the limit, not inside a character.
        assert_eq!(
            read,
            PipedStdin::Text {
                text: "éé".to_string(),
                truncated: true,
            }
        );
        // An endless producer (`yes | neuron prompt ...`) is cut off too.
        let read = read_piped(io::repeat(b'y'), Some(STDIN_WAIT), 64);
        assert!(
            matches!(read, PipedStdin::Text { ref text, truncated: true } if text.len() == 64),
            "{read:?}"
        );
        assert_eq!(
            read_piped(Cursor::new(" \n\t\n"), Some(STDIN_WAIT), 64),
            PipedStdin::Empty
        );
        assert_eq!(read_piped(Cursor::new(""), None, 64), PipedStdin::Empty);
        assert_eq!(
            read_piped(Cursor::new("\u{feff}log"), None, 64),
            PipedStdin::Text {
                text: "log".to_string(),
                truncated: false,
            }
        );
    }
}
