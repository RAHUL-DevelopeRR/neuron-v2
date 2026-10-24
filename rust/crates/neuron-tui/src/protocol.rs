//! Messages between the full-screen UI and the agent engine.
//!
//! The engine runs a turn on its own thread and reports everything the user should see as
//! [`UiEvent`]s; the UI sends [`EngineCommand`]s and answers permission requests with
//! [`PermissionReply`]. Everything is plain data (serde-serialisable): `neuron engine
//! --stdio` speaks the same contract as JSON lines, [`ClientMessage`]s in and [`UiEvent`]s
//! out, starting with [`UiEvent::Ready`].
//!
//! Additions after protocol 1 shipped are listed in [`UiEvent::Ready`]'s `features` (see
//! [`ENGINE_FEATURES`]); fields they add read as their defaults from older engines.
//! "Protocol v2" is the set of features the account, session-list, report and trust
//! screens need: [`FEATURE_REPORTS`], [`FEATURE_ACCOUNT`], [`FEATURE_ERROR_DETAILS`],
//! [`FEATURE_SESSION_SCOPE`], [`FEATURE_TRUST_PREVIEW`] and [`FEATURE_MODEL_IMAGES`].

use serde::{Deserialize, Serialize};

/// The contract's version, sent in [`UiEvent::Ready`]. Bumped when a change would break a
/// UI written against the previous version (new event kinds do not count: a UI ignores
/// events it does not know).
pub const PROTOCOL_VERSION: u32 = 1;

/// Token counts as the provider reported them (or estimated before it did).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// What a tool call needs approval for, as the permission dialog shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionView {
    pub tool: String,
    pub current_mode: String,
    pub required_mode: String,
    pub reason: Option<String>,
    /// The tool input as JSON text (commands, paths, diffs are inside it).
    pub input: String,
    /// What "allow for this session" would approve from now on (command prefixes such as
    /// `cargo test`, or a web domain). Empty when only a one-time approval is possible.
    pub session_prefixes: Vec<String>,
}

/// The workspace's git state, for the footer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStatus {
    /// The branch, `detached HEAD`, or `None` when git could not tell.
    pub branch: Option<String>,
    /// Changed paths in all: staged, unstaged, untracked and conflicted ones.
    pub changed: usize,
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub conflicted: usize,
}

/// One model the user can switch to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: Option<String>,
    /// Whether the user's plan can use it now; `unavailable_reason` says why not.
    pub available: bool,
    pub unavailable_reason: Option<String>,
    pub context_window: Option<u32>,
    /// Plan tiers that include it (`free`, `pro`, ...).
    pub tiers: Vec<String>,
    /// The gateway's view now: `available`, `degraded`, `busy` or `blurred`
    /// (not usable until `available_again_at`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// When a `blurred` model is back (ISO 8601).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_again_at: Option<String>,
    /// The catalog says the model reads images ([`FEATURE_MODEL_IMAGES`]); `None` when
    /// it does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_images: Option<bool>,
}

/// The Zero-X login, as the UI may show it. Never carries the token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountInfo {
    /// Known right after a sign-in in this engine; not stored with the login.
    pub email: Option<String>,
    pub plan: Option<String>,
    /// Where the login comes from: `NEURON_TOKEN`, or the stored login and where it is kept.
    pub source: String,
    /// When the session expires (Unix seconds), when the gateway said.
    pub expires_at: Option<u64>,
}

/// What this engine can do beyond protocol 1, in [`UiEvent::Ready`]. A UI sends the
/// commands a feature adds only when the engine lists it: an older engine would skip
/// them with a notice.
pub const ENGINE_FEATURES: &[&str] = &[
    // `capabilities` (and with `questions` on, `question` events and `answer`).
    FEATURE_CAPABILITIES,
    // `list_sessions`, `resume`, and `transcript` events.
    "sessions",
    // `trust_workspace` and `workspace_trusted`.
    "trust_workspace",
    // `todos`, `subagent_started` and `subagent_finished` events.
    "todos",
    "subagents",
    // `list_extensions` and `extensions`.
    "extensions",
    FEATURE_ERROR_DETAILS,
    FEATURE_ACCOUNT,
    FEATURE_REPORTS,
    FEATURE_SESSION_SCOPE,
    FEATURE_TRUST_PREVIEW,
    FEATURE_MODEL_IMAGES,
];

