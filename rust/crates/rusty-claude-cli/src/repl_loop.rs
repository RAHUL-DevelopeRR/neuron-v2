//! Interactive REPL: launch, line dispatch, error survival, Ctrl-C handling
//! and context diagnostics.
//!
//! Every failure inside one REPL step (a model turn, a slash command, even a
//! panic) is rendered and the loop keeps running; the session is persisted
//! after each step so memory and disk agree.

use super::*;
use runtime::context_manager::{ContextBudget, ContextManager, ContextPolicy, ContextSnapshot};
use runtime::{CancellationToken, HookAbortSignal, SessionCompaction, TurnOutcome, TurnSummary};
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};

/// What the REPL does after handling one line of input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplStep {
    Continue,
    Exit,
}

/// The pieces of the live CLI the line dispatcher needs. Implemented by
/// [`LiveCli`]; unit tests use a fake.
pub(crate) trait ReplBackend {
    /// Handles shortcuts and slash commands. Returns `None` when the line is
    /// a prompt for the model.
    fn handle_local(&mut self, line: &str) -> Option<Result<ReplStep, Box<dyn std::error::Error>>>;
    /// Runs one model turn for `line`.
    fn run_prompt(&mut self, line: &str) -> Result<(), Box<dyn std::error::Error>>;
    /// Writes the in-memory session to disk.
    fn persist(&mut self) -> Result<(), Box<dyn std::error::Error>>;
}

/// Dispatches one submitted line. Errors and panics are reported to `errors`
/// and never end the REPL; only an explicit exit does.
pub(crate) fn dispatch_line(
    backend: &mut impl ReplBackend,
    line: &str,
    errors: &mut impl Write,
) -> ReplStep {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return ReplStep::Continue;
    }
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| match backend.handle_local(trimmed) {
        Some(result) => result,
        None => backend.run_prompt(trimmed).map(|()| ReplStep::Continue),
    }));
    let message = match outcome {
        Ok(Ok(step)) => return step,
        Ok(Err(error)) => error.to_string(),
        Err(payload) => format!("internal error: {}", panic_message(payload.as_ref())),
    };
    report_repl_error(&message, errors);
    if let Err(error) = backend.persist() {
        let _ = writeln!(errors, "warning: failed to save the session: {error}");
    }
    ReplStep::Continue
}

/// Renders a failed REPL step without ending the session.
pub(crate) fn report_repl_error(message: &str, out: &mut impl Write) {
    let _ = writeln!(out, "\n\x1b[31m\u{2718}\x1b[0m {message}");
    let _ = writeln!(
        out,
        "  \x1b[2mThe session is saved. Retry, /compact, switch /model, or keep chatting.\x1b[0m"
    );
}

pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(ToString::to_string)
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Result of a Ctrl-C press while a turn runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InterruptAction {
    /// The turn was asked to stop; keep waiting for it.
    Cancelled,
    /// A second press: save the session and exit once the turn unwinds.
    ForceQuit,
}

/// Whether interrupt notices tell the user that another Ctrl-C quits. The line REPL
/// does; the full-screen UI shows its own key hints, so the engine leaves it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuitHint {
    Shown,
    Hidden,
}

/// Set by a second Ctrl-C during a turn: the REPL saves the session, stops
/// MCP servers and plugins, and exits as soon as the turn has unwound.
static FORCE_QUIT: AtomicBool = AtomicBool::new(false);
/// How long a force quit waits for the turn to unwind (a tool that ignores
/// cancellation) before exiting without cleanup.
const FORCE_QUIT_GRACE: Duration = Duration::from_secs(5);

/// Whether a second Ctrl-C asked the REPL to exit.
pub(crate) fn force_quit_requested() -> bool {
    FORCE_QUIT.load(Ordering::SeqCst)
}

/// Asks the REPL to exit after the current turn unwinds. A watchdog exits
/// anyway when that takes longer than [`FORCE_QUIT_GRACE`].
pub(crate) fn request_force_quit() {
    if FORCE_QUIT.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::spawn(|| {
        thread::sleep(FORCE_QUIT_GRACE);
        // `exit` runs no destructors: give the terminal back if the full-screen UI holds it.
        neuron_tui::terminal::restore();
        eprintln!("\nThe turn did not stop in time; exiting without cleanup.");
        std::process::exit(130);
    });
}

