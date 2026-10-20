//! The engine behind the full-screen UI: runs the live CLI's turns on a thread of its
//! own, driven by [`EngineCommand`]s, and reports everything the user should see as
//! [`UiEvent`]s. It draws nothing, and the UI never touches the runtime.
//!
//! Two threads. The engine thread owns the [`LiveCli`] (its MCP servers and plugins are
//! thread-local) and runs one command at a time. The command thread reads the UI's
//! commands as they arrive, so an interrupt or an answer reaches a turn that is running;
//! everything else waits in the engine's queue.
//!
//! What the UI sees:
//! - first [`UiEvent::Ready`] (the protocol version), then [`UiEvent::Status`] for the
//!   footer;
//! - a prompt ([`EngineCommand::Submit`]): the turn's text, reasoning, tool calls and
//!   results, permission requests, notices, then [`UiEvent::Usage`] and
//!   [`UiEvent::TurnFinished`]; a failed turn sends [`UiEvent::Error`] before
//!   `TurnFinished` instead of `Usage`;
//! - a slash command: its report as a notice (or an `Error`), then `TurnFinished`;
//! - after every `TurnFinished`, a fresh `Status`;
//! - [`EngineCommand::SetModel`]: the switch report as a notice and a `Status`, or an
//!   `Error`;
//! - [`EngineCommand::ListModels`]: one [`UiEvent::Models`];
//! - `runtime::ui_hooks` notices from anywhere in the process, as notices;
//! - an `AskUserQuestion` from the agent, as a notice starting with "Question:" whose
//!   answer is the next submitted text (a `Submit` while the question waits never starts a
//!   turn).
//!
//! When the engine stops, the event channel closes.

// A turn's output goes through a TurnSink or runtime::ui_hooks, so a full-screen UI can own
// the terminal.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use super::*;
use crate::device_login::{DeviceCode, DeviceLoginObserver};
use neuron_tui::protocol::{
    AccountInfo, Attachment, EngineCommand, GitStatus, ModelInfo, PermissionReply, TokenCounts,
    UiEvent, PROTOCOL_VERSION,
};
use repl_loop::InterruptPresses;
use runtime::ui_hooks::{self, UserQuestion};
use runtime::{CancellationToken, HookAbortSignal, TurnSummary};
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{MutexGuard, PoisonError};

/// How often a pending question checks whether its turn was interrupted.
const ANSWER_CANCEL_POLL: Duration = Duration::from_millis(250);

/// How the engine starts. The launcher runs the line REPL's checks first (workspace trust,
/// provider selection), on the terminal, before the UI takes it over.
#[derive(Debug, Clone)]
pub(crate) struct EngineOptions {
    /// The model to use, as `configure_provider_for_model` resolved it; with `signed_out`,
    /// the model asked for.
    pub(crate) model: String,
    pub(crate) allowed_tools: Option<AllowedToolSet>,
    pub(crate) permission_mode: PermissionMode,
    pub(crate) reasoning_effort: Option<String>,
    /// A saved session to continue instead of starting a fresh one.
    pub(crate) resume: Option<String>,
    /// Why the provider could not start without a Zero-X sign-in
    /// ([`ProviderStart::SignInRequired`]). The engine then starts signed out: it answers
    /// `/login`, and starts the provider and the session after it.
    pub(crate) signed_out: Option<String>,
}

/// Why the engine stopped. The session is saved either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EngineExit {
    /// [`EngineCommand::Shutdown`], `/exit`, or the UI went away.
    Shutdown,
    /// A second interrupt during a turn, like Ctrl-C twice in the line REPL: the process
    /// should exit now (status 130). A watchdog exits it anyway five seconds after the
    /// second press.
    ForceQuit,
}

/// The UI's ends of a running engine, shaped like `neuron_tui::fake_engine::FakeEngine`.
pub(crate) struct EngineHandle {
    pub(crate) commands: Sender<EngineCommand>,
    pub(crate) replies: Sender<PermissionReply>,
    pub(crate) events: Receiver<UiEvent>,
    threads: Option<(JoinHandle<()>, JoinHandle<EngineExit>)>,
}

impl EngineHandle {
    /// Stops the engine (interrupting a running turn) and waits until the session is saved.
    pub(crate) fn finish(mut self) -> EngineExit {
        let _ = self.commands.send(EngineCommand::Shutdown);
        let Some((commands, engine)) = self.threads.take() else {
            return EngineExit::Shutdown;
        };
        let _ = commands.join();
        engine.join().unwrap_or(EngineExit::Shutdown)
    }
}

impl Drop for EngineHandle {
    /// Without [`Self::finish`] the engine still stops and saves, in the background.
    fn drop(&mut self) {
        let _ = self.commands.send(EngineCommand::Shutdown);
    }
}

/// Starts the engine: builds the live CLI on the engine thread (continuing
/// `options.resume` when given) and returns once it is ready, or with the error that
/// stopped it.
pub(crate) fn spawn(options: EngineOptions) -> Result<EngineHandle, String> {
    let (commands_tx, commands_rx) = mpsc::channel();
    let (replies_tx, replies_rx) = mpsc::channel();
    let (events_tx, events_rx) = mpsc::channel();
    let (queue_tx, queue_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    let shared = Arc::new(Shared::default());

    let engine_shared = Arc::clone(&shared);
    let engine = thread::Builder::new()
        .name("neuron-engine".to_string())
        .spawn(move || {
            let _hooks = UiHooks::install(&events_tx, &engine_shared);
            let exit = run_engine(
                options,
                &events_tx,
                replies_rx,
                &engine_shared,
                &queue_rx,
                &ready_tx,
            );
            // Sub-agents left running in the background must not outlive the session.
            tools::cancel_background_subagents();
            progress::shutdown_shared_runtime_state();
            exit
        })
        .map_err(|error| format!("could not start the engine: {error}"))?;
    let started = ready_rx
        .recv()
        .unwrap_or_else(|_| Err("the engine stopped while starting".to_string()));
    if let Err(message) = started {
        let _ = engine.join();
        return Err(message);
    }

    let commands = thread::Builder::new()
        .name("neuron-engine-commands".to_string())
        .spawn(move || forward_commands(&commands_rx, &queue_tx, &shared));
    let commands = match commands {
        Ok(commands) => commands,
        Err(error) => {
            // The queue's sender went with the closure: the engine stops and saves.
            let _ = engine.join();
            return Err(format!("could not start the engine: {error}"));
        }
    };
    Ok(EngineHandle {
        commands: commands_tx,
        replies: replies_tx,
        events: events_rx,
        threads: Some((commands, engine)),
    })
}

/// The engine thread: when the provider needs a sign-in, a signed-out stage until `/login`
/// succeeds, then the engine. Reports on `ready` once, before `spawn` returns, so `Ready`
/// is always the first event.
fn run_engine(
    mut options: EngineOptions,
    events: &Sender<UiEvent>,
    replies: Receiver<PermissionReply>,
    shared: &Arc<Shared>,
    queue: &Receiver<EngineCommand>,
    ready: &Sender<Result<(), String>>,
) -> EngineExit {
    let Some(reason) = options.signed_out.take() else {
        let started = open_cli(&options, events).and_then(|(cli, output)| {
            let mut engine = Engine::new(cli, output, Sender::clone(events), replies, shared);
            if let Some(reference) = &options.resume {
                engine
                    .continue_saved_session(reference)
                    .map_err(|error| error.to_string())?;
            }
            Ok(engine)
        });
        return match started {
            Ok(mut engine) => {
                engine.announce();
                let _ = ready.send(Ok(()));
                let exit = engine.serve(queue);
                engine.close();
                exit
            }
            Err(message) => {
                let _ = ready.send(Err(message));
                EngineExit::Shutdown
            }
        };
    };

    let mut signed_out = SignedOut {
        options,
        events,
        shared,
        email: None,
    };
    signed_out.announce(&reason);
    let _ = ready.send(Ok(()));
    let mut replies = Some(replies);
    loop {
        let Some(model) = signed_out.serve(queue) else {
            return EngineExit::Shutdown;
        };
        signed_out.options.model = model;
        let (cli, output) = match open_cli(&signed_out.options, events) {
            Ok(opened) => opened,
            Err(message) => {
                signed_out.fail(&message);
                continue;
            }
        };
        let Some(replies) = replies.take() else {
            return EngineExit::Shutdown;
        };
        let mut engine = Engine::new(cli, output, Sender::clone(events), replies, shared);
        engine.account_email = signed_out.email.take();
        engine.refresh_account();
        if let Some(reference) = &signed_out.options.resume {
            if let Err(error) = engine.continue_saved_session(reference) {
                engine.send(UiEvent::Error {
                    message: format!("could not continue {reference}: {error}"),
                });
            }
        }
        // The end of the `/login` that got here.
        engine.finish_turn();
        let exit = engine.serve(queue);
        engine.close();
        return exit;
    }
}

/// The live CLI the engine runs, showing its turns on the UI.
fn open_cli(
    options: &EngineOptions,
    events: &Sender<UiEvent>,
) -> Result<(LiveCli, TurnOutput), String> {
    let output = TurnOutput::channel(Sender::clone(events));
    let mut cli = LiveCli::with_output(
        options.model.clone(),
        true,
        options.allowed_tools.clone(),
        options.permission_mode,
        output.clone(),
    )
    .map_err(|error| error.to_string())?;
    cli.set_reasoning_effort(options.reasoning_effort.clone());
    Ok((cli, output))
}

/// The engine before a Zero-X sign-in: no provider, no session. `/login` signs in and
/// starts them; `SetModel` picks the model they will use; prompts and other slash commands
/// are refused until then.
struct SignedOut<'a> {
    options: EngineOptions,
    events: &'a Sender<UiEvent>,
    shared: &'a Shared,
    /// From the sign-in, for the engine's account view.
    email: Option<String>,
}