/// `report` events for the commands that show a report (`/usage`, `/init`, `/memory`,
/// `/doctor`), when the UI takes them ([`CAPABILITY_REPORTS`]).
pub const FEATURE_REPORTS: &str = "reports";
/// `account` events and [`EngineCommand::RefreshAccount`].
pub const FEATURE_ACCOUNT: &str = "account";
/// [`ErrorDetail`] on `error` events: what a UI needs to offer a way out (sign in again,
/// upgrade, wait).
pub const FEATURE_ERROR_DETAILS: &str = "error_details";
/// [`EngineCommand::ListSessions`] takes a [`SessionScope`], and [`SessionSummary`] says
/// where each session belongs and whether it runs elsewhere.
pub const FEATURE_SESSION_SCOPE: &str = "session_scope";
/// [`EngineCommand::TrustPreview`] and the `trust_preview` event.
pub const FEATURE_TRUST_PREVIEW: &str = "trust_preview";
/// `supports_images` on models and on `status`.
pub const FEATURE_MODEL_IMAGES: &str = "model_images";

/// One configured MCP server and what it is doing now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerInfo {
    pub name: String,
    /// Where it is configured: `bundled`, `user`, `project` or `local`.
    pub scope: String,
    /// `stdio`, `http`, `sse`, `ws`, `sdk` or `managed_proxy`.
    pub transport: String,
    /// `starting`, `ready`, `failed`, `pending` (started, tools not found),
    /// `unsupported` (a transport Neuron does not speak yet) or `not_started`.
    pub status: String,
    /// Its tools, without the `mcp__<server>__` prefix.
    pub tools: Vec<String>,
    /// Why it failed or is unsupported.
    pub error: Option<String>,
}

/// One installed plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    /// `builtin`, `bundled` or `external`.
    pub kind: String,
    pub enabled: bool,
}

/// One skill the agent can load (its `Skill` tool), or one agent definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionInfo {
    pub name: String,
    pub description: Option<String>,
    /// Where it comes from (`project`, `user`, `bundled`, ...), as people read it.
    pub source: String,
    /// False when another definition of the same name wins.
    pub active: bool,
}

/// The feature that adds [`EngineCommand::Capabilities`].
pub const FEATURE_CAPABILITIES: &str = "capabilities";

/// A UI capability ([`EngineCommand::Capabilities`]): questions from the agent come as
/// [`UiEvent::Question`] instead of a notice starting with "Question:".
pub const CAPABILITY_QUESTIONS: &str = "questions";

/// A UI capability: reports come as [`UiEvent::Report`] instead of a notice.
pub const CAPABILITY_REPORTS: &str = "reports";

/// Which saved chats [`EngineCommand::ListSessions`] lists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionScope {
    /// This workspace's.
    #[default]
    Project,
    /// Every workspace's Neuron has run in on this device, newest first.
    All,
}

/// What a saved chat is doing now, as far as this device can tell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRunStatus {
    /// No Neuron has it open.
    #[default]
    Idle,
    /// Another Neuron (a terminal UI, the desktop app) has it open and is waiting for a
    /// prompt.
    Open,
    /// Another Neuron is running a turn in it.
    Running,
}

/// One saved chat, for a session list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    /// The first prompt, on one line and shortened.
    pub title: Option<String>,
    pub model: Option<String>,
    pub message_count: usize,
    /// When it last changed (Unix milliseconds).
    pub updated_at_ms: u64,
    /// The session the engine runs now.
    pub current: bool,
    /// The branch name of a forked session.
    pub branch: Option<String>,
    /// The workspace folder it belongs to ([`FEATURE_SESSION_SCOPE`]).
    #[serde(default)]
    pub project: Option<String>,
    /// The git branch when it last ran, when Neuron saw one.
    #[serde(default)]
    pub git_branch: Option<String>,
    /// Whether another Neuron has it open or runs a turn in it. The current session is
    /// `idle` here: this engine's own state is in `status`.
    #[serde(default)]
    pub run_status: SessionRunStatus,
    /// Its last turn failed.
    #[serde(default)]
    pub failed: bool,
    /// The Neuron that last ran it, as its UI named itself (`terminal`, `desktop`), when
    /// known.
    #[serde(default)]
    pub client: Option<String>,
}

/// One part of a [`UiEvent::Report`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportSection {
    pub heading: Option<String>,
    /// GitHub-flavoured Markdown.
    pub markdown: String,
}

/// A link a report offers (the usage dashboard, the plans page).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportLink {
    pub label: String,
    pub url: String,
}

/// Requests and tokens: a plan's daily limits, or what was used today.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageAmounts {
    pub requests: Option<u64>,
    pub tokens: Option<u64>,
}