/// First Ctrl-C during a turn cancels it (model stream, tools, hooks and
/// sub-agents, including ones left running in the background by an earlier
/// turn); a second one quits after saving.
pub(crate) fn handle_turn_interrupt(
    presses: u32,
    cancel: &CancellationToken,
    hook_abort: &HookAbortSignal,
    hint: QuitHint,
) -> InterruptAction {
    if presses <= 1 {
        cancel.cancel();
        hook_abort.abort();
        tools::cancel_background_subagents();
        runtime::ui_hooks::notice(match hint {
            QuitHint::Shown => {
                "\n\x1b[33m\u{25a0}\x1b[0m Interrupting\u{2026} \x1b[2m(press Ctrl-C again to quit)\x1b[0m"
            }
            QuitHint::Hidden => "\n\x1b[33m\u{25a0}\x1b[0m Interrupting\u{2026}",
        });
        InterruptAction::Cancelled
    } else {
        cancel.cancel();
        runtime::ui_hooks::notice("\nQuitting: saving the session first\u{2026}");
        InterruptAction::ForceQuit
    }
}

/// Ctrl-C presses from a UI that reads the keyboard itself: in raw mode Ctrl-C arrives
/// as a key, not as a signal. The UI calls [`InterruptPresses::press`]; the turn's
/// interrupt watcher feeds each press through [`handle_turn_interrupt`].
#[derive(Debug, Clone, Default)]
pub(crate) struct InterruptPresses(Arc<tokio::sync::Notify>);

impl InterruptPresses {
    pub(crate) fn press(&self) {
        self.0.notify_one();
    }

    /// Resolves at the next press. A press made while nobody was waiting is kept (once).
    pub(crate) fn next_press(
        &self,
    ) -> impl std::future::Future<Output = io::Result<()>> + Send + 'static {
        let presses = Arc::clone(&self.0);
        async move {
            presses.notified().await;
            Ok(())
        }
    }
}

/// Feeds every Ctrl-C press from `next_press` through
/// [`handle_turn_interrupt`] until `stop` resolves (the turn ended) or the
/// signal source fails. `on_force_quit` runs on a second press.
pub(crate) async fn watch_turn_interrupts<P, F>(
    mut next_press: P,
    stop: impl std::future::Future<Output = ()>,
    cancel: &CancellationToken,
    hook_abort: &HookAbortSignal,
    hint: QuitHint,
    mut on_force_quit: impl FnMut(),
) where
    P: FnMut() -> F,
    F: std::future::Future<Output = io::Result<()>>,
{
    tokio::pin!(stop);
    let mut presses = 0_u32;
    loop {
        tokio::select! {
            result = next_press() => {
                if result.is_err() {
                    break;
                }
                presses += 1;
                if handle_turn_interrupt(presses, cancel, hook_abort, hint)
                    == InterruptAction::ForceQuit
                {
                    on_force_quit();
                }
            }
            () = &mut stop => break,
        }
    }
}

/// Shuts the shared MCP servers and plugins down however `launch` returns,
/// including early errors.
struct SharedStateShutdown;

impl Drop for SharedStateShutdown {
    fn drop(&mut self) {
        crate::progress::shutdown_shared_runtime_state();
    }
}

/// Stable machine-readable name of a turn outcome (JSON output).
pub(crate) fn turn_outcome_label(outcome: &TurnOutcome) -> &'static str {
    match outcome {
        TurnOutcome::Completed => "completed",
        TurnOutcome::Cancelled { .. } => "cancelled",
        TurnOutcome::LoopDetected { .. } => "loop_detected",
        TurnOutcome::OutputTruncated => "output_truncated",
    }
}

/// One-line notice describing how a turn ended, if it did not simply finish.
pub(crate) fn describe_turn_outcome(summary: &TurnSummary, hint: QuitHint) -> Option<String> {
    let mut notes = Vec::new();
    match &summary.outcome {
        TurnOutcome::Completed => {}
        TurnOutcome::Cancelled {
            interrupted_tool_calls,
        } => notes.push(match (interrupted_tool_calls, hint) {
            (0, QuitHint::Shown) => {
                "Interrupted. Back at the prompt; press Ctrl-C again to exit.".to_string()
            }
            (0, QuitHint::Hidden) => "Interrupted.".to_string(),
            (calls, QuitHint::Shown) => format!(
                "Interrupted. {calls} pending tool call(s) were not run. Press Ctrl-C again to exit."
            ),
            (calls, QuitHint::Hidden) => {
                format!("Interrupted. {calls} pending tool call(s) were not run.")
            }
        }),
        TurnOutcome::LoopDetected { tool_name, repeats } => notes.push(format!(
            "Stopped: the model called `{tool_name}` {repeats} times in a row with identical arguments. Rephrase the request or give more guidance."
        )),
        TurnOutcome::OutputTruncated => notes.push(
            "Note: the response was cut off at the output token limit even after one automatic continuation. Ask the model to continue or to work in smaller steps."
                .to_string(),
        ),
    }
    if summary.pruned_tool_results > 0 {
        notes.push(format!(
            "Context: shortened {} older tool result(s) to stay within the model's context window.",
            summary.pruned_tool_results
        ));
    }
    (!notes.is_empty()).then(|| notes.join("\n"))
}