impl SignedOut<'_> {
    fn announce(&self, reason: &str) {
        self.send(UiEvent::Ready {
            protocol: PROTOCOL_VERSION,
            version: VERSION.to_string(),
        });
        if let Some(reason) = plain_notice(reason) {
            self.send(UiEvent::Notice { message: reason });
        }
        self.send_status();
    }

    /// Runs commands until a sign-in starts the provider (its model) or the engine stops.
    fn serve(&mut self, queue: &Receiver<EngineCommand>) -> Option<String> {
        while let Ok(command) = queue.recv() {
            if self.shared.closing.load(Ordering::SeqCst) {
                return None;
            }
            match command {
                EngineCommand::Shutdown => return None,
                EngineCommand::Slash { command, .. } => {
                    match command.trim().trim_start_matches('/') {
                        "exit" | "quit" => return None,
                        "login" => match self.login() {
                            Ok(model) => return Some(model),
                            Err(message) => self.fail(&message),
                        },
                        "logout" => {
                            crate::auth::logout();
                            self.email = None;
                            self.finish();
                        }
                        name => self.fail(&format!(
                            "/{name} needs a session: sign in to Zero-X first with /login."
                        )),
                    }
                }
                EngineCommand::Submit { .. } => {
                    self.fail("Sign in to Zero-X first: /login.");
                }
                EngineCommand::SetModel { model } if !model.trim().is_empty() => {
                    self.options.model = resolve_model_alias_with_config(model.trim());
                    self.send_status();
                }
                EngineCommand::SetModel { .. } | EngineCommand::Interrupt => {}
                EngineCommand::ListModels => self.send(UiEvent::Models {
                    current: self.options.model.clone(),
                    models: Vec::new(),
                }),
            }
        }
        None
    }

    /// Signs in, then starts the provider the sign-in was missing for.
    fn login(&mut self) -> Result<String, String> {
        let (_credential, email) = sign_in(self.events, self.shared)?;
        self.email = email;
        match start_provider(&self.options.model).map_err(|error| error.to_string())? {
            ProviderStart::Ready(model) => Ok(model),
            ProviderStart::SignInRequired(reason) => Err(reason),
        }
    }

    /// Ends a refused or failed command: the error, `TurnFinished`, the footer.
    fn fail(&self, message: &str) {
        self.send(UiEvent::Error {
            message: message.to_string(),
        });
        self.finish();
    }

    fn finish(&self) {
        self.send(UiEvent::TurnFinished);
        self.send_status();
    }

    fn send_status(&self) {
        let account = AccountState::read(self.email.as_deref());
        self.send(status_event(
            &self.options.model,
            self.options.permission_mode,
            "",
            false,
            0,
            &account,
        ));
    }

    fn send(&self, event: UiEvent) {
        let _ = self.events.send(event);
    }
}

/// The device sign-in: a [`UiEvent::SignInCode`] for the UI, then the wait for its
/// approval, which an interrupt or shutdown cancels.
fn sign_in(
    events: &Sender<UiEvent>,
    shared: &Shared,
) -> Result<(crate::auth::GatewayCredential, Option<String>), String> {
    let cancel = CancellationToken::new();
    let running = RunningTurn::start(shared, None);
    running.watch(cancel.clone(), HookAbortSignal::new());
    let cancelled = || cancel.is_cancelled();
    crate::auth::login_with(
        crate::auth::LoginOptions::default(),
        &SignInObserver { events },
        &cancelled,
    )
    .map_err(|error| error.to_string())
}

/// Shows the device code on the UI.
struct SignInObserver<'a> {
    events: &'a Sender<UiEvent>,
}

impl DeviceLoginObserver for SignInObserver<'_> {
    fn show_code(&self, code: &DeviceCode, browser_opened: bool) {
        let _ = self.events.send(UiEvent::SignInCode {
            url: code.verification_uri.clone(),
            url_complete: code.verification_uri_complete.clone(),
            user_code: code.user_code.clone(),
            expires_in_secs: code.expires_in.as_secs(),
            browser_opened,
        });
    }

    fn waiting(&self, _remaining: Duration) {}
}

/// The Zero-X login as the footer shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AccountState {
    signed_in: bool,
    info: Option<AccountInfo>,
}

impl AccountState {
    /// The login (never its token): `NEURON_TOKEN`, else the stored one. `email` is what
    /// the last sign-in in this engine reported.
    fn read(email: Option<&str>) -> Self {
        match crate::auth::current_credential() {
            Ok(Some(credential)) => Self {
                signed_in: !credential.is_expired(crate::clock::now_unix()),
                info: Some(AccountInfo {
                    email: email.map(str::to_string),
                    plan: credential.plan.clone(),
                    source: credential.source.describe(),
                    expires_at: credential.expires_at,
                }),
            },
            _ => Self::default(),
        }
    }
}

/// The footer's `Status`; git and the workspace's trust are read now.
fn status_event(
    model: &str,
    permission_mode: PermissionMode,
    session_id: &str,
    plan_mode: bool,
    context_percent: u32,
    account: &AccountState,
) -> UiEvent {
    let cwd = env::current_dir().unwrap_or_default();
    UiEvent::Status {
        model: model.to_string(),
        permission_mode: permission_mode.as_str().to_string(),
        session_id: session_id.to_string(),
        workspace_trusted: ConfigLoader::default_for(&cwd).workspace_is_trusted(),
        git: runtime::workspace_git_status(&cwd).map(|status| git_status(&status)),
        cwd: cwd.display().to_string(),
        plan_mode,
        context_percent,
        signed_in: account.signed_in,
        account: account.info.clone(),
    }
}

/// What the command thread needs to reach the engine thread's turn.
#[derive(Default)]
struct Shared {
    /// The running turn's Ctrl-C presses; `None` between turns, so an interrupt with
    /// nothing running is not kept for (and does not cancel) the next turn.
    presses: Mutex<Option<InterruptPresses>>,
    /// The running turn's cancellation, for shutdown and pending questions.
    turn: Mutex<Option<(CancellationToken, HookAbortSignal)>>,
    /// Where the answer to the pending question goes, with the question's number.
    answer: Mutex<Option<(u64, Sender<String>)>>,
    next_question: AtomicU64,
    /// Shutdown was requested: commands still queued are dropped.
    closing: AtomicBool,
}