/// The Zero-X account as the account row, the account menu and the usage panel show it
/// ([`UiEvent::Account`]). Never carries the token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountDetails {
    pub email: Option<String>,
    /// The display name the account gave.
    pub name: Option<String>,
    /// The plan's id: `free`, `pro`, `max`, ...
    pub plan: Option<String>,
    /// The plan as people read it ("Pro").
    pub plan_name: Option<String>,
    /// Where the login comes from: `NEURON_TOKEN`, or the stored login and where it is kept.
    pub source: String,
    /// When the session expires (Unix seconds), when the gateway said.
    pub expires_at: Option<u64>,
    /// The plan's daily limits.
    pub limits: Option<UsageAmounts>,
    /// What was used today, from the account or the last reply's quota headers.
    pub usage_today: Option<UsageAmounts>,
    /// When today's usage resets (ISO 8601, UTC).
    pub resets_at: Option<String>,
    /// Prepaid credit, in US cents.
    pub credit_balance_cents: Option<u64>,
    pub suspended: bool,
    /// The account dashboard and the plans page, to open in a browser.
    pub dashboard_url: String,
    pub pricing_url: String,
    /// When the plan and usage were last read (Unix seconds); `None` when only the stored
    /// login is known so far.
    pub usage_updated_at: Option<u64>,
    /// Why the plan and usage could not be read, when they could not.
    pub usage_error: Option<String>,
}

/// What a failed request needs from the user, for a UI to offer the way out: sign in
/// again, upgrade, wait for a reset, or retry later ([`FEATURE_ERROR_DETAILS`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorDetail {
    /// The gateway's code (`invalid_session`, `upgrade_required`, `insufficient_quota`,
    /// `insufficient_credits`, `rate_limited`, `capacity_exhausted`, `account_suspended`,
    /// `context_length_exceeded`, ...), `unreachable` when the server could not be
    /// reached, `not_available` when it does not offer the call yet (a 404), or
    /// `sign_in_required` when the call needs a Zero-X login and there is none.
    pub code: String,
    /// The HTTP status, when there was an answer.
    pub status: Option<u16>,
    pub upgrade_url: Option<String>,
    /// When a quota resets (ISO 8601, UTC).
    pub resets_at: Option<String>,
    pub retry_after_seconds: Option<u64>,
}

/// A hook a workspace's settings would turn on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookPreview {
    /// `PreToolUse`, `PostToolUse` or `PostToolUseFailure`.
    pub event: String,
    /// The command it runs (shortened when long).
    pub command: String,
    /// The settings file it comes from.
    pub source: String,
}

/// An MCP server a workspace's settings would start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerPreview {
    pub name: String,
    /// `stdio`, `http`, `sse`, `ws`, `sdk` or `managed_proxy`.
    pub transport: String,
    /// The program it starts, or the host it connects to; never arguments, environment
    /// or headers, which can hold secrets.
    pub target: Option<String>,
    pub source: String,
}

/// A permission rule a workspace's settings would add.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRulePreview {
    /// `allow` or `ask` (deny rules apply untrusted too).
    pub effect: String,
    pub rule: String,
    pub source: String,
}

/// One entry of a saved conversation, as a UI rebuilds its transcript from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TranscriptEntry {
    User {
        text: String,
    },
    Assistant {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        input: String,
    },
    ToolResult {
        id: String,
        name: String,
        output: String,
        is_error: bool,
    },
}

/// One item of the agent's task list (its `TodoWrite` tool).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    /// `pending`, `in_progress` or `completed`.
    pub status: String,
    /// The item in the present tense, while it is in progress.
    pub active_form: Option<String>,
}