/// How `neuron --resume <session>` continues interactively: the launch
/// flags given with it, the session's stored model when no `--model` was
/// given, and the configured permission mode when none was given.
pub(crate) fn resume_launch(
    launch: ResumeLaunchOptions,
    stored_model: Option<&str>,
    resume: String,
) -> ReplLaunch {
    let model = launch
        .model
        .or_else(|| stored_model.map(ToOwned::to_owned))
        .filter(|model| !model.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    ReplLaunch {
        model,
        allowed_tools: launch.allowed_tools,
        permission_mode: launch
            .permission_mode
            .unwrap_or_else(default_permission_mode),
        base_commit: launch.base_commit,
        reasoning_effort: launch.reasoning_effort,
        allow_broad_cwd: launch.allow_broad_cwd,
        resume: Some(resume),
    }
}

/// Options for starting an interactive session.
pub(crate) struct ReplLaunch {
    pub model: String,
    pub allowed_tools: Option<AllowedToolSet>,
    pub permission_mode: PermissionMode,
    pub base_commit: Option<String>,
    pub reasoning_effort: Option<String>,
    pub allow_broad_cwd: bool,
    /// Session reference to continue instead of starting a fresh session.
    pub resume: Option<String>,
}

/// Starts the interactive REPL (optionally continuing a saved session) and
/// runs it until the user exits.
pub(crate) fn launch(options: ReplLaunch) -> Result<(), Box<dyn std::error::Error>> {
    enforce_broad_cwd_policy(options.allow_broad_cwd, CliOutputFormat::Text)?;
    run_stale_base_preflight(options.base_commit.as_deref());

    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if !crate::auth::check_trust(&cwd) {
        std::process::exit(1);
    }

    // From here on MCP servers may be running: stop them on every exit path.
    let shutdown = SharedStateShutdown;
    let resolved_model = configure_provider_for_model(resolve_repl_model(
        crate::args::model_after_trust(options.model),
    ))?;
    let mut cli = LiveCli::new(
        resolved_model,
        true,
        options.allowed_tools,
        crate::args::permission_mode_after_trust(options.permission_mode),
    )?;
    cli.set_reasoning_effort(options.reasoning_effort);
    let mut editor =
        input::LineEditor::new("> ", cli.repl_completion_candidates().unwrap_or_default());
    println!();
    println!("{}", cli.startup_banner());
    println!("{}", format_connected_line(&cli.model));
    print_footer(&cli);

    if let Some(reference) = options.resume {
        continue_saved_session(&mut cli, &reference)?;
    }

    let result = run_loop(&mut cli, &mut editor);
    // Sub-agents left running in the background must not outlive the session.
    tools::cancel_background_subagents();
    drop(shutdown);
    if force_quit_requested() {
        if let Err(error) = &result {
            eprintln!("error: {error}");
        }
        std::process::exit(130);
    }
    result
}

fn print_footer(cli: &LiveCli) {
    // Shows model, session, and the ? hint so the user knows how to discover
    // shortcuts without reading the manual.
    let dm = "\x1b[2m";
    let r = "\x1b[0m";
    let bc = "\x1b[38;2;100;100;100m";
    let model_s = cli.model.split('/').next_back().unwrap_or(&cli.model);
    let session_short: String = cli.session.id.chars().take(6).collect();
    println!(
        "\n{bc}{}{r}\n  {dm}? for shortcuts{r}                       {dm}{model_s} \u{00b7} session {session_short}{r}\n",
        "\u{2500}".repeat(74),
    );
}

/// Switches the freshly started CLI to a saved session, removing the empty
/// placeholder session file the fresh start created.
fn continue_saved_session(
    cli: &mut LiveCli,
    reference: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let placeholder = cli.session.path.clone();
    let placeholder_is_empty = cli.runtime.session().messages.is_empty();
    cli.resume_session(Some(reference.to_string()))?;
    if placeholder_is_empty && placeholder != cli.session.path {
        let _ = fs::remove_file(&placeholder);
    }
    Ok(())
}

fn run_loop(
    cli: &mut LiveCli,
    editor: &mut input::LineEditor,
) -> Result<(), Box<dyn std::error::Error>> {
    // Input is dispatched through a priority chain (see `handle_local`):
    //   "?" shortcuts, "!cmd" shell escape, /exit, /context, /status,
    //   slash commands, bare skills, then the model.
    loop {
        editor.set_prompt(&mode_aware_prompt(
            &cli.permission_mode,
            cli.plan_mode,
            cli.orchestration_mode.as_deref(),
        ));
        editor.set_completions(cli.repl_completion_candidates().unwrap_or_default());

        match editor.read_line()? {
            input::ReadOutcome::Submit(input) => {
                if !matches!(input.trim(), "" | "?" | "??") {
                    editor.push_history(input.clone());
                }
                if dispatch_line(cli, &input, &mut io::stderr()) == ReplStep::Exit
                    || force_quit_requested()
                {
                    break;
                }
            }
            input::ReadOutcome::Cancel => {}
            input::ReadOutcome::Exit => break,
        }
    }
    cli.persist_session()
}

impl ReplBackend for LiveCli {
    fn handle_local(&mut self, line: &str) -> Option<Result<ReplStep, Box<dyn std::error::Error>>> {
        if line == "?" || line == "??" {
            println!("{}", render_shortcuts_panel());
            return Some(Ok(ReplStep::Continue));
        }
        if let Some(shell_cmd) = line.strip_prefix('!') {
            let shell_cmd = shell_cmd.trim();
            if !shell_cmd.is_empty() {
                run_shell_escape(shell_cmd);
            }
            return Some(Ok(ReplStep::Continue));
        }
        if matches!(line, "/exit" | "/quit") {
            return Some(Ok(ReplStep::Exit));
        }
        if let Some(argument) = context_command_argument(line) {
            return Some(self.print_context(argument).map(|()| ReplStep::Continue));
        }
        if line == "/status" {
            self.print_status();
            println!("\n{}", self.context_report());
            return Some(Ok(ReplStep::Continue));
        }
        match SlashCommand::parse(line) {
            Ok(Some(command)) => Some(self.handle_repl_command(command).and_then(
                |should_persist| {
                    if should_persist {
                        self.persist_session()?;
                    }
                    Ok(ReplStep::Continue)
                },
            )),
            Ok(None) => None,
            Err(error) => {
                eprintln!("{error}");
                Some(Ok(ReplStep::Continue))
            }
        }
    }

    fn run_prompt(&mut self, line: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.record_prompt_history(line);
        let cwd = env::current_dir().unwrap_or_default();
        match try_resolve_bare_skill_prompt(&cwd, line) {
            Some(prompt) => self.run_turn(&prompt),
            None => self.run_turn(line),
        }
    }

    fn persist(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.persist_session()
    }
}

/// Returns the argument of a `/context [show|clear]` line.
fn context_command_argument(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("/context")?;
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim())
}