impl Shared {
    fn turn_cancelled(&self) -> bool {
        lock(&self.turn)
            .as_ref()
            .is_some_and(|(cancel, _)| cancel.is_cancelled())
    }

    fn take_answer(&self) -> Option<Sender<String>> {
        lock(&self.answer).take().map(|(_, answer)| answer)
    }

    /// Stops the running turn, without the interrupt notice: the session is closing.
    fn stop_turn(&self) {
        if let Some((cancel, hook_abort)) = lock(&self.turn).as_ref() {
            cancel.cancel();
            hook_abort.abort();
            tools::cancel_background_subagents();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Marks a turn as running for the command thread until dropped.
struct RunningTurn<'a> {
    shared: &'a Shared,
}

impl<'a> RunningTurn<'a> {
    /// `presses` reach a turn's interrupt monitor; without them (a sign-in) an interrupt
    /// cancels the token given to [`Self::watch`].
    fn start(shared: &'a Shared, presses: Option<InterruptPresses>) -> Self {
        *lock(&shared.presses) = presses;
        Self { shared }
    }

    fn watch(&self, cancel: CancellationToken, hook_abort: HookAbortSignal) {
        *lock(&self.shared.turn) = Some((cancel, hook_abort));
        // A shutdown that came before the turn could be reached.
        if self.shared.closing.load(Ordering::SeqCst) {
            self.shared.stop_turn();
        }
    }
}

impl Drop for RunningTurn<'_> {
    fn drop(&mut self) {
        *lock(&self.shared.presses) = None;
        *lock(&self.shared.turn) = None;
    }
}

/// The command thread: interrupts and answers go straight to the running turn, other
/// commands queue for the engine. Ends at shutdown or when the UI drops its sender.
fn forward_commands(
    commands: &Receiver<EngineCommand>,
    queue: &Sender<EngineCommand>,
    shared: &Shared,
) {
    loop {
        match commands.recv().unwrap_or(EngineCommand::Shutdown) {
            EngineCommand::Interrupt => {
                if let Some(presses) = lock(&shared.presses).as_ref() {
                    presses.press();
                } else if let Some((cancel, _)) = lock(&shared.turn).as_ref() {
                    cancel.cancel();
                }
            }
            EngineCommand::Submit { text, attachments } => {
                if let Some(answer) = shared.take_answer() {
                    let _ = answer.send(text);
                } else if queue
                    .send(EngineCommand::Submit { text, attachments })
                    .is_err()
                {
                    return;
                }
            }
            EngineCommand::Shutdown => {
                shared.closing.store(true, Ordering::SeqCst);
                shared.stop_turn();
                drop(shared.take_answer());
                let _ = queue.send(EngineCommand::Shutdown);
                return;
            }
            command => {
                if queue.send(command).is_err() {
                    return;
                }
            }
        }
    }
}

/// The engine's `runtime::ui_hooks` handlers; the terminal defaults come back on drop.
struct UiHooks;

impl UiHooks {
    fn install(events: &Sender<UiEvent>, shared: &Arc<Shared>) -> Self {
        let notices = Sender::clone(events);
        ui_hooks::set_notice_handler(move |message| {
            if let Some(message) = plain_notice(message) {
                let _ = notices.send(UiEvent::Notice { message });
            }
        });
        let questions = Sender::clone(events);
        let shared = Arc::clone(shared);
        ui_hooks::set_ask_handler(move |question| ask_the_ui(&questions, &shared, question));
        Self
    }
}

impl Drop for UiHooks {
    fn drop(&mut self) {
        ui_hooks::clear_handlers();
    }
}

/// A notice as the UI shows it: no ANSI styling, no blank lines around it. `None` when
/// nothing is left.
fn plain_notice(message: &str) -> Option<String> {
    let plain = crate::render::strip_ansi(message);
    let plain = plain.trim_end().trim_start_matches(['\r', '\n']);
    (!plain.is_empty()).then(|| plain.to_string())
}

/// Shows `question` as a notice and waits for the next submitted text: the answer, raw,
/// as a user would have typed it. An interrupted turn or a closed UI gives up.
fn ask_the_ui(
    events: &Sender<UiEvent>,
    shared: &Shared,
    question: &UserQuestion,
) -> io::Result<String> {
    let (answer_tx, answer_rx) = mpsc::channel();
    let number = shared.next_question.fetch_add(1, Ordering::SeqCst);
    *lock(&shared.answer) = Some((number, answer_tx));
    let withdraw = || {
        let mut pending = lock(&shared.answer);
        if pending
            .as_ref()
            .is_some_and(|(pending, _)| *pending == number)
        {
            *pending = None;
        }
    };
    let closed = || io::Error::new(io::ErrorKind::BrokenPipe, "the Neuron UI closed");

    let asked = events.send(UiEvent::Notice {
        message: question_notice(question),
    });
    if asked.is_err() {
        withdraw();
        return Err(closed());
    }
    loop {
        match answer_rx.recv_timeout(ANSWER_CANCEL_POLL) {
            Ok(answer) => return Ok(answer),
            Err(RecvTimeoutError::Timeout)
                if shared.turn_cancelled() || shared.closing.load(Ordering::SeqCst) =>
            {
                withdraw();
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "the turn was interrupted before the question was answered",
                ));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(closed()),
        }
    }
}

fn question_notice(question: &UserQuestion) -> String {
    let mut notice = format!("Question: {}", question.question.trim());
    match question.options.as_deref() {
        Some(options) if !options.is_empty() => {
            for (index, option) in options.iter().enumerate() {
                notice.push_str(&format!("\n  {}. {option}", index + 1));
            }
            notice.push_str(&format!(
                "\nAnswer with 1-{} or in your own words.",
                options.len()
            ));
        }
        _ => notice.push_str("\nType your answer."),
    }
    notice
}

/// The tokens counted between two session totals.
fn usage_since(before: TokenUsage, after: TokenUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: after.input_tokens.saturating_sub(before.input_tokens),
        output_tokens: after.output_tokens.saturating_sub(before.output_tokens),
        cache_creation_input_tokens: after
            .cache_creation_input_tokens
            .saturating_sub(before.cache_creation_input_tokens),
        cache_read_input_tokens: after
            .cache_read_input_tokens
            .saturating_sub(before.cache_read_input_tokens),
    }
}

/// The footer's view of `git status --short --branch` output.
fn git_status(status: &str) -> GitStatus {
    let summary = parse_git_workspace_summary(Some(status));
    GitStatus {
        branch: git_branch(status),
        changed: summary.changed_files,
        staged: summary.staged_files,
        unstaged: summary.unstaged_files,
        untracked: summary.untracked_files,
        conflicted: summary.conflicted_files,
    }
}

/// The branch from the `## ...` header; a repository without commits says
/// `## No commits yet on main` (`## Initial commit on main` before git 2.15).
fn git_branch(status: &str) -> Option<String> {
    let header = status.lines().next()?.strip_prefix("## ")?;
    match header
        .strip_prefix("No commits yet on ")
        .or_else(|| header.strip_prefix("Initial commit on "))
    {
        Some(branch) => Some(branch.trim().to_string()),
        None => parse_git_status_branch(Some(status)),
    }
}

fn token_counts(usage: TokenUsage) -> TokenCounts {
    TokenCounts {
        input: u64::from(usage.input_tokens),
        output: u64::from(usage.output_tokens),
        cache_read: u64::from(usage.cache_read_input_tokens),
        cache_write: u64::from(usage.cache_creation_input_tokens),
    }
}

/// What the engine does after a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Continue,
    Stop,
}