/// Engine → UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiEvent {
    /// The first event: the engine is ready for commands.
    Ready {
        /// [`PROTOCOL_VERSION`] of the engine.
        protocol: u32,
        /// The Neuron version.
        version: String,
        /// [`ENGINE_FEATURES`]; empty from engines before them.
        #[serde(default)]
        features: Vec<String>,
    },
    /// What the footer shows. Sent after `Ready`, after every `TurnFinished`, and after a
    /// model switch.
    Status {
        model: String,
        permission_mode: String,
        session_id: String,
        cwd: String,
        /// Untrusted workspaces run without their own settings, hooks and MCP servers.
        workspace_trusted: bool,
        plan_mode: bool,
        /// Estimated share of the model's input budget the conversation uses.
        context_percent: u32,
        /// `None` outside a git repository.
        git: Option<GitStatus>,
        /// A usable Zero-X login: `NEURON_TOKEN`, or a stored session that has not expired.
        /// Other providers do not need one, so this alone says nothing about the session.
        #[serde(default)]
        signed_in: bool,
        /// The session cannot run until the user signs in to Zero-X (`/login`): the engine
        /// started signed out, or the session is on Zero-X and has no usable login.
        #[serde(default)]
        sign_in_required: bool,
        /// The login, also when its session expired; `None` when there is none.
        #[serde(default)]
        account: Option<AccountInfo>,
        /// The catalog says the current model reads images ([`FEATURE_MODEL_IMAGES`]);
        /// `None` when no catalog says. A UI hides its image-paste hint unless `true`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supports_images: Option<bool>,
        /// The model that answered the last request, when it is not `model` (`auto`
        /// picks one per request; a failover can switch). `None` before the first
        /// answer, from providers that do not name the model, and from older engines.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        served_model: Option<String>,
    },
    /// After a sign-in from the signed-out start: the engine is opening the session
    /// (plugins, MCP servers), which can take a while. `TurnFinished` and a `Status` with
    /// the session follow, or an `Error`.
    SessionStarting,
    /// `/login` started a device sign-in: open `url` (or `url_complete`, which has the code
    /// filled in) and enter `user_code`. The engine waits for the approval; an interrupt
    /// cancels it. A notice and a `Status` follow when it is done, then `TurnFinished`.
    SignInCode {
        url: String,
        url_complete: Option<String>,
        user_code: String,
        expires_in_secs: u64,
        /// The engine opened `url_complete` (or `url`) in the browser.
        browser_opened: bool,
    },
    /// Answer to [`EngineCommand::ListModels`]. Empty when the provider has no catalog
    /// (only the Zero-X gateway publishes one).
    Models {
        current: String,
        models: Vec<ModelInfo>,
    },
    /// A streamed piece of assistant text (Markdown, not yet rendered).
    TextDelta { text: String },
    /// The current text block ended.
    TextBlockEnd,
    /// A reasoning block started. `chars` is known for complete blocks; `redacted` when the
    /// provider hides the reasoning.
    ReasoningStarted {
        chars: Option<usize>,
        redacted: bool,
    },
    /// Reasoning text, when the provider streams it.
    ReasoningDelta { text: String },
    /// A tool call with its complete input. `cut_off`: the output limit truncated it and it
    /// will not run.
    ToolCallStarted {
        id: String,
        name: String,
        input: String,
        cut_off: bool,
    },
    /// A tool call's result as the model sees it, for every call: one that ran (`is_error`
    /// when it failed), one denied or blocked by a hook, and one that never started
    /// (interrupted, cut off). `id` is the [`UiEvent::ToolCallStarted`] id.
    ToolResult {
        #[serde(default)]
        id: String,
        name: String,
        output: String,
        is_error: bool,
    },
    /// Token usage: this turn so far, the session total, and whether it is an estimate.
    Usage {
        turn: TokenCounts,
        cumulative: TokenCounts,
        estimated: bool,
    },
    /// A tool call waits for approval; answer with a [`PermissionReply`] carrying `id`.
    PermissionRequested { id: u64, request: PermissionView },
    /// A one-line notice (retries, warnings, interrupts).
    Notice { message: String },
    /// The turn is over (finished, cancelled or failed).
    TurnFinished,
    /// The turn failed; `message` is what to show.
    Error {
        message: String,
        /// What the failure needs from the user, when a server said
        /// ([`FEATURE_ERROR_DETAILS`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<ErrorDetail>,
    },
    /// Answer to [`EngineCommand::ListSessions`]: the saved chats in `scope`, newest
    /// first.
    Sessions {
        current: String,
        sessions: Vec<SessionSummary>,
        #[serde(default)]
        scope: SessionScope,
    },
    /// The conversation of the session the engine now runs, for the UI to show from the
    /// start: after `--resume`, [`EngineCommand::Resume`], `/resume` and `/clear`. Long
    /// outputs are shortened; `truncated` when the oldest entries were left out.
    Transcript {
        session_id: String,
        entries: Vec<TranscriptEntry>,
        truncated: bool,
    },
    /// [`EngineCommand::TrustWorkspace`] recorded the workspace as trusted. Its own
    /// settings, hooks and MCP servers load when the session starts, so the UI restarts
    /// the engine (`--resume` keeps the chat) when `restart_required`.
    WorkspaceTrusted {
        cwd: String,
        restart_required: bool,
        /// The folder trust was recorded for: the git root, or the folder itself.
        #[serde(default)]
        root: Option<String>,
        /// Trusted for this engine only (the home folder): the UI restarts the engine
        /// with `NEURON_TRUST_WORKSPACE=1` and asks again at the next launch.
        #[serde(default)]
        session_only: bool,
    },
    /// The agent asks the user something (with [`CAPABILITY_QUESTIONS`] on). Answer with
    /// [`EngineCommand::Answer`] and the same `id` (or a plain `Submit`).
    Question {
        id: u64,
        question: String,
        /// Choices, when the agent offered some; the answer may be one of them or free
        /// text.
        options: Vec<String>,
    },
    /// The agent's task list changed (its `TodoWrite` tool ran): the whole list.
    Todos { items: Vec<TodoItem> },
    /// A sub-agent started (an `Agent` tool call began running). `id` is the call's
    /// [`UiEvent::ToolCallStarted`] id.
    SubagentStarted {
        id: String,
        description: String,
        subagent_type: Option<String>,
    },
    /// A sub-agent's `Agent` call ended: `status` is the sub-agent's (`completed`,
    /// `failed`, `running` when it went on in the background, ...); `result` its final
    /// text, `note` why it stopped early.
    SubagentFinished {
        id: String,
        status: String,
        is_error: bool,
        result: Option<String>,
        note: Option<String>,
    },
    /// Answer to [`EngineCommand::ListExtensions`]: the MCP servers with what each is
    /// doing now, the plugins, the skills and the agent definitions. Nothing secret:
    /// no commands, arguments, environment or headers.
    Extensions {
        mcp_servers: Vec<McpServerInfo>,
        plugins: Vec<PluginInfo>,
        skills: Vec<DefinitionInfo>,
        agents: Vec<DefinitionInfo>,
    },
    /// What a report command (`/usage`, `/init`, `/memory`, `/doctor`) found, for a
    /// scrollable panel, with [`CAPABILITY_REPORTS`] on; `TurnFinished` follows, as after
    /// any slash command. `command` is the command's name without the slash.
    Report {
        command: String,
        title: String,
        sections: Vec<ReportSection>,
        links: Vec<ReportLink>,
    },
    /// The Zero-X account ([`FEATURE_ACCOUNT`]): sent after `Ready` when there is a
    /// login, again once the plan and usage were read, after `/login` and `/logout`,
    /// on [`EngineCommand::RefreshAccount`], and after a turn whose quota headers changed
    /// today's usage. `account` is `None` when there is no login.
    Account {
        signed_in: bool,
        account: Option<AccountDetails>,
    },
    /// Answer to [`EngineCommand::TrustPreview`]: what trusting the workspace would turn
    /// on. Reading it starts nothing.
    TrustPreview {
        cwd: String,
        /// The folder trust would be recorded for: the git root, or the folder itself.
        root: String,
        /// `root` is the top folder of a git repository (else it is `cwd`, outside git).
        is_git_root: bool,
        /// Trust would last for this engine only (the home folder, or above it).
        session_only: bool,
        /// The workspace is trusted already.
        trusted: bool,
        /// The workspace's settings files.
        settings_files: Vec<String>,
        hooks: Vec<HookPreview>,
        mcp_servers: Vec<McpServerPreview>,
        permission_rules: Vec<PermissionRulePreview>,
        /// Other settings the files would apply, by key (`env`, `model`, `sandbox`, ...).
        other_settings: Vec<String>,
    },
}