impl LiveCli {
    fn print_context(&self, argument: &str) -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", self.context_command(argument)?);
        Ok(())
    }

    /// What `/context [show]` shows.
    pub(crate) fn context_command(
        &self,
        argument: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        match argument {
            "" | "show" => Ok(self.context_report()),
            "clear" => Err(
                "/context clear is not supported; use /compact to summarize or /clear --confirm to start fresh"
                    .into(),
            ),
            other => Err(format!("unknown /context argument `{other}`. Usage: /context [show]").into()),
        }
    }

    /// Context diagnostics for `/context` and `/status`.
    pub(crate) fn context_report(&self) -> String {
        let session = self.runtime.session();
        render_context_report(&ContextReport {
            model: &self.model,
            snapshot: self.runtime.context_snapshot(),
            compaction: session.compaction.as_ref(),
            cumulative: self.runtime.usage().cumulative_usage(),
            message_count: session.messages.len(),
        })
    }
}

/// Inputs of the context diagnostics view.
pub(crate) struct ContextReport<'a> {
    pub model: &'a str,
    pub snapshot: ContextSnapshot,
    pub compaction: Option<&'a SessionCompaction>,
    pub cumulative: TokenUsage,
    pub message_count: usize,
}

/// Context diagnostics for a saved session (no live tools or system prompt).
pub(crate) fn saved_session_context_report(session: &Session) -> ContextReport<'_> {
    let model = session.model.as_deref().unwrap_or(DEFAULT_MODEL);
    let manager = ContextManager::new(
        ContextBudget::for_model(model, max_tokens_for_model(model), 0),
        ContextPolicy::from_env(),
    );
    ContextReport {
        model,
        snapshot: manager.snapshot(&session.messages, &[]),
        compaction: session.compaction.as_ref(),
        cumulative: UsageTracker::from_session(session).cumulative_usage(),
        message_count: session.messages.len(),
    }
}