/// The engine thread's state: the live CLI and the UI's channels.
struct Engine {
    cli: LiveCli,
    /// Every runtime the engine builds shows its turns here.
    output: TurnOutput,
    events: Sender<UiEvent>,
    replies: Receiver<PermissionReply>,
    shared: Arc<Shared>,
    /// The last permission request's id: ids never repeat within the engine.
    last_permission_id: u64,
    /// The Zero-X login, read at start and after `/login` and `/logout`.
    account: AccountState,
    /// The email the last sign-in in this engine reported.
    account_email: Option<String>,
    /// The UI stopped listening.
    ui_closed: bool,
}

impl Engine {
    fn new(
        cli: LiveCli,
        output: TurnOutput,
        events: Sender<UiEvent>,
        replies: Receiver<PermissionReply>,
        shared: &Arc<Shared>,
    ) -> Self {
        Self {
            cli,
            output,
            events,
            replies,
            shared: Arc::clone(shared),
            last_permission_id: 0,
            account: AccountState::read(None),
            account_email: None,
            ui_closed: false,
        }
    }

    fn refresh_account(&mut self) {
        self.account = AccountState::read(self.account_email.as_deref());
    }

    /// Runs queued commands until shutdown. A failure or panic in one command is reported
    /// and the engine keeps going, like the line REPL.
    fn serve(&mut self, queue: &Receiver<EngineCommand>) -> EngineExit {
        while let Ok(command) = queue.recv() {
            if self.shared.closing.load(Ordering::SeqCst) {
                break;
            }
            let step = match panic::catch_unwind(AssertUnwindSafe(|| self.handle(command))) {
                Ok(step) => step,
                Err(payload) => {
                    self.send(UiEvent::Error {
                        message: format!(
                            "internal error: {}",
                            repl_loop::panic_message(payload.as_ref())
                        ),
                    });
                    self.finish_turn();
                    self.persist();
                    Step::Continue
                }
            };
            if repl_loop::force_quit_requested() {
                return EngineExit::ForceQuit;
            }
            if step == Step::Stop || self.ui_closed {
                break;
            }
        }
        EngineExit::Shutdown
    }

    /// Saves the session; the runtime's MCP servers and plugins stop when it drops.
    fn close(mut self) {
        self.persist();
    }

    fn handle(&mut self, command: EngineCommand) -> Step {
        match command {
            EngineCommand::Submit { text, attachments } => self.submit(&text, &attachments),
            EngineCommand::Slash { command, args } => return self.slash(&command, &args),
            EngineCommand::SetModel { model } if model.trim().is_empty() => self.model_report(),
            EngineCommand::SetModel { model } => match self.switch_model(&model) {
                Ok(()) => {
                    self.persist();
                    self.send_status();
                }
                Err(error) => self.send(UiEvent::Error {
                    message: error.to_string(),
                }),
            },
            // The command thread hands interrupts to the running turn; one that gets here
            // found nothing to interrupt.
            EngineCommand::Interrupt => {}
            EngineCommand::ListModels => self.list_models(),
            EngineCommand::Shutdown => return Step::Stop,
        }
        Step::Continue
    }

    fn submit(&mut self, text: &str, attachments: &[Attachment]) {
        let line = text.trim();
        if line.is_empty() && attachments.is_empty() {
            self.finish_turn();
            return;
        }
        self.cli.record_prompt_history(line);
        let cwd = env::current_dir().unwrap_or_default();
        let prompt = try_resolve_bare_skill_prompt(&cwd, line).unwrap_or_else(|| line.to_string());
        let prompt = self.with_attachments(prompt, attachments);
        let input = self.wrap_for_mode(&prompt);
        match self.run_turn(&input) {
            Ok((summary, usage)) => self.report_turn(&summary, usage),
            Err(message) => self.send(UiEvent::Error { message }),
        }
        self.finish_turn();
    }

    /// The prompt with the attached paths listed after it; the model reads them with its
    /// tools. Images are listed too, though the model cannot see them yet.
    fn with_attachments(&mut self, prompt: String, attachments: &[Attachment]) -> String {
        if attachments.is_empty() {
            return prompt;
        }
        let mut listed = Vec::with_capacity(attachments.len());
        for attachment in attachments {
            match attachment {
                Attachment::File { path } => listed.push(format!("- {path}")),
                Attachment::Folder { path } => listed.push(format!("- {path} (folder)")),
                Attachment::Image { path, .. } => {
                    self.notice(&format!(
                        "Images are not sent to the model yet; {path} is attached as a path."
                    ));
                    listed.push(format!("- {path} (image)"));
                }
            }
        }
        format!("{prompt}\n\nAttached:\n{}", listed.join("\n"))
    }

    /// The prompt as plan mode sends it (as `LiveCli::run_turn` wraps it).
    fn wrap_for_mode(&mut self, prompt: &str) -> String {
        if self.cli.plan_mode {
            plan_mode::wrap_plan_prompt(prompt)
        } else if std::mem::take(&mut self.cli.plan_just_exited) {
            plan_mode::wrap_post_plan_prompt(prompt, self.cli.permission_mode)
        } else {
            prompt.to_string()
        }
    }

    /// Runs one turn with UI permission prompts and UI interrupts, and saves the session
    /// however it ends. Returns the summary and the turn's own token usage (the summary
    /// counts the whole session).
    fn run_turn(&mut self, input: &str) -> Result<(TurnSummary, TokenUsage), String> {
        let shared = Arc::clone(&self.shared);
        let presses = InterruptPresses::default();
        let running = RunningTurn::start(&shared, Some(presses.clone()));
        let monitor = self
            .cli
            .prepare_turn_runtime_watching(self.output.clone(), Some(presses))
            .map_err(|error| error.to_string())?;
        let cancel = self.cli.runtime.cancellation_token().clone();
        running.watch(cancel.clone(), self.cli.runtime.hook_abort_signal().clone());
        // Answers to an earlier turn's requests are stale.
        while self.replies.try_recv().is_ok() {}
        let mut prompter = ChannelPermissionPrompter::new(
            self.cli.permission_mode,
            Sender::clone(&self.events),
            &self.replies,
            cancel,
        )
        .numbered_after(self.last_permission_id);
        let before = self.cli.runtime.usage().cumulative_usage();
        let result = self.cli.runtime.run_turn(input, Some(&mut prompter));
        self.last_permission_id = prompter.last_id();
        monitor.stop();
        drop(running);
        // Every message was appended to the session file as it was produced; the full
        // snapshot keeps disk and memory identical however the turn ended.
        self.persist();
        let summary = result.map_err(|error| error.to_string())?;
        let usage = usage_since(before, summary.usage);
        Ok((summary, usage))
    }

    /// How the turn ended (when not simply finished), then its token usage.
    fn report_turn(&mut self, summary: &TurnSummary, usage: TokenUsage) {
        if let Some(event) = &summary.auto_compaction {
            self.notice(&format_auto_compaction_notice(event.removed_message_count));
        }
        if let Some(notice) = repl_loop::describe_turn_outcome(summary) {
            self.notice(&notice);
        }
        self.send(UiEvent::Usage {
            turn: token_counts(usage),
            cumulative: token_counts(summary.usage),
            estimated: false,
        });
    }

    /// Runs `/<command> <args>` and ends it with [`UiEvent::TurnFinished`]. Commands that
    /// would print to the terminal and have no UI form yet are refused with an error.
    fn slash(&mut self, command: &str, args: &str) -> Step {
        let name = command.trim().trim_start_matches('/');
        let args = args.trim();
        if matches!(name, "exit" | "quit") {
            return Step::Stop;
        }
        let result = if name == "context" {
            self.cli
                .context_command(args)
                .map(|report| self.notice(&report))
        } else {
            let line = if args.is_empty() {
                format!("/{name}")
            } else {
                format!("/{name} {args}")
            };
            match SlashCommand::parse(&line) {
                Ok(Some(command)) => self.run_slash(command),
                Ok(None) => Err(format_unknown_slash_command(name).into()),
                Err(error) => Err(error.to_string().into()),
            }
        };
        if let Err(error) = result {
            self.send(UiEvent::Error {
                message: error.to_string(),
            });
        }
        self.finish_turn();
        Step::Continue
    }