impl UiEvent {
    /// An [`UiEvent::Error`] with nothing for the user to act on beyond its message.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
            detail: None,
        }
    }
}

/// A file, folder or image the user attached to a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Attachment {
    File { path: String },
    Folder { path: String },
    Image { path: String, media_type: String },
}

/// UI → engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineCommand {
    /// Run a turn with this prompt.
    Submit {
        text: String,
        attachments: Vec<Attachment>,
    },
    /// Run a slash command (`/model`, `/compact`, ...), without the leading slash.
    Slash { command: String, args: String },
    /// Switch the model for the next turn.
    SetModel { model: String },
    /// Ask for the models the user can switch to ([`UiEvent::Models`]).
    ListModels,
    /// Interrupt the running turn (a second one quits after saving, like Ctrl-C twice).
    /// Sent right after a `Submit` the engine has not started yet, it cancels that
    /// prompt's turn; with nothing running or waiting, it is dropped.
    Interrupt,
    /// Save and stop.
    Shutdown,
    /// What this UI handles (`features` from [`UiEvent::Ready`] lists `capabilities`):
    /// [`CAPABILITY_QUESTIONS`] and [`CAPABILITY_REPORTS`]. Replaces what an earlier one
    /// said.
    Capabilities {
        features: Vec<String>,
        /// Which UI this is (`terminal`, `desktop`, ...), recorded with the sessions it
        /// runs so other UIs can say where a session is open.
        #[serde(default)]
        client: Option<String>,
    },
    /// Answer the [`UiEvent::Question`] with this `id`. A stale id is ignored.
    Answer { id: u64, text: String },
    /// Ask for the saved chats in `scope` ([`UiEvent::Sessions`]); engines without
    /// [`FEATURE_SESSION_SCOPE`] list this workspace's whatever the scope.
    ListSessions {
        #[serde(default)]
        scope: SessionScope,
    },
    /// Continue a saved chat (an id from [`UiEvent::Sessions`], a path, or `latest`):
    /// a [`UiEvent::Transcript`] and a `Status` follow, or an `Error`.
    Resume { session: String },
    /// Trust the workspace, as the line REPL's trust prompt does; a
    /// [`UiEvent::WorkspaceTrusted`] and a `Status` follow, or an `Error`.
    TrustWorkspace,
    /// Ask for the extensions ([`UiEvent::Extensions`]).
    ListExtensions,
    /// Ask what trusting the workspace would turn on ([`UiEvent::TrustPreview`]).
    TrustPreview,
    /// Read the account's plan and usage again ([`UiEvent::Account`]).
    RefreshAccount,
}