pub(crate) fn render_context_report(report: &ContextReport<'_>) -> String {
    let snapshot = &report.snapshot;
    let budget = snapshot.budget;
    let last_compaction = report.compaction.map_or_else(
        || "never".to_string(),
        |compaction| {
            format!(
                "#{} \u{00b7} {} messages summarized",
                compaction.count, compaction.removed_message_count
            )
        },
    );
    let usage = report.cumulative;
    [
        "Context".to_string(),
        format!(
            "  Model            {}",
            crate::provider::shown_model(report.model)
        ),
        format!(
            "  Context window   {} tokens",
            group_digits(budget.context_window)
        ),
        format!(
            "  Output reserve   {} tokens",
            group_digits(budget.output_reserve)
        ),
        format!(
            "  Tool schemas     ~{} tokens",
            group_digits(budget.tool_schema_tokens)
        ),
        format!(
            "  System prompt    ~{} tokens",
            group_digits(snapshot.system_prompt_tokens)
        ),
        format!(
            "  Input budget     {} tokens",
            group_digits(snapshot.input_budget)
        ),
        format!(
            "  Estimated used   ~{} tokens ({}% of budget, {} messages)",
            group_digits(snapshot.estimated_tokens),
            snapshot.used_percent(),
            report.message_count
        ),
        format!(
            "  Compacts at      ~{} tokens",
            group_digits(snapshot.threshold_tokens)
        ),
        format!("  Last compaction  {last_compaction}"),
        format!(
            "  Cumulative usage input {} \u{00b7} output {} \u{00b7} cache read {} \u{00b7} cache write {} \u{00b7} total {}",
            group_digits(usage.input_tokens as usize),
            group_digits(usage.output_tokens as usize),
            group_digits(usage.cache_read_input_tokens as usize),
            group_digits(usage.cache_creation_input_tokens as usize),
            group_digits(usage.total_tokens() as usize),
        ),
    ]
    .join("\n")
}

pub(crate) fn context_report_json(report: &ContextReport<'_>) -> serde_json::Value {
    let snapshot = &report.snapshot;
    json!({
        "kind": "context",
        "model": crate::provider::shown_model(report.model),
        "context_window": snapshot.budget.context_window,
        "output_reserve": snapshot.budget.output_reserve,
        "tool_schema_tokens": snapshot.budget.tool_schema_tokens,
        "system_prompt_tokens": snapshot.system_prompt_tokens,
        "input_budget": snapshot.input_budget,
        "estimated_tokens": snapshot.estimated_tokens,
        "compaction_threshold_tokens": snapshot.threshold_tokens,
        "messages": report.message_count,
        "last_compaction": report.compaction.map(|compaction| json!({
            "count": compaction.count,
            "removed_messages": compaction.removed_message_count,
        })),
        "cumulative_usage": {
            "input_tokens": report.cumulative.input_tokens,
            "output_tokens": report.cumulative.output_tokens,
            "cache_creation_input_tokens": report.cumulative.cache_creation_input_tokens,
            "cache_read_input_tokens": report.cumulative.cache_read_input_tokens,
            "total_tokens": report.cumulative.total_tokens(),
        },
    })
}