    fn run_slash(&mut self, command: SlashCommand) -> Result<(), Box<dyn std::error::Error>> {
        match command {
            SlashCommand::Help => self.notice(&render_repl_help()),
            // `/status` in the line REPL shows the context report too.
            SlashCommand::Status => {
                let report = format!(
                    "{}\n\n{}",
                    self.cli.status_report(),
                    self.cli.context_report()
                );
                self.notice(&report);
            }
            SlashCommand::Cost => {
                let usage = self.cli.runtime.usage().cumulative_usage();
                self.notice(&format_cost_report(usage));
            }
            SlashCommand::Stats => {
                let usage =
                    UsageTracker::from_session(self.cli.runtime.session()).cumulative_usage();
                self.notice(&format_cost_report(usage));
            }
            SlashCommand::Compact => self.compact()?,
            SlashCommand::Model { model: None } => self.model_report(),
            SlashCommand::Model { model: Some(model) } => {
                self.switch_model(&model)?;
                self.persist();
            }
            SlashCommand::Permissions { mode } => self.permissions(mode.as_deref())?,
            SlashCommand::Clear { confirm } => self.clear(confirm)?,
            SlashCommand::Resume { session_path: None } => self.notice(&render_resume_usage()),
            SlashCommand::Resume {
                session_path: Some(reference),
            } => {
                self.resume(&reference)?;
                self.persist();
            }
            SlashCommand::Plan { mode } => self.plan(mode.as_deref())?,
            SlashCommand::Login => self.login()?,
            SlashCommand::Logout => self.logout(),
            SlashCommand::Unknown(name) => return Err(format_unknown_slash_command(&name).into()),
            other => {
                return Err(format!(
                    "{} is not available in the full-screen UI yet.",
                    other.slash_name()
                )
                .into())
            }
        }
        Ok(())
    }

    /// `Ready`, then the footer's first `Status`.
    fn announce(&mut self) {
        self.send(UiEvent::Ready {
            protocol: PROTOCOL_VERSION,
            version: VERSION.to_string(),
        });
        self.send_status();
    }

    /// Ends a prompt or a slash command: `TurnFinished`, then the footer's new `Status`.
    fn finish_turn(&mut self) {
        self.send(UiEvent::TurnFinished);
        self.send_status();
    }

    fn send_status(&mut self) {
        let context_percent =
            u32::try_from(self.cli.runtime.context_snapshot().used_percent()).unwrap_or(u32::MAX);
        let status = status_event(
            &self.cli.model,
            self.cli.permission_mode,
            &self.cli.session.id,
            self.cli.plan_mode,
            context_percent,
            &self.account,
        );
        self.send(status);
    }

    /// `/login`: the device sign-in. A session on the Zero-X gateway switches to the new
    /// login at once (as the line REPL's `/login` does).
    fn login(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (credential, email) = sign_in(&self.events, &self.shared)?;
        self.account_email = email;
        self.refresh_account();
        let Some(active) = active_provider().filter(|active| active.id == ProviderId::ZeroX) else {
            self.notice(
                "Signed in. This session uses another provider; restart neuron (or pass --provider zero-x) to use the Zero-X gateway.",
            );
            return Ok(());
        };
        let base = active
            .base_url
            .clone()
            .unwrap_or_else(|| crate::auth::GATEWAY_BASE_URL.to_string());
        api::install_openai_compat_endpoint(Some(api::OpenAiCompatEndpoint::new(
            api::OpenAiCompatConfig::zero_x(),
            base.clone(),
            credential.token.expose(),
        )));
        if let Err(error) = crate::gateway_catalog::load(&base, Some(credential.token.expose())) {
            self.notice(&format!("warning: {error}"));
        }
        let session = self.cli.runtime.session().clone();
        let session_id = self.cli.session.id.clone();
        self.rebuild_runtime(session, &session_id)?;
        self.notice("This session now uses your new Zero-X login.");
        Ok(())
    }

    /// `/logout`: revokes and removes the stored login.
    fn logout(&mut self) {
        crate::auth::logout();
        self.account_email = None;
        self.refresh_account();
        if active_provider().is_some_and(|active| active.id == ProviderId::ZeroX) {
            self.notice("Requests in this session will fail until you run /login again.");
        }
    }