/// The user's answer to a permission request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PermissionDecision {
    /// Run this call.
    AllowOnce,
    /// Run this call and similar ones for the rest of the session.
    AllowForSession,
    /// Do not run it; `note` is passed to the model.
    Deny { note: Option<String> },
}

/// Answer to [`UiEvent::PermissionRequested`] with the same `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionReply {
    pub id: u64,
    pub decision: PermissionDecision,
}

/// One input line of `neuron engine --stdio`: a command, or an answer to a permission
/// request. `{"kind":"command","type":"submit","text":"hi","attachments":[]}`,
/// `{"kind":"permission_reply","id":1,"decision":{"kind":"allow_once"}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientMessage {
    Command(EngineCommand),
    PermissionReply(PermissionReply),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_and_commands_round_trip_as_tagged_json() {
        let event = UiEvent::ToolCallStarted {
            id: "toolu_1".into(),
            name: "bash".into(),
            input: r#"{"command":"cargo test"}"#.into(),
            cut_off: false,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "tool_call_started");
        assert_eq!(serde_json::from_value::<UiEvent>(json).unwrap(), event);

        let command = EngineCommand::Submit {
            text: "fix the test".into(),
            attachments: vec![Attachment::Image {
                path: "shot.png".into(),
                media_type: "image/png".into(),
            }],
        };
        let json = serde_json::to_value(&command).unwrap();
        assert_eq!(json["type"], "submit");
        assert_eq!(json["attachments"][0]["kind"], "image");
        assert_eq!(
            serde_json::from_value::<EngineCommand>(json).unwrap(),
            command
        );

        let reply = PermissionReply {
            id: 7,
            decision: PermissionDecision::Deny {
                note: Some("use a dry run".into()),
            },
        };
        let json = serde_json::to_value(&reply).unwrap();
        assert_eq!(json["decision"]["kind"], "deny");
        assert_eq!(
            serde_json::from_value::<PermissionReply>(json).unwrap(),
            reply
        );
    }

    /// Fields added after protocol 1 shipped read as their defaults from older engines.
    #[test]
    fn later_fields_default_when_absent() {
        let result: UiEvent = serde_json::from_str(
            r#"{"type":"tool_result","name":"bash","output":"ok","is_error":false}"#,
        )
        .unwrap();
        assert_eq!(
            result,
            UiEvent::ToolResult {
                id: String::new(),
                name: "bash".into(),
                output: "ok".into(),
                is_error: false,
            }
        );
        let status: UiEvent = serde_json::from_str(
            r#"{"type":"status","model":"m","permission_mode":"read-only","session_id":"s","cwd":".","workspace_trusted":false,"plan_mode":false,"context_percent":0,"git":null}"#,
        )
        .unwrap();
        assert!(matches!(
            status,
            UiEvent::Status {
                signed_in: false,
                sign_in_required: false,
                account: None,
                served_model: None,
                ..
            }
        ));
    }

    /// Protocol 1 additions: engine features (absent from older engines), the commands
    /// they add, and their events, as JSON lines.
    #[test]
    fn features_sessions_questions_and_tasks_as_json() {
        let old: UiEvent =
            serde_json::from_str(r#"{"type":"ready","protocol":1,"version":"6.3.0"}"#).unwrap();
        assert_eq!(
            old,
            UiEvent::Ready {
                protocol: 1,
                version: "6.3.0".into(),
                features: Vec::new(),
            }
        );
        assert!(ENGINE_FEATURES.contains(&FEATURE_CAPABILITIES));

        for (line, command) in [
            (
                r#"{"kind":"command","type":"capabilities","features":["questions"]}"#,
                EngineCommand::Capabilities {
                    features: vec![CAPABILITY_QUESTIONS.into()],
                    client: None,
                },
            ),
            (
                r#"{"kind":"command","type":"answer","id":3,"text":"dev"}"#,
                EngineCommand::Answer {
                    id: 3,
                    text: "dev".into(),
                },
            ),
            (
                r#"{"kind":"command","type":"list_sessions"}"#,
                EngineCommand::ListSessions {
                    scope: SessionScope::Project,
                },
            ),
            (
                r#"{"kind":"command","type":"resume","session":"latest"}"#,
                EngineCommand::Resume {
                    session: "latest".into(),
                },
            ),
            (
                r#"{"kind":"command","type":"trust_workspace"}"#,
                EngineCommand::TrustWorkspace,
            ),
            (
                r#"{"kind":"command","type":"list_extensions"}"#,
                EngineCommand::ListExtensions,
            ),
        ] {
            assert_eq!(
                serde_json::from_str::<ClientMessage>(line).unwrap(),
                ClientMessage::Command(command),
                "{line}"
            );
        }

        let transcript = UiEvent::Transcript {
            session_id: "s".into(),
            entries: vec![
                TranscriptEntry::User { text: "hi".into() },
                TranscriptEntry::ToolCall {
                    id: "t".into(),
                    name: "bash".into(),
                    input: "{}".into(),
                },
            ],
            truncated: false,
        };
        let json = serde_json::to_value(&transcript).unwrap();
        assert_eq!(json["type"], "transcript");
        assert_eq!(json["entries"][1]["kind"], "tool_call");
        assert_eq!(serde_json::from_value::<UiEvent>(json).unwrap(), transcript);

        let todos = serde_json::to_value(UiEvent::Todos {
            items: vec![TodoItem {
                content: "Run the tests".into(),
                status: "in_progress".into(),
                active_form: Some("Running the tests".into()),
            }],
        })
        .unwrap();
        assert_eq!(todos["items"][0]["active_form"], "Running the tests");
    }

    /// The stdio transport's input lines: a top-level `kind`, then the command's own
    /// `type` (or the reply's fields) beside it.
    #[test]
    fn client_messages_are_flat_tagged_json_lines() {
        let line = r#"{"kind":"command","type":"submit","text":"hi","attachments":[]}"#;
        assert_eq!(
            serde_json::from_str::<ClientMessage>(line).unwrap(),
            ClientMessage::Command(EngineCommand::Submit {
                text: "hi".into(),
                attachments: Vec::new(),
            })
        );
        assert_eq!(
            serde_json::from_str::<ClientMessage>(r#"{"kind":"command","type":"interrupt"}"#)
                .unwrap(),
            ClientMessage::Command(EngineCommand::Interrupt)
        );
        let reply = r#"{"kind":"permission_reply","id":3,"decision":{"kind":"allow_once"}}"#;
        assert_eq!(
            serde_json::from_str::<ClientMessage>(reply).unwrap(),
            ClientMessage::PermissionReply(PermissionReply {
                id: 3,
                decision: PermissionDecision::AllowOnce,
            })
        );
        let message = ClientMessage::Command(EngineCommand::Slash {
            command: "model".into(),
            args: "opus".into(),
        });
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["kind"], "command");
        assert_eq!(json["type"], "slash");
        assert_eq!(
            serde_json::from_value::<ClientMessage>(json).unwrap(),
            message
        );
        assert!(serde_json::from_str::<ClientMessage>(r#"{"type":"submit"}"#).is_err());
    }

    /// Protocol v2's commands as JSON lines, and the fields they add reading as their
    /// defaults when an older UI leaves them out.
    #[test]
    fn v2_commands_round_trip_as_json_lines() {
        for (line, command) in [
            (
                r#"{"kind":"command","type":"list_sessions","scope":"all"}"#,
                EngineCommand::ListSessions {
                    scope: SessionScope::All,
                },
            ),
            (
                r#"{"kind":"command","type":"capabilities","features":["questions","reports"],"client":"desktop"}"#,
                EngineCommand::Capabilities {
                    features: vec![CAPABILITY_QUESTIONS.into(), CAPABILITY_REPORTS.into()],
                    client: Some("desktop".into()),
                },
            ),
            (
                r#"{"kind":"command","type":"trust_preview"}"#,
                EngineCommand::TrustPreview,
            ),
            (
                r#"{"kind":"command","type":"refresh_account"}"#,
                EngineCommand::RefreshAccount,
            ),
        ] {
            let message = ClientMessage::Command(command);
            assert_eq!(
                serde_json::from_str::<ClientMessage>(line).unwrap(),
                message,
                "{line}"
            );
            let written = serde_json::to_string(&message).unwrap();
            assert_eq!(
                serde_json::from_str::<ClientMessage>(&written).unwrap(),
                message
            );
        }
        assert!(serde_json::from_str::<ClientMessage>(
            r#"{"kind":"command","type":"list_sessions","scope":"galaxy"}"#
        )
        .is_err());
    }

    /// Protocol v2's events survive a round trip, with the tags and field names the UIs
    /// read.
    #[test]
    fn v2_events_round_trip_as_tagged_json() {
        for event in crate::fake_engine::samples::v2_events() {
            let json = serde_json::to_value(&event).unwrap();
            assert_eq!(
                serde_json::from_value::<UiEvent>(json.clone()).unwrap(),
                event,
                "{json}"
            );
        }

        let quota = serde_json::to_value(crate::fake_engine::samples::quota_error()).unwrap();
        assert_eq!(quota["type"], "error");
        assert_eq!(quota["detail"]["code"], "insufficient_quota");
        assert_eq!(quota["detail"]["status"], 429);
        assert_eq!(quota["detail"]["resets_at"], "2026-10-24T00:00:00Z");
        // A plain error has no `detail` key at all, as before.
        let plain = serde_json::to_value(UiEvent::error("boom")).unwrap();
        assert_eq!(
            plain,
            serde_json::json!({"type": "error", "message": "boom"})
        );

        let sessions =
            serde_json::to_value(crate::fake_engine::samples::sessions(SessionScope::All)).unwrap();
        assert_eq!(sessions["scope"], "all");
        assert_eq!(sessions["sessions"][1]["run_status"], "running");
        assert_eq!(sessions["sessions"][1]["client"], "terminal");

        let preview = serde_json::to_value(crate::fake_engine::samples::trust_preview()).unwrap();
        assert_eq!(preview["type"], "trust_preview");
        assert_eq!(preview["mcp_servers"][0]["target"], "uvx");
        assert_eq!(preview["permission_rules"][0]["effect"], "allow");
    }

    /// Fields protocol v2 added to protocol 1 events read as their defaults from older
    /// engines.
    #[test]
    fn v2_fields_default_from_older_engines() {
        let sessions: UiEvent = serde_json::from_str(
            r#"{"type":"sessions","current":"s1","sessions":[{"id":"s1","title":null,"model":null,"message_count":2,"updated_at_ms":5,"current":true,"branch":null}]}"#,
        )
        .unwrap();
        let UiEvent::Sessions {
            scope, sessions, ..
        } = sessions
        else {
            panic!("a sessions event");
        };
        assert_eq!(scope, SessionScope::Project);
        assert_eq!(sessions[0].run_status, SessionRunStatus::Idle);
        assert!(!sessions[0].failed);
        assert_eq!(sessions[0].project, None);

        let trusted: UiEvent = serde_json::from_str(
            r#"{"type":"workspace_trusted","cwd":"/repo","restart_required":true}"#,
        )
        .unwrap();
        assert_eq!(
            trusted,
            UiEvent::WorkspaceTrusted {
                cwd: "/repo".into(),
                restart_required: true,
                root: None,
                session_only: false,
            }
        );

        let error: UiEvent = serde_json::from_str(r#"{"type":"error","message":"x"}"#).unwrap();
        assert_eq!(error, UiEvent::error("x"));

        let model: ModelInfo = serde_json::from_str(
            r#"{"id":"m","display_name":null,"available":true,"unavailable_reason":null,"context_window":null,"tiers":[]}"#,
        )
        .unwrap();
        assert_eq!(model.supports_images, None);
    }
}
