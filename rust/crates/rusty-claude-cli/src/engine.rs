//! The engine behind the full-screen UI: runs the live CLI's turns on a thread of its
//! own, driven by [`EngineCommand`]s, and reports everything the user should see as
//! [`UiEvent`]s. It draws nothing, and the UI never touches the runtime.
//!
//! Two threads. The engine thread owns the [`LiveCli`] (its MCP servers and plugins are
//! thread-local) and runs one command at a time. The command thread reads the UI's
//! commands as they arrive, so an interrupt or an answer reaches a turn that is running,
//! and a model listing is answered during one; everything else waits in the engine's
//! queue.
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
//! - [`EngineCommand::ListModels`]: one [`UiEvent::Models`], also while a turn runs;
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
use crate::engine_account::{self, SharedAccountCache};
use crate::engine_reports;
use crate::engine_trust;
use crate::error_detail::DetailedError;
use crate::session_presence::{self, SessionPresence};
use neuron_tui::protocol::{
    AccountInfo, Attachment, DefinitionInfo, EngineCommand, GitStatus, McpServerInfo, ModelInfo,
    PermissionReply, PluginInfo, SessionScope, TokenCounts, TranscriptEntry, UiEvent,
    CAPABILITY_QUESTIONS, CAPABILITY_REPORTS, ENGINE_FEATURES, PROTOCOL_VERSION,
};
use repl_loop::InterruptPresses;
use runtime::ui_hooks::{self, UserQuestion};
use runtime::{CancellationToken, HookAbortSignal, TurnSummary};
use runtime::{ContentBlock, MessageRole};
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
            let _models = ModelListing::install(&events_tx, &engine_shared);
            let _hooks = UiHooks::install(&events_tx, &engine_shared);
            let exit = run_engine(
                options,
                &events_tx,
                replies_rx,
                &engine_shared,
                &queue_rx,
                &ready_tx,
            );
            // Sub-agents and commands left running in the background must not outlive
            // the session.
            tools::cancel_background_subagents();
            let _ = runtime::stop_background_commands();
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
            let resumed = match &options.resume {
                Some(reference) => Some(
                    engine
                        .continue_saved_session(reference)
                        .map_err(|error| error.to_string())?,
                ),
                None => None,
            };
            Ok((engine, resumed))
        });
        return match started {
            Ok((mut engine, resumed)) => {
                engine.announce();
                if let Some(report) = resumed {
                    engine.notice(&report);
                }
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
        signed_out.send(UiEvent::SessionStarting);
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
        engine.announce_account();
        if let Some(reference) = &signed_out.options.resume {
            match engine.continue_saved_session(reference) {
                Ok(report) => {
                    engine.notice(&report);
                    engine.send_transcript();
                }
                Err(error) => engine.send(UiEvent::Error {
                    message: format!("could not continue {reference}: {error}"),
                    detail: None,
                }),
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
        self.send(ready_event());
        if let Some(reason) = plain_notice(reason) {
            self.send(UiEvent::Notice { message: reason });
        }
        self.send_status();
        self.send_account();
    }

    /// The login there is, if any (expired, or signed out): no session reads the account.
    fn send_account(&self) {
        self.send(engine_account::account_event(
            &SharedAccountCache::default(),
            self.email.as_deref(),
        ));
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
                            Err(error) => self.fail_with(error),
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
                    self.shared.prompt_dropped();
                    self.fail("Sign in to Zero-X first: /login.");
                }
                EngineCommand::SetModel { model } if !model.trim().is_empty() => {
                    self.options.model = resolve_model_alias_with_config(model.trim());
                    self.send_status();
                }
                // Capabilities and answers are the command thread's.
                EngineCommand::SetModel { .. }
                | EngineCommand::Interrupt
                | EngineCommand::Capabilities { .. }
                | EngineCommand::Answer { .. } => {}
                EngineCommand::ListModels => self.send(UiEvent::Models {
                    current: shown_model(&self.options.model).to_string(),
                    models: Vec::new(),
                }),
                // The saved chats are there before a sign-in; continuing one needs it.
                EngineCommand::ListSessions { scope } => self.send(sessions_event("", scope)),
                EngineCommand::Resume { .. } => {
                    self.send(UiEvent::Error {
                        message: "Sign in to Zero-X first (/login) to continue a chat.".into(),
                        detail: None,
                    });
                }
                EngineCommand::TrustWorkspace => {
                    for event in trust_workspace_events() {
                        self.send(event);
                    }
                    self.send_status();
                }
                EngineCommand::ListExtensions => self.send(extensions_event()),
                EngineCommand::RefreshAccount => self.send_account(),
                EngineCommand::TrustPreview => self.send(engine_trust::trust_preview_event(
                    &env::current_dir().unwrap_or_default(),
                )),
            }
        }
        None
    }

    /// Signs in, then starts the provider the sign-in was missing for.
    fn login(&mut self) -> Result<String, DetailedError> {
        let (_credential, email) = sign_in(self.events, self.shared)?;
        self.email = email;
        let plain = |message: String| DetailedError {
            message,
            detail: None,
        };
        match start_provider(&self.options.model).map_err(|error| plain(error.to_string()))? {
            ProviderStart::Ready(model) => Ok(model),
            ProviderStart::SignInRequired(reason) => Err(plain(reason)),
        }
    }

    /// Ends a refused or failed command: the error, `TurnFinished`, the footer.
    fn fail(&self, message: &str) {
        self.fail_with(DetailedError {
            message: message.to_string(),
            detail: None,
        });
    }

    /// [`Self::fail`] with what the failure needs from the user.
    fn fail_with(&self, error: DetailedError) {
        self.send(UiEvent::Error {
            message: error.message,
            detail: error.detail,
        });
        self.finish();
    }

    fn finish(&self) {
        self.send(UiEvent::TurnFinished);
        self.send_status();
    }

    fn send_status(&self) {
        lock(&self.shared.model).clone_from(&self.options.model);
        let account = AccountState::read(self.email.as_deref());
        self.send(status_event(
            &self.options.model,
            self.options.permission_mode,
            "",
            false,
            0,
            &account,
            true,
            None,
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
) -> Result<(crate::auth::GatewayCredential, Option<String>), DetailedError> {
    let cancel = CancellationToken::new();
    let running = RunningTurn::start(shared);
    running.watch(cancel.clone(), HookAbortSignal::new());
    let cancelled = || cancel.is_cancelled();
    crate::auth::login_with(
        crate::auth::LoginOptions::default(),
        &SignInObserver { events },
        &cancelled,
    )
    .map_err(|error| DetailedError::from_io(&error))
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
                    // This engine's sign-in, else the one stored with the login.
                    email: email
                        .map(str::to_string)
                        .or_else(|| credential.email.clone()),
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
#[allow(clippy::too_many_arguments)]
fn status_event(
    model: &str,
    permission_mode: PermissionMode,
    session_id: &str,
    plan_mode: bool,
    context_percent: u32,
    account: &AccountState,
    sign_in_required: bool,
    served_model: Option<&str>,
) -> UiEvent {
    let cwd = env::current_dir().unwrap_or_default();
    let shown = shown_model(model);
    UiEvent::Status {
        model: shown.to_string(),
        permission_mode: permission_mode.as_str().to_string(),
        session_id: session_id.to_string(),
        workspace_trusted: ConfigLoader::default_for(&cwd).workspace_is_trusted(),
        git: runtime::workspace_git_status(&cwd).map(|status| git_status(&status)),
        cwd: cwd.display().to_string(),
        plan_mode,
        context_percent,
        signed_in: account.signed_in,
        sign_in_required,
        account: account.info.clone(),
        supports_images: crate::gateway_catalog::model_supports_images(model),
        served_model: served_model
            .map(shown_model)
            .filter(|served| *served != shown)
            .map(ToString::to_string),
    }
}

/// The first event: the protocol version and what this engine adds to it.
fn ready_event() -> UiEvent {
    UiEvent::Ready {
        protocol: PROTOCOL_VERSION,
        version: VERSION.to_string(),
        features: ENGINE_FEATURES.iter().map(ToString::to_string).collect(),
    }
}

/// The saved chats in `scope` (this workspace's, or every known workspace's), newest
/// first; `current` is the one the engine runs (empty for none). A store that cannot be
/// read lists none.
fn sessions_event(current: &str, scope: SessionScope) -> UiEvent {
    let cwd = env::current_dir().unwrap_or_default();
    UiEvent::Sessions {
        current: current.to_string(),
        sessions: session_presence::list(scope, current, &cwd),
        scope,
    }
}

/// How much of a conversation a transcript carries: the newest entries, and no single
/// text longer than this many characters.
const TRANSCRIPT_ENTRY_LIMIT: usize = 400;
const TRANSCRIPT_TEXT_LIMIT: usize = 4_000;

/// `session`'s conversation as a UI rebuilds it: prompts, replies, tool calls and their
/// results. System messages (and a compaction's summary) are left out.
fn transcript_event(session_id: &str, session: &Session) -> UiEvent {
    let mut entries = Vec::new();
    for message in &session.messages {
        for block in &message.blocks {
            let entry = match (message.role, block) {
                (MessageRole::User, ContentBlock::Text { text }) if !text.trim().is_empty() => {
                    TranscriptEntry::User {
                        text: shorten(text, TRANSCRIPT_TEXT_LIMIT),
                    }
                }
                (MessageRole::Assistant, ContentBlock::Text { text })
                    if !text.trim().is_empty() =>
                {
                    TranscriptEntry::Assistant {
                        text: shorten(text, TRANSCRIPT_TEXT_LIMIT),
                    }
                }
                (MessageRole::Assistant, ContentBlock::ToolUse { id, name, input }) => {
                    TranscriptEntry::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        input: shorten_json_strings(input, TRANSCRIPT_TEXT_LIMIT),
                    }
                }
                (
                    _,
                    ContentBlock::ToolResult {
                        tool_use_id,
                        tool_name,
                        output,
                        is_error,
                    },
                ) => TranscriptEntry::ToolResult {
                    id: tool_use_id.clone(),
                    name: tool_name.clone(),
                    output: shorten(output, TRANSCRIPT_TEXT_LIMIT),
                    is_error: *is_error,
                },
                _ => continue,
            };
            entries.push(entry);
        }
    }
    let truncated = entries.len() > TRANSCRIPT_ENTRY_LIMIT;
    if truncated {
        entries.drain(..entries.len() - TRANSCRIPT_ENTRY_LIMIT);
    }
    UiEvent::Transcript {
        session_id: session_id.to_string(),
        entries,
        truncated,
    }
}

/// `text` cut to `limit` characters, saying how much was left out.
fn shorten(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        return text.to_string();
    }
    let kept: String = text.chars().take(limit).collect();
    format!("{kept}\n\u{2026} ({} more characters)", count - limit)
}

/// A tool input with its long string values shortened, so it stays valid JSON (a UI
/// reads paths and diffs from it). Input that is not JSON is shortened as text.
fn shorten_json_strings(input: &str, limit: usize) -> String {
    fn walk(value: &mut serde_json::Value, limit: usize) {
        match value {
            serde_json::Value::String(text) if text.chars().count() > limit => {
                *text = shorten(text, limit);
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(|item| walk(item, limit)),
            serde_json::Value::Object(fields) => {
                fields.values_mut().for_each(|field| walk(field, limit));
            }
            _ => {}
        }
    }
    if input.chars().count() <= limit {
        return input.to_string();
    }
    match serde_json::from_str::<serde_json::Value>(input) {
        Ok(mut value) => {
            walk(&mut value, limit);
            value.to_string()
        }
        Err(_) => shorten(input, limit),
    }
}

/// `TrustWorkspace`: records trust for the working directory's git root (the folder itself
/// outside git) in the trust store; the home folder is trusted for this engine only. The
/// workspace's own settings, hooks and MCP servers load when a session starts, so the UI
/// restarts the engine to use them.
fn trust_workspace_events() -> Vec<UiEvent> {
    let cwd = env::current_dir().unwrap_or_default();
    match runtime::workspace_trust::trust_workspace_root(&cwd) {
        Ok(trusted) => vec![
            UiEvent::WorkspaceTrusted {
                cwd: cwd.display().to_string(),
                restart_required: true,
                root: Some(trusted.root.display().to_string()),
                session_only: trusted.session_only,
            },
            UiEvent::Notice {
                message: if trusted.session_only {
                    format!(
                        "Trusted {} for this session only. Its settings, hooks and MCP servers load when the session restarts.",
                        cwd.display()
                    )
                } else {
                    format!(
                        "Trusted {}. Its settings, hooks and MCP servers load when the session restarts.",
                        trusted.root.display()
                    )
                },
            },
        ],
        Err(error) => vec![UiEvent::error(format!(
            "could not trust {}: {error}",
            cwd.display()
        ))],
    }
}

/// `ListExtensions`: the configured MCP servers with what each is doing now, the
/// plugins, and the skills and agent definitions this workspace sees. A server's
/// command, arguments, environment and headers are left out: they can hold secrets.
fn extensions_event() -> UiEvent {
    let cwd = env::current_dir().unwrap_or_default();
    let loader = ConfigLoader::default_for(&cwd);
    let config = loader.load();
    let snapshot = extensions_snapshot();
    let live = snapshot.as_ref().and_then(|snapshot| snapshot.mcp.as_ref());
    let mcp_servers = match &config {
        Ok(config) => config
            .mcp()
            .servers()
            .iter()
            .map(|(name, server)| {
                let (status, tools, error) = mcp_server_status(name, live);
                McpServerInfo {
                    name: name.clone(),
                    scope: config_scope_id(server.scope).to_string(),
                    transport: mcp_transport_id(server.transport()).to_string(),
                    status: status.to_string(),
                    tools,
                    error,
                }
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    // Before a runtime was built (signed out), the plugins come from the manager.
    let plugins = match snapshot {
        Some(snapshot) => snapshot.plugins,
        None => config
            .as_ref()
            .ok()
            .and_then(|config| {
                build_plugin_manager(&cwd, &loader, config)
                    .list_plugins()
                    .ok()
            })
            .unwrap_or_default(),
    };
    UiEvent::Extensions {
        mcp_servers,
        plugins: plugins
            .into_iter()
            .map(|plugin| PluginInfo {
                id: plugin.metadata.id,
                name: plugin.metadata.name,
                version: plugin.metadata.version,
                description: plugin.metadata.description,
                kind: plugin.metadata.kind.to_string(),
                enabled: plugin.enabled,
            })
            .collect(),
        skills: definitions(
            commands::handle_skills_slash_command_json(None, &cwd).ok(),
            "skills",
        ),
        agents: definitions(
            commands::handle_agents_slash_command_json(None, &cwd).ok(),
            "agents",
        ),
    }
}

/// What the MCP server `name` is doing: its status, its tools (without the
/// `mcp__<server>__` prefix) and why it failed or is unsupported. `live` is `None`
/// before the session's runtime started its servers.
fn mcp_server_status(
    name: &str,
    live: Option<&McpRuntimeView>,
) -> (&'static str, Vec<String>, Option<String>) {
    let Some(live) = live else {
        return ("not_started", Vec::new(), None);
    };
    let reason = |servers: &[(String, String)]| {
        servers
            .iter()
            .find(|(server, _)| server == name)
            .map(|(_, reason)| reason.clone())
    };
    if let Some(reason) = reason(&live.unsupported) {
        return ("unsupported", Vec::new(), Some(reason));
    }
    if live.starting {
        return ("starting", Vec::new(), None);
    }
    if let Some(message) = reason(&live.failed) {
        return ("failed", Vec::new(), Some(message));
    }
    if live.pending.iter().any(|server| server == name) {
        return ("pending", Vec::new(), None);
    }
    if live.running.iter().any(|server| server == name) {
        let prefix = runtime::mcp_tool_prefix(name);
        let tools = live
            .tools
            .iter()
            .filter_map(|tool| tool.strip_prefix(&prefix))
            .map(str::to_string)
            .collect();
        return ("ready", tools, None);
    }
    ("not_started", Vec::new(), None)
}

fn config_scope_id(scope: ConfigSource) -> &'static str {
    match scope {
        ConfigSource::Bundled => "bundled",
        ConfigSource::User => "user",
        ConfigSource::Project => "project",
        ConfigSource::Local => "local",
    }
}

pub(crate) fn mcp_transport_id(transport: runtime::McpTransport) -> &'static str {
    match transport {
        runtime::McpTransport::Stdio => "stdio",
        runtime::McpTransport::Sse => "sse",
        runtime::McpTransport::Http => "http",
        runtime::McpTransport::Ws => "ws",
        runtime::McpTransport::Sdk => "sdk",
        runtime::McpTransport::ManagedProxy => "managed_proxy",
    }
}

/// The skills or agents (`key`) of a `/skills` or `/agents` JSON report.
fn definitions(report: Option<serde_json::Value>, key: &str) -> Vec<DefinitionInfo> {
    let Some(report) = report else {
        return Vec::new();
    };
    report
        .get(key)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|definition| {
            Some(DefinitionInfo {
                name: definition.get("name")?.as_str()?.to_string(),
                description: definition
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                source: definition
                    .pointer("/source/label")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                active: definition
                    .get("active")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true),
            })
        })
        .collect()
}

/// What the command thread needs to reach the engine thread's turn.
#[derive(Default)]
struct Shared {
    /// Where an interrupt goes. Lock it before `turn` when taking both.
    interrupts: Mutex<Interrupts>,
    /// The running turn's cancellation, for shutdown and pending questions.
    turn: Mutex<Option<(CancellationToken, HookAbortSignal)>>,
    /// Where the answer to the pending question goes, with the question's number.
    answer: Mutex<Option<(u64, Sender<String>)>>,
    next_question: AtomicU64,
    /// The UI takes questions as [`UiEvent::Question`] (its `questions` capability).
    structured_questions: AtomicBool,
    /// The UI shows reports as [`UiEvent::Report`] (its `reports` capability).
    report_panels: AtomicBool,
    /// Which UI this is, as its capabilities named it (`terminal`, `desktop`).
    client: Mutex<Option<String>>,
    /// Shutdown was requested: commands still queued are dropped.
    closing: AtomicBool,
    /// The UI's events while the engine runs, for answering [`EngineCommand::ListModels`]
    /// during a turn ([`ModelListing`]).
    events: Mutex<Option<Sender<UiEvent>>>,
    /// The session's model as the footer last showed it.
    model: Mutex<String>,
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

    fn turn_running(&self) -> bool {
        lock(&self.interrupts).presses.is_some() || lock(&self.turn).is_some()
    }

    /// The UI's interrupt: the running turn's, else one for the prompt the engine has
    /// not reached yet (it starts cancelled), else nothing to interrupt.
    fn interrupt(&self) {
        let mut guard = lock(&self.interrupts);
        let interrupts = &mut *guard;
        let turn = lock(&self.turn);
        if let Some(presses) = &interrupts.presses {
            // The turn's monitor reports the press (and quits on a second one).
            presses.press();
            match turn.as_ref() {
                // Now, not when the monitor gets to the press.
                Some((cancel, _)) => cancel.cancel(),
                // The turn's cancellation is not installed yet: `watch` cancels it
                // before the first request.
                None => interrupts.early = true,
            }
        } else if let Some((cancel, _)) = turn.as_ref() {
            cancel.cancel();
        } else if interrupts.queued_prompts > 0 {
            interrupts.early = true;
        }
    }

    /// The command thread queued a prompt for the engine.
    fn prompt_queued(&self) {
        lock(&self.interrupts).queued_prompts += 1;
    }

    /// The engine took a queued prompt and will not run it (no session yet): an early
    /// interrupt meant for it is spent.
    fn prompt_dropped(&self) {
        let mut interrupts = lock(&self.interrupts);
        interrupts.queued_prompts = interrupts.queued_prompts.saturating_sub(1);
        interrupts.early = false;
    }

    /// Answers [`EngineCommand::ListModels`] now, on a thread of its own (the catalog may
    /// be fetched, and interrupts must not wait for it). `false` when the engine is not
    /// running.
    fn answer_models(&self) -> bool {
        let Some(events) = lock(&self.events).clone() else {
            return false;
        };
        let current = lock(&self.model).clone();
        thread::Builder::new()
            .name("neuron-engine-models".to_string())
            .spawn(move || {
                let _ = events.send(models_event(&current));
            })
            .is_ok()
    }

    /// The pending question's answer channel, when `id` is that question.
    fn take_answer_to(&self, id: u64) -> Option<Sender<String>> {
        let mut pending = lock(&self.answer);
        if pending.as_ref().is_some_and(|(number, _)| *number == id) {
            pending.take().map(|(_, answer)| answer)
        } else {
            None
        }
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

/// Where the UI's interrupts go ([`Shared::interrupt`]). An interrupt with nothing
/// running and no prompt queued is dropped, not kept to cancel a later turn; one sent
/// right after a prompt, before the engine reached it, cancels that prompt's turn.
#[derive(Default)]
struct Interrupts {
    /// The running turn's Ctrl-C presses; `None` between turns.
    presses: Option<InterruptPresses>,
    /// Prompts the command thread queued that the engine has not taken yet.
    queued_prompts: usize,
    /// An interrupt came for a prompt whose turn has not installed its cancellation yet
    /// ([`RunningTurn::watch`] cancels it).
    early: bool,
}

/// Marks a turn as running for the command thread until dropped.
struct RunningTurn<'a> {
    shared: &'a Shared,
    /// A queued prompt's turn ([`Self::for_prompt`]), which owns the interrupt presses.
    prompt: bool,
}

impl<'a> RunningTurn<'a> {
    /// A turn without a prompt (a sign-in): an interrupt cancels the token given to
    /// [`Self::watch`].
    fn start(shared: &'a Shared) -> Self {
        Self {
            shared,
            prompt: false,
        }
    }

    /// The turn of the queued prompt the engine just took. Its `presses` reach the
    /// turn's interrupt monitor; an interrupt that came after the prompt was queued is
    /// one of them, so it cancels this turn (and a second one still quits).
    fn for_prompt(shared: &'a Shared, presses: InterruptPresses) -> Self {
        let mut interrupts = lock(&shared.interrupts);
        interrupts.queued_prompts = interrupts.queued_prompts.saturating_sub(1);
        if interrupts.early {
            presses.press();
        }
        interrupts.presses = Some(presses);
        drop(interrupts);
        Self {
            shared,
            prompt: true,
        }
    }

    fn watch(&self, cancel: CancellationToken, hook_abort: HookAbortSignal) {
        let mut interrupts = lock(&self.shared.interrupts);
        // Pressed before the cancellation was installed: the monitor may not have seen
        // it yet, and the first request must not go out.
        if self.prompt && std::mem::take(&mut interrupts.early) {
            cancel.cancel();
            hook_abort.abort();
        }
        *lock(&self.shared.turn) = Some((cancel, hook_abort));
        drop(interrupts);
        // A shutdown that came before the turn could be reached.
        if self.shared.closing.load(Ordering::SeqCst) {
            self.shared.stop_turn();
        }
    }
}

impl Drop for RunningTurn<'_> {
    fn drop(&mut self) {
        let mut interrupts = lock(&self.shared.interrupts);
        if self.prompt {
            interrupts.presses = None;
            interrupts.early = false;
        }
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
            EngineCommand::Interrupt => shared.interrupt(),
            EngineCommand::Submit { text, attachments } => {
                if let Some(answer) = shared.take_answer() {
                    let _ = answer.send(text);
                    continue;
                }
                // Counted before the engine can take it, so an interrupt sent right
                // after it is kept for its turn.
                shared.prompt_queued();
                if queue
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
            // It only reads the catalog, so a running turn need not end first. Between
            // turns it queues, after a `SetModel` sent before it.
            EngineCommand::ListModels if shared.turn_running() && shared.answer_models() => {}
            // Both take effect at once, also while a turn runs.
            EngineCommand::Capabilities { features, client } => {
                *lock(&shared.client) = client;
                let has = |capability: &str| features.iter().any(|feature| feature == capability);
                shared
                    .structured_questions
                    .store(has(CAPABILITY_QUESTIONS), Ordering::SeqCst);
                shared
                    .report_panels
                    .store(has(CAPABILITY_REPORTS), Ordering::SeqCst);
            }
            EngineCommand::Answer { id, text } => {
                // A stale or unknown id answers nothing.
                if let Some(answer) = shared.take_answer_to(id) {
                    let _ = answer.send(text);
                }
            }
            command => {
                if queue.send(command).is_err() {
                    return;
                }
            }
        }
    }
}

/// Lets the command thread answer [`EngineCommand::ListModels`] while the engine runs.
/// Dropped when the engine stops, so the event channel can close.
struct ModelListing<'a> {
    shared: &'a Shared,
}