    /// The gateway catalog's models, available ones first; none on other providers.
    fn list_models(&mut self) {
        let models = gateway_repl::session_catalog()
            .map(|catalog| {
                catalog
                    .sorted()
                    .into_iter()
                    .map(|model| ModelInfo {
                        id: model.id.clone(),
                        display_name: model.display_name.clone(),
                        available: model.available,
                        unavailable_reason: model.unavailable_reason.clone(),
                        context_window: model.context_window,
                        tiers: model.tiers.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.send(UiEvent::Models {
            current: self.cli.model.clone(),
            models,
        });
    }

    fn model_report(&mut self) {
        let mut report = format_model_report(
            &self.cli.model,
            self.cli.runtime.session().messages.len(),
            self.cli.runtime.usage().turns(),
        );
        if let Some(listing) = gateway_repl::gateway_model_listing(&self.cli.model) {
            report.push_str("\n\n");
            report.push_str(&listing);
        }
        self.notice(&report);
    }

    /// `/model <model>` and [`EngineCommand::SetModel`]: the next turns use `model`. The
    /// runtime is rebuilt now, so a model that cannot be used fails here, not mid-turn.
    fn switch_model(&mut self, model: &str) -> Result<(), Box<dyn std::error::Error>> {
        let model = self.cli.resolve_model_choice(model.trim());
        if let Some(warning) = gateway_repl::gateway_model_warning(&model) {
            self.notice(&warning);
        }
        if model == self.cli.model {
            self.model_report();
            return Ok(());
        }
        let message_count = self.cli.runtime.session().messages.len();
        let previous = std::mem::replace(&mut self.cli.model, model);
        let session = self.cli.runtime.session().clone();
        let session_id = self.cli.session.id.clone();
        if let Err(error) = self.rebuild_runtime(session, &session_id) {
            self.cli.model = previous;
            return Err(error);
        }
        let report = format_model_switch_report(&previous, &self.cli.model, message_count);
        self.notice(&report);
        Ok(())
    }

    fn permissions(&mut self, mode: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let Some(mode) = mode else {
            self.notice(&format_permissions_report(
                self.cli.permission_mode.as_str(),
            ));
            return Ok(());
        };
        let normalized = normalize_permission_mode(mode).ok_or_else(|| {
            format!(
                "unsupported permission mode '{mode}'. Use read-only, workspace-write, or danger-full-access."
            )
        })?;
        if normalized == self.cli.permission_mode.as_str() {
            self.notice(&format_permissions_report(normalized));
            return Ok(());
        }
        let previous = self.cli.permission_mode.as_str();
        self.cli.permission_mode = permission_mode_from_label(normalized);
        let session = self.cli.runtime.session().clone();
        let session_id = self.cli.session.id.clone();
        self.rebuild_runtime(session, &session_id)?;
        self.notice(&format_permissions_switch_report(previous, normalized));
        self.persist();
        Ok(())
    }

    fn compact(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let result = self.cli.runtime.compact(CompactionConfig::default());
        let removed = result.removed_message_count;
        let kept = result.compacted_session.messages.len();
        let session_id = self.cli.session.id.clone();
        self.rebuild_runtime(result.compacted_session, &session_id)?;
        self.cli.persist_session()?;
        self.notice(&format_compact_report(removed, kept, removed == 0));
        Ok(())
    }

    fn clear(&mut self, confirm: bool) -> Result<(), Box<dyn std::error::Error>> {
        if !confirm {
            self.notice(
                "clear: confirmation required; run /clear --confirm to start a fresh session.",
            );
            return Ok(());
        }
        let previous = self.cli.session.clone();
        let session_state = new_cli_session()?;
        self.cli.session = create_managed_session_handle(&session_state.session_id)?;
        let session_id = self.cli.session.id.clone();
        let session = session_state.with_persistence_path(self.cli.session.path.clone());
        self.rebuild_runtime(session, &session_id)?;
        let report = self.cli.session_cleared_report(&previous);
        self.notice(&report);
        self.persist();
        Ok(())
    }

    fn resume(&mut self, reference: &str) -> Result<(), Box<dyn std::error::Error>> {
        let (handle, session) = load_session_reference(reference)?;
        let message_count = session.messages.len();
        let session_id = session.session_id.clone();
        self.rebuild_runtime(session, &handle.id)?;
        self.cli.session = SessionHandle {
            id: session_id,
            path: handle.path,
        };
        let report = format_resume_report(
            &self.cli.session.path.display().to_string(),
            message_count,
            self.cli.runtime.usage().turns(),
        );
        self.notice(&report);
        Ok(())
    }

    /// Continues a saved session at startup, removing the empty one the fresh start made.
    fn continue_saved_session(
        &mut self,
        reference: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let placeholder = self.cli.session.path.clone();
        let placeholder_is_empty = self.cli.runtime.session().messages.is_empty();
        self.resume(reference)?;
        if placeholder_is_empty && placeholder != self.cli.session.path {
            let _ = fs::remove_file(&placeholder);
        }
        Ok(())
    }

    /// `/plan [on|off]`, as in the line REPL.
    fn plan(&mut self, mode: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        match mode.map(str::trim) {
            Some("on") | None => {
                if self.cli.plan_mode {
                    self.notice("[plan] Plan mode is already active.");
                } else {
                    self.cli.plan_mode = true;
                    self.cli.permission_mode = plan_mode::enter_plan_mode(self.cli.permission_mode);
                    self.notice(
                        "[plan] Plan mode ON\n  The agent will generate architecture plans, not full code.\n  Type /plan off to exit, then ask the agent to execute.",
                    );
                }
            }
            Some("off") => {
                if self.cli.plan_mode {
                    self.cli.plan_mode = false;
                    self.cli.plan_just_exited = true;
                    self.cli.permission_mode = plan_mode::exit_plan_mode(self.cli.permission_mode);
                    let notice = format!(
                        "Plan mode OFF -- permission mode restored to {}\n  Type \"execute the plan\" to implement it now.",
                        self.cli.permission_mode.as_str()
                    );
                    self.notice(&notice);
                } else {
                    self.notice("Plan mode is not active.");
                }
            }
            Some(other) => {
                return Err(
                    format!("Unknown plan argument: \"{other}\". Usage: /plan [on|off]").into(),
                )
            }
        }
        Ok(())
    }

    /// Rebuilds the runtime for `session` with the current model, tools and permission
    /// mode, keeping the reasoning effort. Its turns show on the UI.
    fn rebuild_runtime(
        &mut self,
        session: Session,
        session_id: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let reasoning_effort = self
            .cli
            .runtime
            .api_client()
            .reasoning_effort()
            .map(ToOwned::to_owned);
        let mut runtime = build_runtime(
            session,
            session_id,
            self.cli.model.clone(),
            self.cli.system_prompt.clone(),
            true,
            self.output.clone(),
            self.cli.allowed_tools.clone(),
            self.cli.permission_mode,
            None,
        )?;
        runtime
            .api_client_mut()
            .set_reasoning_effort(reasoning_effort);
        self.cli.replace_runtime(runtime)
    }

    fn persist(&mut self) {
        if let Err(error) = self.cli.persist_session() {
            self.notice(&format!("warning: failed to save the session: {error}"));
        }
    }

    fn send(&mut self, event: UiEvent) {
        if self.events.send(event).is_err() {
            self.ui_closed = true;
        }
    }

    fn notice(&mut self, message: &str) {
        if let Some(message) = plain_notice(message) {
            self.send(UiEvent::Notice { message });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        git_status, plain_notice, question_notice, spawn, EngineExit, EngineHandle, EngineOptions,
    };
    use crate::tests::{env_lock, with_current_dir};
    use mock_anthropic_service::{MockAnthropicService, SCENARIO_PREFIX};
    use neuron_tui::protocol::{
        EngineCommand, PermissionDecision, PermissionReply, TokenCounts, UiEvent, PROTOCOL_VERSION,
    };
    use runtime::ui_hooks::{self, UserQuestion};
    use runtime::PermissionMode;
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{Receiver, RecvTimeoutError};
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    /// Generous: a turn talks to the mock service over HTTP and may start a shell.
    const EVENT_TIMEOUT: Duration = Duration::from_secs(60);

    /// Sets environment variables for one test and puts the old values back on drop.
    struct EnvVars(Vec<(&'static str, Option<OsString>)>);

    impl EnvVars {
        fn set(vars: &[(&'static str, Option<&OsStr>)]) -> Self {
            Self(
                vars.iter()
                    .map(|(name, value)| {
                        let previous = std::env::var_os(name);
                        match value {
                            Some(value) => std::env::set_var(name, value),
                            None => std::env::remove_var(name),
                        }
                        (*name, previous)
                    })
                    .collect(),
            )
        }
    }

    impl Drop for EnvVars {
        fn drop(&mut self) {
            for (name, value) in self.0.drain(..).rev() {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    fn unique_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "neuron-engine-{label}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn next_event(events: &Receiver<UiEvent>) -> UiEvent {
        events
            .recv_timeout(EVENT_TIMEOUT)
            .expect("the engine reports in time")
    }

    /// Events up to and including the first one `last` matches.
    fn events_until(events: &Receiver<UiEvent>, last: impl Fn(&UiEvent) -> bool) -> Vec<UiEvent> {
        let mut seen = Vec::new();
        loop {
            let event = next_event(events);
            let done = last(&event);
            seen.push(event);
            if done {
                return seen;
            }
        }
    }

    /// Events up to `TurnFinished`, and the `Status` that always follows it.
    fn turn_events(events: &Receiver<UiEvent>) -> Vec<UiEvent> {
        let mut turn = events_until(events, |event| *event == UiEvent::TurnFinished);
        turn.extend(events_until(events, |event| {
            matches!(event, UiEvent::Status { .. })
        }));
        turn
    }

    /// The next notice containing `needle`, skipping other notices and footer updates.
    fn expect_notice(events: &Receiver<UiEvent>, needle: &str) -> String {
        loop {
            match next_event(events) {
                UiEvent::Notice { message } if message.contains(needle) => return message,
                UiEvent::Notice { .. } | UiEvent::Status { .. } => {}
                other => panic!("expected a notice with {needle:?}, got {other:?}"),
            }
        }
    }

    /// The events without notices and footer updates: the hooks are process-wide, so a test
    /// running alongside may add notices of its own.
    fn without_notices(events: &[UiEvent]) -> Vec<UiEvent> {
        events
            .iter()
            .filter(|event| !matches!(event, UiEvent::Notice { .. } | UiEvent::Status { .. }))
            .cloned()
            .collect()
    }

    fn notices(events: &[UiEvent]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|event| match event {
                UiEvent::Notice { message } => Some(message.as_str()),
                _ => None,
            })
            .collect()
    }

    fn scenario(name: &str) -> EngineCommand {
        EngineCommand::Submit {
            text: format!("{SCENARIO_PREFIX}{name}"),
            attachments: Vec::new(),
        }
    }

    fn counts(input: u64, output: u64) -> TokenCounts {
        TokenCounts {
            input,
            output,
            cache_read: 0,
            cache_write: 0,
        }
    }

    /// Starts a fresh session: the mock answers from the latest tool result anywhere in the
    /// conversation, so each tool scenario needs a clean one.
    fn clear_session(engine: &EngineHandle) {
        engine
            .commands
            .send(EngineCommand::Slash {
                command: "clear".into(),
                args: "--confirm".into(),
            })
            .unwrap();
        let cleared = turn_events(&engine.events);
        assert!(
            notices(&cleared)
                .iter()
                .any(|notice| notice.starts_with("Session cleared")),
            "{cleared:?}"
        );
        assert_eq!(without_notices(&cleared), vec![UiEvent::TurnFinished]);
    }

    /// Submits a bash scenario that needs approval and returns the permission request's id
    /// once the engine waits for the answer.
    fn submit_bash_needing_approval(engine: &EngineHandle, scenario_name: &str) -> u64 {
        engine.commands.send(scenario(scenario_name)).unwrap();
        let asked = events_until(&engine.events, |event| {
            matches!(event, UiEvent::PermissionRequested { .. })
        });
        match without_notices(&asked).as_slice() {
            [UiEvent::ToolCallStarted {
                name,
                input,
                cut_off: false,
                ..
            }, UiEvent::PermissionRequested { id, request }] => {
                assert_eq!(name, "bash");
                assert!(input.contains("| tee "), "{input}");
                assert_eq!(request.tool, "bash");
                assert_eq!(request.current_mode, "workspace-write");
                assert_eq!(request.required_mode, "danger-full-access");
                *id
            }
            other => panic!("expected the tool call, then its approval request: {other:?}"),
        }
    }

    fn saved_session_files(dir: &Path) -> Vec<String> {
        let mut contents = Vec::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return contents;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                contents.extend(saved_session_files(&path));
            } else if path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
            {
                contents.push(fs::read_to_string(&path).unwrap_or_default());
            }
        }
        contents
    }

    #[test]
    fn engine_runs_ui_turns_against_the_mock_service() {
        let _env = env_lock();
        let tokio = tokio::runtime::Runtime::new().expect("tokio runtime");
        let server = tokio
            .block_on(MockAnthropicService::spawn())
            .expect("the mock service starts");
        let workspace = unique_dir("workspace");
        let config_home = unique_dir("config");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::create_dir_all(&config_home).expect("config home");
        fs::write(
            config_home.join("settings.json"),
            r#"{"defaultMcpServers":false}"#,
        )
        .expect("settings");
        // Its own repository, so the footer's git state does not depend on where the
        // temporary directory lives.
        let git_init = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&workspace)
            .status()
            .expect("git runs");
        assert!(git_init.success(), "git init");
        let base_url = server.base_url();
        let vars = EnvVars::set(&[
            ("ANTHROPIC_API_KEY", Some(OsStr::new("test-engine-key"))),
            ("ANTHROPIC_AUTH_TOKEN", None),
            ("ANTHROPIC_BASE_URL", Some(OsStr::new(&base_url))),
            ("NEURON_CONFIG_HOME", Some(config_home.as_os_str())),
            // The login is read for the footer: an empty store of its own, never the
            // real keychain.
            ("NEURON_HOME", Some(config_home.as_os_str())),
            ("NEURON_CREDENTIAL_STORE", Some(OsStr::new("file"))),
            ("NEURON_TOKEN", None),
        ]);

        with_current_dir(&workspace, || drive_engine(&workspace));

        drop(vars);
        drop(server);
        let _ = fs::remove_dir_all(&workspace);
        let _ = fs::remove_dir_all(&config_home);
    }

    #[allow(clippy::too_many_lines)]
    fn drive_engine(workspace: &Path) {
        let engine = spawn(EngineOptions {
            model: "claude-sonnet-4-6".to_string(),
            allowed_tools: Some(["bash".to_string()].into_iter().collect()),
            permission_mode: PermissionMode::WorkspaceWrite,
            reasoning_effort: None,
            resume: None,
            signed_out: None,
        })
        .expect("the engine starts");

        // First the protocol version, then the footer.
        assert_eq!(
            next_event(&engine.events),
            UiEvent::Ready {
                protocol: PROTOCOL_VERSION,
                version: env!("CARGO_PKG_VERSION").to_string(),
            }
        );
        let untracked_at_start = match next_event(&engine.events) {
            UiEvent::Status {
                model,
                permission_mode,
                plan_mode: false,
                git: Some(git),
                signed_in: false,
                account: None,
                ..
            } => {
                assert_eq!(model, "claude-sonnet-4-6");
                assert_eq!(permission_mode, "workspace-write");
                assert_eq!(git.branch.as_deref(), Some("main"), "{git:?}");
                git.untracked
            }
            other => panic!("expected the footer status, got {other:?}"),
        };

        // Notices from anywhere in the process reach the UI, as plain text.
        ui_hooks::notice("\n\x1b[33m!\x1b[0m engine-test notice\n");
        assert_eq!(
            expect_notice(&engine.events, "engine-test notice"),
            "! engine-test notice"
        );

        // AskUserQuestion: the question is a notice, the next submitted text its answer.
        let ask = ui_hooks::ask_handler().expect("the engine answers questions");
        let asker = thread::spawn(move || {
            ask(&UserQuestion {
                question: "Which branch?".into(),
                options: Some(vec!["main".into(), "dev".into()]),
            })
        });
        let question = expect_notice(&engine.events, "Question: Which branch?");
        assert!(question.contains("\n  2. dev"), "{question}");
        engine
            .commands
            .send(EngineCommand::Submit {
                text: "2".into(),
                attachments: Vec::new(),
            })
            .unwrap();
        assert_eq!(asker.join().unwrap().expect("answered"), "2");

        // Nothing runs: the interrupt is dropped, not kept to cancel the next turn.
        engine.commands.send(EngineCommand::Interrupt).unwrap();

        // Streamed text, then the turn's real usage.
        engine.commands.send(scenario("streaming_text")).unwrap();
        assert_eq!(
            without_notices(&turn_events(&engine.events)),
            vec![
                UiEvent::TextDelta {
                    text: "Mock streaming ".into()
                },
                UiEvent::TextDelta {
                    text: "says hello from the parity harness.".into()
                },
                UiEvent::TextBlockEnd,
                UiEvent::Usage {
                    turn: counts(11, 8),
                    cumulative: counts(11, 8),
                    estimated: false,
                },
                UiEvent::TurnFinished,
            ]
        );

        // A tool call that needs approval, approved: it runs.
        let approved = submit_bash_needing_approval(&engine, "bash_permission_prompt_approved");
        engine
            .replies
            .send(PermissionReply {
                id: approved,
                decision: PermissionDecision::AllowOnce,
            })
            .unwrap();
        let turn = turn_events(&engine.events);
        match without_notices(&turn).as_slice() {
            [UiEvent::ToolResult {
                id,
                name,
                output,
                is_error: false,
            }, UiEvent::TextDelta { text }, UiEvent::TextBlockEnd, UiEvent::Usage {
                turn,
                cumulative,
                estimated: false,
            }, UiEvent::TurnFinished] => {
                assert_eq!(id, "toolu_bash_prompt_allow", "the call's id");
                assert_eq!(name, "bash");
                assert!(output.contains("approved via prompt"), "{output}");
                assert_eq!(text, "bash approved and executed: approved via prompt");
                // Two model calls: the tool call (12 in, 4 out), then the answer (14, 7).
                assert_eq!(*turn, counts(26, 11));
                assert_eq!(*cumulative, counts(37, 19));
            }
            other => panic!("unexpected approved turn: {other:?}"),
        }
        assert!(workspace.join("approved.txt").exists());
        match turn.last() {
            Some(UiEvent::Status { git: Some(git), .. }) => assert_eq!(
                git.untracked,
                untracked_at_start + 1,
                "the footer sees the new file: {git:?}"
            ),
            other => panic!("expected the footer status after the turn, got {other:?}"),
        }

        // Denied: it does not run, the UI gets its result, and the model hears why.
        clear_session(&engine);
        let denied = submit_bash_needing_approval(&engine, "bash_permission_prompt_denied");
        assert_eq!(denied, approved + 1, "request ids never repeat");
        engine
            .replies
            .send(PermissionReply {
                id: denied,
                decision: PermissionDecision::Deny { note: None },
            })
            .unwrap();
        let turn = turn_events(&engine.events);
        match without_notices(&turn).as_slice() {
            [UiEvent::ToolResult {
                id,
                is_error: true,
                output,
                ..
            }, UiEvent::TextDelta { text }, UiEvent::TextBlockEnd, UiEvent::Usage {
                turn, cumulative, ..
            }, UiEvent::TurnFinished] => {
                assert_eq!(id, "toolu_bash_prompt_deny");
                assert_eq!(output, "tool 'bash' denied by user approval prompt");
                assert!(
                    text.starts_with("bash denied as expected: ")
                        && text.contains("denied by user approval prompt"),
                    "{text}"
                );
                assert_eq!(turn, cumulative, "a cleared session counts afresh");
            }
            other => panic!("unexpected denied turn: {other:?}"),
        }
        assert!(!workspace.join("denied.txt").exists());

        // Interrupted while it waits for approval: nothing runs and the turn ends.
        clear_session(&engine);
        submit_bash_needing_approval(&engine, "bash_permission_prompt_approved");
        engine.commands.send(EngineCommand::Interrupt).unwrap();
        let turn = turn_events(&engine.events);
        let turn_notices = notices(&turn);
        assert!(
            turn_notices
                .iter()
                .any(|notice| notice.starts_with("\u{25a0} Interrupting")),
            "{turn:?}"
        );
        assert!(
            turn_notices
                .iter()
                .any(|notice| notice.starts_with("Interrupted.")),
            "{turn:?}"
        );
        assert!(
            matches!(
                without_notices(&turn).as_slice(),
                [UiEvent::ToolResult { id, is_error: true, output, .. }, UiEvent::Usage { .. }, UiEvent::TurnFinished]
                    if id == "toolu_bash_prompt_allow" && output.contains("interrupted")
            ),
            "{turn:?}"
        );

        // Slash commands report as notices; ones without a UI form yet are refused.
        engine
            .commands
            .send(EngineCommand::Slash {
                command: "cost".into(),
                args: String::new(),
            })
            .unwrap();
        let cost = turn_events(&engine.events);
        assert!(
            notices(&cost)
                .iter()
                .any(|notice| notice.starts_with("Cost")),
            "{cost:?}"
        );
        engine
            .commands
            .send(EngineCommand::Slash {
                command: "vim".into(),
                args: String::new(),
            })
            .unwrap();
        assert_eq!(
            without_notices(&turn_events(&engine.events)),
            vec![
                UiEvent::Error {
                    message: "/vim is not available in the full-screen UI yet.".into()
                },
                UiEvent::TurnFinished,
            ]
        );

        // SetModel switches for the next turn.
        engine
            .commands
            .send(EngineCommand::SetModel {
                model: "opus".into(),
            })
            .unwrap();
        let switched = expect_notice(&engine.events, "Model updated");
        assert!(switched.contains("claude-opus"), "{switched}");
        match events_until(&engine.events, |event| {
            matches!(event, UiEvent::Status { .. })
        })
        .pop()
        {
            Some(UiEvent::Status { model, .. }) => assert!(model.contains("claude-opus")),
            other => panic!("expected the footer status, got {other:?}"),
        }

        // Only the Zero-X gateway publishes a model catalog.
        engine.commands.send(EngineCommand::ListModels).unwrap();
        match events_until(&engine.events, |event| {
            matches!(event, UiEvent::Models { .. })
        })
        .pop()
        {
            Some(UiEvent::Models { current, models }) => {
                assert!(current.contains("claude-opus"), "{current}");
                assert!(models.is_empty(), "{models:?}");
            }
            other => panic!("expected the model list, got {other:?}"),
        }

        // Shutdown while a turn waits for approval: the turn stops, the session is saved,
        // the hooks go back to the terminal and the event channel closes.
        clear_session(&engine);
        submit_bash_needing_approval(&engine, "bash_permission_prompt_approved");
        engine.commands.send(EngineCommand::Shutdown).unwrap();
        let turn = turn_events(&engine.events);
        assert!(
            matches!(
                without_notices(&turn).as_slice(),
                [
                    UiEvent::ToolResult { is_error: true, .. },
                    UiEvent::Usage { .. },
                    UiEvent::TurnFinished
                ]
            ),
            "{turn:?}"
        );
        loop {
            match engine.events.recv_timeout(EVENT_TIMEOUT) {
                Ok(_) => {}
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => panic!("the engine did not stop"),
            }
        }
        assert_eq!(engine.finish(), EngineExit::Shutdown);
        assert!(!ui_hooks::notice_handler_installed());
        assert!(ui_hooks::ask_handler().is_none());
        let sessions = saved_session_files(&workspace.join(".neuron"));
        assert!(
            sessions.iter().any(
                |session| session.contains("bash_permission_prompt_approved")
                    && session.contains("claude-opus")
            ),
            "the last session is saved: {sessions:?}"
        );
    }

    #[test]
    fn a_failed_start_reports_the_error() {
        let _env = env_lock();
        let workspace = unique_dir("no-credentials");
        let config_home = unique_dir("no-credentials-config");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::create_dir_all(&config_home).expect("config home");
        let vars = EnvVars::set(&[
            ("ANTHROPIC_API_KEY", None),
            ("ANTHROPIC_AUTH_TOKEN", None),
            ("NEURON_CONFIG_HOME", Some(config_home.as_os_str())),
            ("NEURON_HOME", Some(config_home.as_os_str())),
            ("NEURON_CREDENTIAL_STORE", Some(OsStr::new("file"))),
            ("NEURON_TOKEN", None),
        ]);
        let started = with_current_dir(&workspace, || {
            spawn(EngineOptions {
                model: "claude-sonnet-4-6".to_string(),
                allowed_tools: None,
                permission_mode: PermissionMode::ReadOnly,
                reasoning_effort: None,
                resume: None,
                signed_out: None,
            })
        });
        let Err(message) = started else {
            panic!("no credentials, no engine");
        };
        assert!(message.contains("ANTHROPIC_API_KEY"), "{message}");
        assert!(
            !ui_hooks::notice_handler_installed(),
            "the hooks go back to the terminal"
        );
        drop(vars);
        let _ = fs::remove_dir_all(&workspace);
        let _ = fs::remove_dir_all(&config_home);
    }

    #[test]
    fn the_footer_reads_git_status_headers() {
        let status = "## main...origin/main [ahead 1]\nM  src/lib.rs\n M README.md\n?? new.txt";
        let git = git_status(status);
        assert_eq!(git.branch.as_deref(), Some("main"));
        assert_eq!(
            (git.changed, git.staged, git.unstaged, git.untracked),
            (3, 1, 1, 1)
        );
        assert_eq!(
            git_status("## No commits yet on trunk").branch.as_deref(),
            Some("trunk")
        );
        assert_eq!(
            git_status("## Initial commit on master").branch.as_deref(),
            Some("master")
        );
        assert_eq!(
            git_status("## HEAD (no branch)").branch.as_deref(),
            Some("detached HEAD")
        );
    }

    #[test]
    fn notices_and_questions_read_as_plain_text() {
        assert_eq!(
            plain_notice(
                "\n\x1b[33m\u{25a0}\x1b[0m Interrupting\u{2026} \x1b[2m(press Ctrl-C again to quit)\x1b[0m"
            ),
            Some("\u{25a0} Interrupting\u{2026} (press Ctrl-C again to quit)".to_string())
        );
        assert_eq!(plain_notice("\n\x1b[0m \n"), None);
        assert_eq!(
            plain_notice("Context\n  Model  x\n"),
            Some("Context\n  Model  x".to_string())
        );
        assert_eq!(
            question_notice(&UserQuestion {
                question: "Ship it?".into(),
                options: None,
            }),
            "Question: Ship it?\nType your answer."
        );
        assert_eq!(
            question_notice(&UserQuestion {
                question: "Which branch?".into(),
                options: Some(vec!["main".into(), "dev".into()]),
            }),
            "Question: Which branch?\n  1. main\n  2. dev\nAnswer with 1-2 or in your own words."
        );
    }
}