fn group_digits(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::{
        context_command_argument, describe_turn_outcome, dispatch_line, group_digits,
        handle_turn_interrupt, render_context_report, watch_turn_interrupts, ContextReport,
        InterruptAction, InterruptPresses, QuitHint, ReplBackend, ReplStep,
    };
    use crate::{CliAction, ResumeLaunchOptions, DEFAULT_MODEL};
    use runtime::context_manager::{ContextBudget, ContextManager, ContextPolicy};
    use runtime::PermissionMode;
    use runtime::{
        CancellationToken, ConversationMessage, HookAbortSignal, SessionCompaction, TokenUsage,
        TurnOutcome, TurnSummary,
    };
    use std::path::PathBuf;

    /// Fake backend: every prompt fails, `/boom` panics, `/quit` exits.
    #[derive(Default)]
    struct FlakyBackend {
        prompts: Vec<String>,
        persisted: usize,
    }

    impl ReplBackend for FlakyBackend {
        fn handle_local(
            &mut self,
            line: &str,
        ) -> Option<Result<ReplStep, Box<dyn std::error::Error>>> {
            match line {
                "/quit" => Some(Ok(ReplStep::Exit)),
                "/boom" => panic!("command exploded"),
                "/bad" => Some(Err("command failed".into())),
                _ => None,
            }
        }

        fn run_prompt(&mut self, line: &str) -> Result<(), Box<dyn std::error::Error>> {
            self.prompts.push(line.to_string());
            Err(format!("upstream error for {line}").into())
        }

        fn persist(&mut self) -> Result<(), Box<dyn std::error::Error>> {
            self.persisted += 1;
            Ok(())
        }
    }

    #[test]
    fn repl_survives_turn_errors_command_errors_and_panics() {
        let mut backend = FlakyBackend::default();
        let mut errors = Vec::new();

        let steps = ["first", "/bad", "/boom", "  ", "second", "/quit"]
            .iter()
            .map(|line| dispatch_line(&mut backend, line, &mut errors))
            .collect::<Vec<_>>();

        assert_eq!(
            steps,
            vec![
                ReplStep::Continue,
                ReplStep::Continue,
                ReplStep::Continue,
                ReplStep::Continue,
                ReplStep::Continue,
                ReplStep::Exit
            ]
        );
        assert_eq!(backend.prompts, vec!["first", "second"]);
        assert_eq!(backend.persisted, 4, "session saved after every failure");
        let errors = String::from_utf8(errors).expect("utf8");
        assert!(errors.contains("upstream error for first"));
        assert!(errors.contains("command failed"));
        assert!(errors.contains("internal error: command exploded"));
        assert!(errors.contains("The session is saved"));
    }

    #[test]
    fn first_ctrl_c_cancels_the_turn_and_second_force_quits() {
        let cancel = CancellationToken::new();
        let hooks = HookAbortSignal::new();
        assert_eq!(
            handle_turn_interrupt(1, &cancel, &hooks, QuitHint::Shown),
            InterruptAction::Cancelled
        );
        assert!(cancel.is_cancelled());
        assert!(hooks.is_aborted());
        assert_eq!(
            handle_turn_interrupt(2, &cancel, &hooks, QuitHint::Shown),
            InterruptAction::ForceQuit
        );
    }

    fn current_thread_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
    }

    #[test]
    fn monitored_ctrl_c_reaches_the_turn_token_and_a_second_press_quits() {
        let cancel = CancellationToken::new();
        let hooks = HookAbortSignal::new();
        let mut presses_left = 2_u32;
        let mut force_quits = 0_u32;

        current_thread_runtime().block_on(watch_turn_interrupts(
            || {
                let press = if presses_left > 0 {
                    presses_left -= 1;
                    Ok(())
                } else {
                    Err(std::io::Error::other("signal source closed"))
                };
                std::future::ready(press)
            },
            std::future::pending(),
            &cancel,
            &hooks,
            QuitHint::Shown,
            || force_quits += 1,
        ));

        assert!(cancel.is_cancelled(), "the first press cancels the turn");
        assert!(hooks.is_aborted(), "and aborts running hooks");
        assert_eq!(force_quits, 1, "the second press asks to quit");
    }

    #[test]
    fn ui_key_presses_interrupt_the_turn_like_the_signal() {
        let cancel = CancellationToken::new();
        let hooks = HookAbortSignal::new();
        let presses = InterruptPresses::default();
        // Pressed before the watcher waits: the press is kept, not lost.
        presses.press();
        let mut force_quits = 0_u32;

        current_thread_runtime().block_on(async {
            let ui = presses.clone();
            let stop = async move {
                tokio::task::yield_now().await;
                ui.press();
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;
            };
            watch_turn_interrupts(
                || presses.next_press(),
                stop,
                &cancel,
                &hooks,
                QuitHint::Hidden,
                || force_quits += 1,
            )
            .await;
        });

        assert!(
            cancel.is_cancelled(),
            "the first key press cancels the turn"
        );
        assert!(hooks.is_aborted());
        assert_eq!(force_quits, 1, "the second key press asks to quit");
    }

    #[test]
    fn interrupt_monitor_stops_with_the_turn_without_cancelling() {
        let cancel = CancellationToken::new();
        let hooks = HookAbortSignal::new();

        current_thread_runtime().block_on(watch_turn_interrupts(
            std::future::pending::<std::io::Result<()>>,
            std::future::ready(()),
            &cancel,
            &hooks,
            QuitHint::Shown,
            || panic!("no press, no quit"),
        ));

        assert!(!cancel.is_cancelled());
        assert!(!hooks.is_aborted());
    }

    fn summary(outcome: TurnOutcome, pruned: usize) -> TurnSummary {
        TurnSummary {
            assistant_messages: Vec::new(),
            tool_results: Vec::new(),
            prompt_cache_events: Vec::new(),
            iterations: 1,
            usage: TokenUsage::default(),
            auto_compaction: None,
            outcome,
            stop_reason: None,
            pruned_tool_results: pruned,
        }
    }

    #[test]
    fn describes_non_trivial_turn_outcomes() {
        assert_eq!(
            describe_turn_outcome(&summary(TurnOutcome::Completed, 0), QuitHint::Shown),
            None
        );
        let cancelled = describe_turn_outcome(
            &summary(
                TurnOutcome::Cancelled {
                    interrupted_tool_calls: 2,
                },
                0,
            ),
            QuitHint::Shown,
        )
        .expect("notice");
        assert!(cancelled.contains("2 pending tool call(s) were not run"));
        assert!(cancelled.contains("Ctrl-C again to exit"));
        let looped = describe_turn_outcome(
            &summary(
                TurnOutcome::LoopDetected {
                    tool_name: "bash".to_string(),
                    repeats: 4,
                },
                1,
            ),
            QuitHint::Shown,
        )
        .expect("notice");
        assert!(looped.contains("`bash` 4 times"));
        assert!(looped.contains("shortened 1 older tool result(s)"));
    }

    #[test]
    fn interrupt_notices_for_the_full_screen_ui_leave_out_the_quit_hint() {
        let cancelled = |calls| {
            describe_turn_outcome(
                &summary(
                    TurnOutcome::Cancelled {
                        interrupted_tool_calls: calls,
                    },
                    0,
                ),
                QuitHint::Hidden,
            )
        };
        assert_eq!(cancelled(0).as_deref(), Some("Interrupted."));
        assert_eq!(
            cancelled(3).as_deref(),
            Some("Interrupted. 3 pending tool call(s) were not run.")
        );
    }

    #[test]
    fn context_report_shows_window_usage_compaction_and_cumulative_tokens() {
        let manager = ContextManager::new(
            ContextBudget {
                context_window: 131_072,
                output_reserve: 32_000,
                tool_schema_tokens: 5_000,
            },
            ContextPolicy::default(),
        );
        let messages = vec![ConversationMessage::user_text("x".repeat(4_000))];
        let compaction = SessionCompaction {
            count: 2,
            removed_message_count: 31,
            summary: String::new(),
        };
        let report = ContextReport {
            model: "openai/auto",
            snapshot: manager.snapshot(&messages, &["system".to_string()]),
            compaction: Some(&compaction),
            cumulative: TokenUsage {
                input_tokens: 1_234_567,
                output_tokens: 89,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 10,
            },
            message_count: messages.len(),
        };

        let rendered = render_context_report(&report);

        // Shown without the `openai/` routing prefix.
        assert!(rendered.contains("  Model            auto\n"), "{rendered}");
        assert!(rendered.contains("Context window   131,072 tokens"));
        assert!(rendered.contains("Output reserve   32,000 tokens"));
        // 4,000 ASCII chars at 3 chars/token plus 4 tokens of message overhead.
        assert!(
            rendered.contains("Estimated used   ~1,338 tokens"),
            "{rendered}"
        );
        assert!(rendered.contains("Last compaction  #2 \u{00b7} 31 messages summarized"));
        assert!(rendered.contains("input 1,234,567"));
        let json = super::context_report_json(&report);
        assert_eq!(json["model"], "auto");
        assert_eq!(json["context_window"], 131_072);
        assert_eq!(json["last_compaction"]["removed_messages"], 31);
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    fn resume_options(values: &[&str]) -> (PathBuf, ResumeLaunchOptions) {
        match crate::parse_args(&args(values)).expect("resume args parse") {
            CliAction::ResumeSession {
                session_path,
                launch,
                ..
            } => (session_path, launch),
            other => panic!("expected a resume action, got {other:?}"),
        }
    }

    #[test]
    fn resume_carries_launch_flags_given_in_either_order() {
        let flags_first = resume_options(&[
            "--model",
            "groq/openai/gpt-oss-120b",
            "--permission-mode",
            "read-only",
            "--allowedTools",
            "read_file,grep_search",
            "--reasoning-effort",
            "high",
            "--base-commit",
            "abc123",
            "--allow-broad-cwd",
            "--resume",
            "s.jsonl",
        ]);
        let flags_last = resume_options(&[
            "--resume",
            "s.jsonl",
            "--model=groq/openai/gpt-oss-120b",
            "--permission-mode=read-only",
            "--allowedTools=read_file,grep_search",
            "--reasoning-effort=high",
            "--base-commit=abc123",
            "--allow-broad-cwd",
        ]);
        assert_eq!(flags_first, flags_last);
        let (session, launch) = flags_first;
        assert_eq!(session, PathBuf::from("s.jsonl"));
        assert_eq!(launch.model.as_deref(), Some("groq/openai/gpt-oss-120b"));
        assert_eq!(launch.permission_mode, Some(PermissionMode::ReadOnly));
        assert_eq!(
            launch.allowed_tools,
            Some(
                ["read_file", "grep_search"]
                    .into_iter()
                    .map(String::from)
                    .collect()
            )
        );
        assert_eq!(launch.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(launch.base_commit.as_deref(), Some("abc123"));
        assert!(launch.allow_broad_cwd);

        let (_, skip) = resume_options(&["--resume=s.jsonl", "--dangerously-skip-permissions"]);
        assert_eq!(skip.permission_mode, Some(PermissionMode::DangerFullAccess));

        // No flags: nothing is overridden, and a session file whose name
        // starts like a flag is still the session.
        let (session, none) = resume_options(&["--resume", "--modelling.jsonl"]);
        assert_eq!(session, PathBuf::from("--modelling.jsonl"));
        assert_eq!(none, ResumeLaunchOptions::default());
    }

    #[test]
    fn interactive_resume_honours_flags_and_falls_back_to_the_stored_model() {
        let explicit = super::resume_launch(
            ResumeLaunchOptions {
                model: Some("openai/gpt-4.1".to_string()),
                permission_mode: Some(PermissionMode::ReadOnly),
                allowed_tools: Some(["read_file".to_string()].into_iter().collect()),
                reasoning_effort: Some("low".to_string()),
                base_commit: Some("abc123".to_string()),
                allow_broad_cwd: true,
            },
            Some("openai/auto"),
            "s.jsonl".to_string(),
        );
        assert_eq!(explicit.model, "openai/gpt-4.1");
        assert_eq!(explicit.permission_mode, PermissionMode::ReadOnly);
        assert_eq!(
            explicit.allowed_tools,
            Some(["read_file".to_string()].into_iter().collect())
        );
        assert_eq!(explicit.reasoning_effort.as_deref(), Some("low"));
        assert_eq!(explicit.base_commit.as_deref(), Some("abc123"));
        assert!(explicit.allow_broad_cwd);
        assert_eq!(explicit.resume.as_deref(), Some("s.jsonl"));

        let stored = super::resume_launch(
            ResumeLaunchOptions::default(),
            Some("groq/openai/gpt-oss-120b"),
            "s.jsonl".to_string(),
        );
        assert_eq!(stored.model, "groq/openai/gpt-oss-120b");
        assert_eq!(stored.allowed_tools, None);
        assert!(!stored.allow_broad_cwd);

        let legacy = super::resume_launch(ResumeLaunchOptions::default(), None, "s".into());
        assert_eq!(legacy.model, DEFAULT_MODEL);
    }

    #[test]
    fn parses_context_command_arguments() {
        assert_eq!(context_command_argument("/context"), Some(""));
        assert_eq!(context_command_argument("/context show"), Some("show"));
        assert_eq!(context_command_argument("/contextual"), None);
        assert_eq!(context_command_argument("/status"), None);
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_000), "1,000");
        assert_eq!(group_digits(1_048_576), "1,048,576");
    }
}