impl<'a> ModelListing<'a> {
    fn install(events: &Sender<UiEvent>, shared: &'a Shared) -> Self {
        *lock(&shared.events) = Some(Sender::clone(events));
        Self { shared }
    }
}

impl Drop for ModelListing<'_> {
    fn drop(&mut self) {
        lock(&self.shared.events).take();
    }
}

/// The models [`EngineCommand::ListModels`] answers with: the gateway catalog's,
/// available ones first, with their status as of now (asked for again at most every
/// 30 s); none on other providers.
fn models_event(current_model: &str) -> UiEvent {
    let models = gateway_repl::session_catalog()
        .map(|catalog| {
            crate::gateway_catalog::with_session_status(&catalog)
                .sorted()
                .into_iter()
                .map(|model| ModelInfo {
                    id: model.id.clone(),
                    display_name: model.display_name.clone(),
                    available: model.available,
                    unavailable_reason: model.unavailable_reason.clone(),
                    context_window: model.context_window,
                    tiers: model.tiers.clone(),
                    status: model.status.clone(),
                    available_again_at: model.available_again_at.clone(),
                    supports_images: model.supports_images,
                })
                .collect()
        })
        .unwrap_or_default();
    UiEvent::Models {
        current: shown_model(current_model).to_string(),
        models,
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

/// Shows `question` (a [`UiEvent::Question`] when the UI takes them, else a notice) and
/// waits for the answer: an [`EngineCommand::Answer`] with its id, or the next submitted
/// text, raw, as a user would have typed it. An interrupted turn or a closed UI gives up.
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

    let asked = if shared.structured_questions.load(Ordering::SeqCst) {
        events.send(UiEvent::Question {
            id: number,
            question: question.question.trim().to_string(),
            options: question.options.clone().unwrap_or_default(),
        })
    } else {
        events.send(UiEvent::Notice {
            message: question_notice(question),
        })
    };
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
    /// The plan and usage `/api/account` reported, shared with the threads that read it.
    account_cache: SharedAccountCache,
    /// The session this engine has open, as other Neurons' session lists see it.
    presence: Option<SessionPresence>,
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
            account_cache: SharedAccountCache::default(),
            presence: None,
            ui_closed: false,
        }
    }

    fn refresh_account(&mut self) {
        self.account = AccountState::read(self.account_email.as_deref());
    }

    /// The account as it is known now, then (with a login) again once `/api/account`
    /// answered.
    fn announce_account(&mut self) {
        self.send_account();
        self.read_account_in_background();
    }

    fn send_account(&mut self) {
        let event =
            engine_account::account_event(&self.account_cache, self.account_email.as_deref());
        self.send(event);
    }

    /// Reads `/api/account` on a thread of its own; its account event follows. False
    /// without a login.
    fn read_account_in_background(&self) -> bool {
        engine_account::refresh_in_background(
            &self.account_cache,
            &self.events,
            self.account_email.clone(),
        )
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
                        detail: None,
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
                    detail: None,
                }),
            },
            // The command thread hands interrupts to the running turn; one that gets here
            // found nothing to interrupt. Capabilities and answers are the command
            // thread's too.
            EngineCommand::Interrupt
            | EngineCommand::Capabilities { .. }
            | EngineCommand::Answer { .. } => {}
            EngineCommand::ListModels => self.list_models(),
            EngineCommand::ListSessions { scope } => {
                let event = sessions_event(&self.cli.session.id, scope);
                self.send(event);
            }
            EngineCommand::Resume { session } => match self.resume(&session) {
                Ok(()) => {
                    self.persist();
                    self.send_transcript();
                    self.send_status();
                }
                Err(error) => self.send(UiEvent::Error {
                    message: error.to_string(),
                    detail: None,
                }),
            },
            EngineCommand::TrustWorkspace => {
                for event in trust_workspace_events() {
                    self.send(event);
                }
                self.send_status();
            }
            EngineCommand::ListExtensions => self.send(extensions_event()),
            EngineCommand::RefreshAccount => {
                if !self.read_account_in_background() {
                    self.send_account();
                }
            }
            EngineCommand::TrustPreview => {
                let event =
                    engine_trust::trust_preview_event(&env::current_dir().unwrap_or_default());
                self.send(event);
            }
            EngineCommand::Shutdown => return Step::Stop,
        }
        Step::Continue
    }

    /// The running session's conversation, for the UI to show from the start.
    fn send_transcript(&mut self) {
        let event = transcript_event(&self.cli.session.id, self.cli.runtime.session());
        self.send(event);
    }

    fn submit(&mut self, text: &str, attachments: &[Attachment]) {
        // The turn is the interrupts' target from the moment the prompt is taken.
        let shared = Arc::clone(&self.shared);
        let presses = InterruptPresses::default();
        let running = RunningTurn::for_prompt(&shared, presses.clone());
        let line = text.trim();
        if line.is_empty() && attachments.is_empty() {
            drop(running);
            self.finish_turn();
            return;
        }
        self.cli.record_prompt_history(line);
        let cwd = env::current_dir().unwrap_or_default();
        let prompt = try_resolve_bare_skill_prompt(&cwd, line).unwrap_or_else(|| line.to_string());
        let prompt = self.with_attachments(prompt, attachments);
        let input = self.wrap_for_mode(&prompt);
        crate::error_detail::clear_model_call_failure();
        self.track_session();
        let client = lock(&self.shared.client).clone();
        if let Some(presence) = self.presence.as_mut() {
            presence.turn_started(client);
        }
        let failed = match self.run_turn(&input, running, presses) {
            Ok((summary, usage)) => {
                self.report_turn(&summary, usage);
                false
            }
            Err(message) => {
                self.send(UiEvent::Error {
                    message,
                    detail: crate::error_detail::take_model_call_failure(),
                });
                true
            }
        };
        if let Some(presence) = self.presence.as_mut() {
            let branch = runtime::workspace_git_status(&cwd).and_then(|status| git_branch(&status));
            presence.turn_finished(failed, branch);
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

    /// Runs one turn with UI permission prompts and UI interrupts (`presses`, the ones
    /// `running` receives), and saves the session however it ends. Returns the summary
    /// and the turn's own token usage (the summary counts the whole session).
    fn run_turn(
        &mut self,
        input: &str,
        running: RunningTurn<'_>,
        presses: InterruptPresses,
    ) -> Result<(TurnSummary, TokenUsage), String> {
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
        if let Some(notice) = repl_loop::describe_turn_outcome(summary, repl_loop::QuitHint::Hidden)
        {
            self.notice(&notice);
        }
        self.send(UiEvent::Usage {
            turn: token_counts(usage),
            cumulative: token_counts(summary.usage),
            estimated: false,
        });
        // The gateway's quota headers moved today's usage.
        if engine_account::apply_quota_headers(&self.account_cache) {
            self.send_account();
        }
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
                detail: DetailedError::detail_of(error.as_ref()),
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
            SlashCommand::Clear { confirm } => {
                self.clear(confirm)?;
                if confirm {
                    self.send_transcript();
                }
            }
            SlashCommand::Resume { session_path: None } => self.notice(&render_resume_usage()),
            SlashCommand::Resume {
                session_path: Some(reference),
            } => {
                self.resume(&reference)?;
                self.persist();
                self.send_transcript();
            }
            SlashCommand::Plan { mode } => self.plan(mode.as_deref())?,
            SlashCommand::Login => self.login()?,
            SlashCommand::Logout => self.logout(),
            SlashCommand::Usage { .. } => {
                let account = engine_account::read_now(&self.account_cache)?;
                self.send_account();
                let pricing = crate::account::pricing_url();
                let base = crate::auth::gateway_base_url()?;
                let dashboard = format!("{}/dashboard", crate::gateway_http::origin(&base));
                let now = crate::clock::now_unix();
                self.show_report(engine_reports::usage(&account, now, &pricing, &dashboard));
            }
            SlashCommand::Init => {
                let report = crate::init::initialize_repo(&env::current_dir()?)?;
                self.show_report(engine_reports::init(&report));
            }
            SlashCommand::Memory => {
                let report = engine_reports::memory(&env::current_dir()?)?;
                self.show_report(report);
            }
            SlashCommand::Doctor => {
                let report = engine_reports::doctor(&render_doctor_report()?);
                self.show_report(report);
            }
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

    /// `Ready`, then the footer's first `Status`, then the conversation of a continued
    /// chat.
    fn announce(&mut self) {
        if let Ok(cwd) = env::current_dir() {
            session_presence::record_project(&cwd);
        }
        self.send(ready_event());
        self.send_status();
        self.announce_account();
        if !self.cli.runtime.session().messages.is_empty() {
            self.send_transcript();
        }
    }

    /// Holds the session the engine runs now as open (letting go of the one before), so
    /// other Neurons' session lists show it.
    fn track_session(&mut self) {
        let id = &self.cli.session.id;
        if self
            .presence
            .as_ref()
            .is_some_and(|presence| presence.session_id() == id)
        {
            return;
        }
        self.presence = None;
        let Some(dir) = self.cli.session.path.parent() else {
            return;
        };
        let client = lock(&self.shared.client).clone();
        self.presence = Some(SessionPresence::claim(dir, id, client));
    }

    /// A report in the UI's report panel, or as a notice when it has none.
    fn show_report(&mut self, report: engine_reports::Report) {
        if self.shared.report_panels.load(Ordering::SeqCst) {
            self.send(report.into_event());
        } else {
            self.notice(&report.render_text());
        }
    }

    /// Ends a prompt or a slash command: `TurnFinished`, then the footer's new `Status`.
    fn finish_turn(&mut self) {
        self.send(UiEvent::TurnFinished);
        self.send_status();
    }

    fn send_status(&mut self) {
        lock(&self.shared.model).clone_from(&self.cli.model);
        self.track_session();
        let context_percent =
            u32::try_from(self.cli.runtime.context_snapshot().used_percent()).unwrap_or(u32::MAX);
        // A session on the gateway needs the login; other providers do not.
        let sign_in_required = !self.account.signed_in
            && active_provider().is_some_and(|active| active.id == ProviderId::ZeroX);
        let status = status_event(
            &self.cli.model,
            self.cli.permission_mode,
            &self.cli.session.id,
            self.cli.plan_mode,
            context_percent,
            &self.account,
            sign_in_required,
            self.cli.runtime.served_model(),
        );
        self.send(status);
    }

    /// `/login`: the device sign-in. A session on the Zero-X gateway switches to the new
    /// login at once (as the line REPL's `/login` does).
    fn login(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (credential, email) = sign_in(&self.events, &self.shared)?;
        self.account_email = email;
        self.refresh_account();
        engine_account::forget(&self.account_cache);
        self.announce_account();
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
        engine_account::forget(&self.account_cache);
        self.send_account();
        if active_provider().is_some_and(|active| active.id == ProviderId::ZeroX) {
            self.notice("Requests in this session will fail until you run /login again.");
        }
    }

    /// The gateway catalog's models ([`models_event`]). During a turn the command thread
    /// answers instead.
    fn list_models(&mut self) {
        let models = models_event(&self.cli.model);
        self.send(models);
    }

    fn model_report(&mut self) {
        let mut report = format_model_report(
            &self.cli.model,
            self.cli.runtime.session().messages.len(),
            prompt_count(self.cli.runtime.session()),
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
        let report = self.load_session(reference)?;
        self.notice(&report);
        Ok(())
    }

    /// Switches to saved session `reference`; returns the "Session resumed" report.
    fn load_session(&mut self, reference: &str) -> Result<String, Box<dyn std::error::Error>> {
        let (handle, session) = load_session_reference(reference)?;
        let message_count = session.messages.len();
        let session_id = session.session_id.clone();
        self.rebuild_runtime(session, &handle.id)?;
        self.cli.session = SessionHandle {
            id: session_id,
            path: handle.path,
        };
        Ok(format_resume_report(
            &self.cli.session.path.display().to_string(),
            message_count,
            prompt_count(self.cli.runtime.session()),
        ))
    }

    /// Continues a saved session at startup, removing the empty one the fresh start made.
    /// Returns the "Session resumed" report: at startup it can only be shown after `Ready`.
    fn continue_saved_session(
        &mut self,
        reference: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let placeholder = self.cli.session.path.clone();
        let placeholder_is_empty = self.cli.runtime.session().messages.is_empty();
        let report = self.load_session(reference)?;
        if placeholder_is_empty && placeholder != self.cli.session.path {
            let _ = fs::remove_file(&placeholder);
        }
        Ok(report)
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
        definitions, forward_commands, git_status, mcp_server_status, plain_notice,
        question_notice, shorten, shorten_json_strings, spawn, transcript_event, EngineExit,
        EngineHandle, EngineOptions, McpRuntimeView, RunningTurn, Shared,
    };
    use crate::repl_loop::InterruptPresses;
    use crate::tests::{env_lock, with_current_dir};
    use mock_anthropic_service::{MockAnthropicService, SCENARIO_PREFIX};
    use neuron_tui::protocol::{
        EngineCommand, PermissionDecision, PermissionReply, SessionRunStatus, SessionScope,
        TokenCounts, TranscriptEntry, UiEvent, CAPABILITY_QUESTIONS, CAPABILITY_REPORTS,
        ENGINE_FEATURES, PROTOCOL_VERSION,
    };
    use runtime::ui_hooks::{self, UserQuestion};
    use runtime::PermissionMode;
    use runtime::{CancellationToken, HookAbortSignal};
    use runtime::{ContentBlock, ConversationMessage, MessageRole, Session};
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{self, Sender};
    use std::sync::mpsc::{Receiver, RecvTimeoutError};
    use std::sync::Arc;
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
                UiEvent::Notice { .. } | UiEvent::Status { .. } | UiEvent::Account { .. } => {}
                other => panic!("expected a notice with {needle:?}, got {other:?}"),
            }
        }
    }

    /// The events without notices, footer updates and account updates: the hooks are
    /// process-wide, so a test running alongside may add notices of its own.
    fn without_notices(events: &[UiEvent]) -> Vec<UiEvent> {
        events
            .iter()
            .filter(|event| {
                !matches!(
                    event,
                    UiEvent::Notice { .. } | UiEvent::Status { .. } | UiEvent::Account { .. }
                )
            })
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
        match without_notices(&cleared).as_slice() {
            [UiEvent::Transcript {
                entries, truncated, ..
            }, UiEvent::TurnFinished] => {
                assert!(entries.is_empty() && !truncated, "the new chat is empty");
            }
            other => panic!("expected an empty transcript, then the end: {other:?}"),
        }
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

        with_current_dir(&workspace, || {
            drive_engine(&workspace);
            resume_engine();
        });

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

        // First the protocol version and the engine's features, then the footer.
        assert_eq!(
            next_event(&engine.events),
            UiEvent::Ready {
                protocol: PROTOCOL_VERSION,
                version: env!("CARGO_PKG_VERSION").to_string(),
                features: ENGINE_FEATURES.iter().map(ToString::to_string).collect(),
            }
        );
        let untracked_at_start = match next_event(&engine.events) {
            UiEvent::Status {
                model,
                permission_mode,
                plan_mode: false,
                git: Some(git),
                signed_in: false,
                sign_in_required: false,
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
        // Then the account: there is no Zero-X login here, so nothing is read.
        assert_eq!(
            next_event(&engine.events),
            UiEvent::Account {
                signed_in: false,
                account: None,
            }
        );

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

        // With the `questions` capability: a Question event, answered by its id. The
        // session list that follows is queued behind the capability, so it is in effect.
        engine
            .commands
            .send(EngineCommand::Capabilities {
                features: vec![CAPABILITY_QUESTIONS.into()],
                client: None,
            })
            .unwrap();
        engine
            .commands
            .send(EngineCommand::ListSessions {
                scope: SessionScope::Project,
            })
            .unwrap();
        events_until(&engine.events, |event| {
            matches!(event, UiEvent::Sessions { .. })
        });
        let ask = ui_hooks::ask_handler().expect("the engine answers questions");
        let asker = thread::spawn(move || {
            ask(&UserQuestion {
                question: "Which crate?".into(),
                options: Some(vec!["api".into(), "tools".into()]),
            })
        });
        let id = match next_event(&engine.events) {
            UiEvent::Question {
                id,
                question,
                options,
            } => {
                assert_eq!(question, "Which crate?");
                assert_eq!(options, vec!["api".to_string(), "tools".to_string()]);
                id
            }
            other => panic!("expected a question event, got {other:?}"),
        };
        // A stale id answers nothing; the right one does.
        engine
            .commands
            .send(EngineCommand::Answer {
                id: id + 7,
                text: "api".into(),
            })
            .unwrap();
        engine
            .commands
            .send(EngineCommand::Answer {
                id,
                text: "tools".into(),
            })
            .unwrap();
        assert_eq!(asker.join().unwrap().expect("answered"), "tools");
        engine
            .commands
            .send(EngineCommand::Capabilities {
                features: Vec::new(),
                client: None,
            })
            .unwrap();

        // Nothing runs: the interrupt is dropped, not kept to cancel the next turn.
        engine.commands.send(EngineCommand::Interrupt).unwrap();

        // Streamed text, then the turn's real usage.
        engine.commands.send(scenario("streaming_text")).unwrap();
        let streamed = turn_events(&engine.events);
        let first_session = match streamed.last() {
            Some(UiEvent::Status { session_id, .. }) => session_id.clone(),
            other => panic!("expected the footer after the turn, got {other:?}"),
        };
        assert_eq!(
            without_notices(&streamed),
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

        // The session list has this chat, current, titled by its first prompt.
        engine
            .commands
            .send(EngineCommand::ListSessions {
                scope: SessionScope::Project,
            })
            .unwrap();
        match events_until(&engine.events, |event| {
            matches!(event, UiEvent::Sessions { .. })
        })
        .pop()
        {
            Some(UiEvent::Sessions {
                current, sessions, ..
            }) => {
                assert_eq!(current, first_session);
                let listed = sessions
                    .iter()
                    .find(|session| session.id == first_session)
                    .unwrap_or_else(|| panic!("the chat is listed: {sessions:?}"));
                assert!(listed.current);
                assert_eq!(
                    listed.title.as_deref(),
                    Some("PARITY_SCENARIO:streaming_text")
                );
                assert!(listed.message_count >= 2, "{listed:?}");
                // Where it belongs and how its turn ended; this engine's own session is
                // not "open elsewhere".
                // The engine reports its working directory, which is canonical on macOS
                // (`/private/var/...` for the test's `/var/...`).
                assert_eq!(
                    listed
                        .project
                        .as_deref()
                        .map(|project| fs::canonicalize(project).ok()),
                    Some(fs::canonicalize(workspace).ok())
                );
                assert_eq!(listed.run_status, SessionRunStatus::Idle);
                assert!(!listed.failed);
                assert_eq!(listed.git_branch.as_deref(), Some("main"));
            }
            other => panic!("expected the session list, got {other:?}"),
        }
        // Every project's chats: this workspace was recorded when the engine started.
        engine
            .commands
            .send(EngineCommand::ListSessions {
                scope: SessionScope::All,
            })
            .unwrap();
        match events_until(&engine.events, |event| {
            matches!(event, UiEvent::Sessions { .. })
        })
        .pop()
        {
            Some(UiEvent::Sessions {
                scope: SessionScope::All,
                sessions,
                ..
            }) => assert!(
                sessions.iter().any(|session| session.id == first_session),
                "{sessions:?}"
            ),
            other => panic!("expected every project's sessions, got {other:?}"),
        }

        // Interrupted at once, before the engine reached the prompt: nothing is
        // streamed and the turn ends as interrupted.
        engine.commands.send(scenario("streaming_text")).unwrap();
        engine.commands.send(EngineCommand::Interrupt).unwrap();
        let interrupted = turn_events(&engine.events);
        assert!(
            notices(&interrupted)
                .iter()
                .any(|notice| notice.starts_with("Interrupted.")),
            "{interrupted:?}"
        );
        assert!(
            matches!(
                without_notices(&interrupted).as_slice(),
                [UiEvent::Usage { turn, .. }, UiEvent::TurnFinished] if *turn == counts(0, 0)
            ),
            "{interrupted:?}"
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
            turn_notices.contains(&"\u{25a0} Interrupting\u{2026}"),
            "{turn:?}"
        );
        assert!(
            turn_notices
                .iter()
                .any(|notice| notice.starts_with("Interrupted.")),
            "{turn:?}"
        );
        // The UI shows its own key hints; the line REPL's would be wrong there.
        assert!(
            turn_notices
                .iter()
                .all(|notice| !notice.contains("Ctrl-C again")),
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
                    message: "/vim is not available in the full-screen UI yet.".into(),
                    detail: None
                },
                UiEvent::TurnFinished,
            ]
        );

        // Reports: a notice until the UI shows report panels, then a report event.
        let slash = |command: &str| EngineCommand::Slash {
            command: command.into(),
            args: String::new(),
        };
        engine.commands.send(slash("memory")).unwrap();
        let memory = turn_events(&engine.events);
        assert_eq!(without_notices(&memory), vec![UiEvent::TurnFinished]);
        assert!(
            notices(&memory)
                .iter()
                .any(|notice| notice.starts_with("Instruction files")),
            "{memory:?}"
        );
        engine
            .commands
            .send(EngineCommand::Capabilities {
                features: vec![CAPABILITY_QUESTIONS.into(), CAPABILITY_REPORTS.into()],
                client: Some("terminal".into()),
            })
            .unwrap();
        for (command, title) in [
            ("memory", "Instruction files"),
            ("doctor", "Doctor"),
            ("init", "Project instructions"),
        ] {
            engine.commands.send(slash(command)).unwrap();
            let events = without_notices(&turn_events(&engine.events));
            match events.as_slice() {
                [UiEvent::Report {
                    command: sent,
                    title: sent_title,
                    sections,
                    ..
                }, UiEvent::TurnFinished] => {
                    assert_eq!((sent.as_str(), sent_title.as_str()), (command, title));
                    assert!(!sections.is_empty(), "{events:?}");
                }
                other => panic!("/{command}: a report, got {other:?}"),
            }
        }
        assert!(
            workspace.join("NEURON.md").is_file(),
            "/init wrote NEURON.md"
        );
        // /usage needs a Zero-X login, and says so in a way a UI can act on.
        engine.commands.send(slash("usage")).unwrap();
        match without_notices(&turn_events(&engine.events)).as_slice() {
            [UiEvent::Error {
                detail: Some(detail),
                ..
            }, UiEvent::TurnFinished] => assert_eq!(detail.code, "sign_in_required"),
            other => panic!("/usage signed out: an error with a detail, got {other:?}"),
        }
        engine
            .commands
            .send(EngineCommand::Capabilities {
                features: Vec::new(),
                client: None,
            })
            .unwrap();

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

        // Resume the first chat: its conversation for the UI, then the footer on it.
        engine
            .commands
            .send(EngineCommand::Resume {
                session: first_session.clone(),
            })
            .unwrap();
        let resumed = events_until(&engine.events, |event| {
            matches!(event, UiEvent::Status { .. })
        });
        match without_notices(&resumed).as_slice() {
            [UiEvent::Transcript {
                session_id,
                entries,
                truncated: false,
            }] => {
                assert_eq!(session_id, &first_session);
                assert_eq!(
                    entries.first(),
                    Some(&TranscriptEntry::User {
                        text: "PARITY_SCENARIO:streaming_text".into()
                    })
                );
                assert!(entries.contains(&TranscriptEntry::Assistant {
                    text: "Mock streaming says hello from the parity harness.".into()
                }));
                assert!(
                    entries.iter().any(|entry| matches!(
                        entry,
                        TranscriptEntry::ToolCall { name, .. } if name == "bash"
                    )),
                    "{entries:?}"
                );
                assert!(
                    entries.iter().any(|entry| matches!(
                        entry,
                        TranscriptEntry::ToolResult { output, is_error: false, .. }
                            if output.contains("approved via prompt")
                    )),
                    "{entries:?}"
                );
            }
            other => panic!("expected the transcript, got {other:?}"),
        }
        assert!(
            matches!(resumed.last(), Some(UiEvent::Status { session_id, .. }) if *session_id == first_session)
        );
        engine
            .commands
            .send(EngineCommand::Resume {
                session: "no-such-chat".into(),
            })
            .unwrap();
        assert!(matches!(
            events_until(&engine.events, |event| matches!(
                event,
                UiEvent::Error { .. }
            ))
            .pop(),
            Some(UiEvent::Error { .. })
        ));

        // Before trusting it: what the workspace would turn on, and where trust would go
        // (the repository's root). Nothing in it starts.
        let settings = workspace.join(".neuron").join("settings.json");
        fs::create_dir_all(settings.parent().unwrap()).expect("settings dir");
        fs::write(
            &settings,
            r#"{"permissions":{"allow":["bash(cargo test)"]},"mcpServers":{"tools":{"command":"uvx"}}}"#,
        )
        .expect("workspace settings");
        engine.commands.send(EngineCommand::TrustPreview).unwrap();
        match events_until(&engine.events, |event| {
            matches!(event, UiEvent::TrustPreview { .. })
        })
        .pop()
        {
            Some(UiEvent::TrustPreview {
                root,
                is_git_root: true,
                session_only: false,
                trusted: false,
                mcp_servers,
                permission_rules,
                ..
            }) => {
                assert_eq!(
                    fs::canonicalize(&root).ok(),
                    fs::canonicalize(workspace).ok()
                );
                assert_eq!(mcp_servers[0].name, "tools");
                assert_eq!(permission_rules[0].rule, "bash(cargo test)");
            }
            other => panic!("expected the trust preview, got {other:?}"),
        }
        fs::remove_file(&settings).expect("settings removed");

        // Trust the workspace: recorded, and the footer says so; settings load on restart.
        engine.commands.send(EngineCommand::TrustWorkspace).unwrap();
        let trusted = events_until(&engine.events, |event| {
            matches!(event, UiEvent::Status { .. })
        });
        let workspace_root = fs::canonicalize(workspace).ok();
        assert!(
            trusted.iter().any(|event| matches!(
                event,
                UiEvent::WorkspaceTrusted {
                    restart_required: true,
                    root: Some(root),
                    session_only: false,
                    ..
                } if fs::canonicalize(root).ok() == workspace_root
            )),
            "{trusted:?}"
        );
        assert!(
            matches!(
                trusted.last(),
                Some(UiEvent::Status {
                    workspace_trusted: true,
                    ..
                })
            ),
            "{trusted:?}"
        );

        // The extensions: no MCP servers in this home, and the workspace's own skill.
        let skill = workspace.join(".claw").join("skills").join("release-notes");
        fs::create_dir_all(&skill).expect("skill dir");
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: release-notes\ndescription: Drafts release notes\n---\nWrite them.\n",
        )
        .expect("skill");
        engine.commands.send(EngineCommand::ListExtensions).unwrap();
        match next_event(&engine.events) {
            UiEvent::Extensions {
                mcp_servers,
                skills,
                ..
            } => {
                assert!(mcp_servers.is_empty(), "{mcp_servers:?}");
                assert!(
                    skills.iter().any(|skill| skill.name == "release-notes"
                        && skill.description.as_deref() == Some("Drafts release notes")
                        && skill.active),
                    "{skills:?}"
                );
            }
            other => panic!("expected the extensions, got {other:?}"),
        }

        // Shutdown while a turn waits for approval: the turn stops, the session is saved,
        // the hooks go back to the terminal and the event channel closes.
        clear_session(&engine);
        submit_bash_needing_approval(&engine, "bash_permission_prompt_approved");
        // The model list does not wait for the turn.
        engine.commands.send(EngineCommand::ListModels).unwrap();
        let listed = events_until(&engine.events, |event| {
            matches!(event, UiEvent::Models { .. })
        });
        assert!(
            !listed.contains(&UiEvent::TurnFinished),
            "answered during the turn: {listed:?}"
        );
        match listed.last() {
            Some(UiEvent::Models { current, .. }) => {
                assert!(current.contains("claude-opus"), "{current}");
            }
            other => panic!("expected the model list, got {other:?}"),
        }
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

    /// `--resume`: `Ready` is still the first event, then the footer, then the report on
    /// the session it continues.
    fn resume_engine() {
        let engine = spawn(EngineOptions {
            model: "claude-sonnet-4-6".to_string(),
            allowed_tools: None,
            permission_mode: PermissionMode::WorkspaceWrite,
            reasoning_effort: None,
            resume: Some("latest".to_string()),
            signed_out: None,
        })
        .expect("the engine continues the saved session");
        let first = next_event(&engine.events);
        assert!(matches!(first, UiEvent::Ready { .. }), "{first:?}");
        let second = next_event(&engine.events);
        assert!(matches!(second, UiEvent::Status { .. }), "{second:?}");
        expect_notice(&engine.events, "Session resumed");
        assert_eq!(engine.finish(), EngineExit::Shutdown);
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
    fn mcp_servers_report_what_they_are_doing() {
        let view = |starting: bool| McpRuntimeView {
            starting,
            running: vec!["docs".into(), "search".into(), "broken".into()],
            pending: vec!["search".into()],
            failed: vec![("broken".into(), "spawn failed".into())],
            unsupported: vec![("remote".into(), "ws is not supported".into())],
            tools: vec![
                "mcp__docs__lookup".into(),
                "mcp__docs__index".into(),
                "mcp__search__query".into(),
            ],
        };
        let live = view(false);
        assert_eq!(
            mcp_server_status("docs", Some(&live)),
            (
                "ready",
                vec!["lookup".to_string(), "index".to_string()],
                None
            )
        );
        assert_eq!(
            mcp_server_status("search", Some(&live)),
            ("pending", Vec::new(), None)
        );
        assert_eq!(
            mcp_server_status("broken", Some(&live)),
            ("failed", Vec::new(), Some("spawn failed".to_string()))
        );
        assert_eq!(
            mcp_server_status("remote", Some(&live)),
            (
                "unsupported",
                Vec::new(),
                Some("ws is not supported".to_string())
            )
        );
        assert_eq!(
            mcp_server_status("other", Some(&live)),
            ("not_started", Vec::new(), None)
        );
        assert_eq!(
            mcp_server_status("docs", Some(&view(true))),
            ("starting", Vec::new(), None)
        );
        assert_eq!(
            mcp_server_status("docs", None),
            ("not_started", Vec::new(), None)
        );
    }

    #[test]
    fn definitions_read_the_skills_report() {
        let report = serde_json::json!({
            "skills": [
                {"name": "plan", "description": "Plans work", "source": {"id": "project", "label": "Project"}, "active": true},
                {"name": "plan", "description": null, "source": {"id": "user", "label": "User"}, "active": false},
                {"description": "no name"}
            ]
        });
        let skills = definitions(Some(report), "skills");
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].source, "Project");
        assert_eq!(skills[0].description.as_deref(), Some("Plans work"));
        assert!(!skills[1].active);
        assert_eq!(skills[1].description, None);
        assert_eq!(definitions(None, "skills"), Vec::new());
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

    /// A saved conversation as a UI rebuilds it: prompts, replies, calls and results in
    /// order, system messages left out, long texts shortened with JSON inputs still JSON.
    #[test]
    fn transcripts_carry_the_conversation_a_ui_shows() {
        let mut session = Session::new();
        session.messages = vec![
            ConversationMessage {
                role: MessageRole::System,
                blocks: vec![ContentBlock::Text {
                    text: "Earlier messages were summarized.".into(),
                }],
                usage: None,
            },
            ConversationMessage::user_text("write the file"),
            ConversationMessage::assistant(vec![
                ContentBlock::Text {
                    text: "Writing it.".into(),
                },
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "write_file".into(),
                    input: serde_json::json!({ "path": "a.txt", "content": "x".repeat(5_000) })
                        .to_string(),
                },
            ]),
            ConversationMessage::tool_result("t1", "write_file", "y".repeat(5_000), false),
        ];
        let UiEvent::Transcript {
            session_id,
            entries,
            truncated,
        } = transcript_event("s1", &session)
        else {
            panic!("a transcript");
        };
        assert_eq!((session_id.as_str(), truncated), ("s1", false));
        assert_eq!(entries.len(), 4);
        assert_eq!(
            entries[..2],
            [
                TranscriptEntry::User {
                    text: "write the file".into()
                },
                TranscriptEntry::Assistant {
                    text: "Writing it.".into()
                },
            ]
        );
        let TranscriptEntry::ToolCall { input, .. } = &entries[2] else {
            panic!("a tool call: {:?}", entries[2]);
        };
        let input: serde_json::Value = serde_json::from_str(input).expect("still JSON");
        assert_eq!(input["path"], "a.txt");
        assert!(input["content"]
            .as_str()
            .unwrap()
            .ends_with("(1000 more characters)"));
        let TranscriptEntry::ToolResult { output, .. } = &entries[3] else {
            panic!("a tool result: {:?}", entries[3]);
        };
        assert!(output.ends_with("(1000 more characters)"));

        assert_eq!(shorten("short", 10), "short");
        assert_eq!(
            shorten_json_strings("not json", 3),
            "not\n\u{2026} (5 more characters)"
        );
    }

    #[test]
    fn notices_and_questions_read_as_plain_text() {
        assert_eq!(
            plain_notice("\n\x1b[33m\u{25a0}\x1b[0m Interrupting\u{2026}"),
            Some("\u{25a0} Interrupting\u{2026}".to_string())
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

    /// The command thread and the engine thread's side of a turn, without a session.
    struct Commands {
        shared: Arc<Shared>,
        sender: Sender<EngineCommand>,
        queue: Receiver<EngineCommand>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl Commands {
        fn start() -> Self {
            let shared = Arc::new(Shared::default());
            let (sender, commands_rx) = mpsc::channel();
            let (queue_tx, queue) = mpsc::channel();
            let forwarding = Arc::clone(&shared);
            let thread =
                thread::spawn(move || forward_commands(&commands_rx, &queue_tx, &forwarding));
            Self {
                shared,
                sender,
                queue,
                thread: Some(thread),
            }
        }

        /// Sends `commands`, then waits until the command thread handled them all.
        fn send(&self, commands: Vec<EngineCommand>) {
            for command in commands {
                self.sender.send(command).unwrap();
            }
            let marker = EngineCommand::SetModel {
                model: "marker".into(),
            };
            self.sender.send(marker.clone()).unwrap();
            while self.queue.recv_timeout(EVENT_TIMEOUT).expect("queued") != marker {}
        }

        /// Takes a queued prompt as the engine does and installs its cancellation;
        /// returns the turn, its token and whether a press is waiting for its monitor.
        fn take_prompt(&self) -> (RunningTurn<'_>, CancellationToken, bool) {
            let presses = InterruptPresses::default();
            let running = RunningTurn::for_prompt(&self.shared, presses.clone());
            let cancel = CancellationToken::new();
            running.watch(cancel.clone(), HookAbortSignal::new());
            let pressed = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime")
                .block_on(async {
                    tokio::time::timeout(Duration::from_millis(50), presses.next_press())
                        .await
                        .is_ok()
                });
            (running, cancel, pressed)
        }
    }

    impl Drop for Commands {
        fn drop(&mut self) {
            let _ = self.sender.send(EngineCommand::Shutdown);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn prompt() -> EngineCommand {
        EngineCommand::Submit {
            text: "write it in Rust".into(),
            attachments: Vec::new(),
        }
    }

    /// An interrupt that came within ~0.1 s of the submit, before the engine reached the
    /// prompt, used to be dropped and the turn ran to completion.
    #[test]
    fn an_interrupt_right_after_a_submit_cancels_that_turn() {
        let commands = Commands::start();

        // Before the engine took the prompt.
        commands.send(vec![prompt(), EngineCommand::Interrupt]);
        let (running, cancel, pressed) = commands.take_prompt();
        assert!(cancel.is_cancelled(), "the turn starts cancelled");
        assert!(pressed, "the turn's monitor hears the press");
        drop(running);

        // After the engine took it, before its cancellation was installed.
        commands.send(vec![prompt()]);
        let presses = InterruptPresses::default();
        let running = RunningTurn::for_prompt(&commands.shared, presses);
        commands.send(vec![EngineCommand::Interrupt]);
        let cancel = CancellationToken::new();
        running.watch(cancel.clone(), HookAbortSignal::new());
        assert!(cancel.is_cancelled());
        drop(running);

        // While it runs: the token is cancelled at once, not when the monitor gets to it.
        commands.send(vec![prompt()]);
        let (running, cancel, _) = commands.take_prompt();
        assert!(!cancel.is_cancelled());
        commands.send(vec![EngineCommand::Interrupt]);
        assert!(cancel.is_cancelled());
        drop(running);

        // With nothing running or queued, an interrupt is dropped: the next turn runs.
        commands.send(vec![EngineCommand::Interrupt, prompt()]);
        let (running, cancel, pressed) = commands.take_prompt();
        assert!(!cancel.is_cancelled());
        assert!(!pressed);
        drop(running);

        // A prompt the engine refuses (signed out) spends the interrupt meant for it.
        commands.send(vec![prompt(), EngineCommand::Interrupt]);
        commands.shared.prompt_dropped();
        commands.send(vec![prompt()]);
        let (running, cancel, pressed) = commands.take_prompt();
        assert!(!cancel.is_cancelled());
        assert!(!pressed);
        drop(running);
    }
}
