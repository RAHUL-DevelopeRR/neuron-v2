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

use std::collections::BTreeMap;
use std::fmt;

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
    /// The `permissions.allow` rules "always allow in this project"
    /// ([`PermissionDecision::AllowForProject`]) would save to the project's
    /// `.neuron/settings.local.json`. Added after protocol 1: older engines leave it
    /// out, and a UI offers the choice only when it is not empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub project_rules: Vec<String>,
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
    /// Who serves it ([`FEATURE_STATUS_BAR`]): the gateway's `owned_by` or `provider`
    /// (`gemini`, `groq`, `zero-x` for `auto`, ...). A UI draws it as a monogram.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The thinking levels it takes, lowest first ([`FEATURE_STATUS_BAR`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_efforts: Option<Vec<String>>,
    /// The catalog says it reasons ([`FEATURE_STATUS_BAR`]); `None` when it does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_reasoning: Option<bool>,
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

/// The feature that adds [`EngineCommand::Capabilities`]. Engine features are listed in
/// [`UiEvent::Ready`]'s `features`; a UI sends a feature's commands only when it is listed.
pub const FEATURE_CAPABILITIES: &str = "capabilities";

/// The engine can report a turn while the model writes it: [`UiEvent::ToolInputStarted`],
/// [`UiEvent::ToolInputProgress`], [`UiEvent::OutputProgress`] and
/// [`UiEvent::RequestUsage`]. Sent only to a UI that declared [`CAPABILITY_LIVE_PROGRESS`].
pub const FEATURE_LIVE_PROGRESS: &str = "live_progress";

/// The engine reports the files the agent changed ([`UiEvent::Changes`]) and answers
/// `/diff`, `/commit` and [`EngineCommand::Commit`] with [`UiEvent::Diff`] and
/// [`UiEvent::CommitProposal`]. Without [`CAPABILITY_GIT_VIEWS`] it answers with notices.
pub const FEATURE_GIT_VIEWS: &str = "git_views";

/// The engine reports a shell command's output while it runs
/// ([`UiEvent::ToolOutputDelta`]). Sent only to a UI that declared
/// [`CAPABILITY_LIVE_OUTPUT`].
pub const FEATURE_LIVE_OUTPUT: &str = "live_output";

/// The engine keeps a checkpoint per prompt: `/rewind` lists them
/// ([`UiEvent::Checkpoints`]) and [`EngineCommand::Rewind`] goes back to one
/// ([`UiEvent::Rewound`]). Without [`CAPABILITY_GIT_VIEWS`] it answers with notices.
pub const FEATURE_CHECKPOINTS: &str = "checkpoints";

/// [`EngineCommand::SetMcpServerEnabled`] turns an MCP server on or off (persisted to
/// `disabledMcpServers` in the user settings); [`McpServerInfo::enabled`] is set and
/// disabled servers are listed with status `disabled`.
pub const FEATURE_MCP_TOGGLE: &str = "mcp_toggle";
/// The engine reports hook commands as they run ([`UiEvent::HookStarted`] and
/// [`UiEvent::HookFinished`]). Sent only to a UI that declared
/// [`CAPABILITY_HOOK_PROGRESS`].
pub const FEATURE_HOOK_PROGRESS: &str = "hook_progress";

/// The connector catalog: [`EngineCommand::ListConnectorCatalog`] (answered with
/// [`UiEvent::ConnectorCatalog`]), [`EngineCommand::AddMcpServer`],
/// [`EngineCommand::RemoveMcpServer`] and [`EngineCommand::ApproveMcpServer`] (each
/// answered with a notice and a fresh [`UiEvent::Extensions`], or an error), and
/// [`McpServerInfo::catalog`].
pub const FEATURE_MCP_CATALOG: &str = "mcp_catalog";

/// The connector policy (managed settings and the Zero-X plan): [`McpServerInfo`] rows
/// with scope `managed`, status `blocked` with the policy's reason in `error`, and
/// [`McpServerInfo::required`].
pub const FEATURE_MCP_POLICY: &str = "mcp_policy";

/// Hosted connectors through Zero-X: [`McpServerInfo`] rows with transport `zerox`,
/// status `needs_connect` while the user has not linked the connector on the Zero-X
/// dashboard, and [`McpServerInfo::connect_url`], the page to link it on (a UI opens it).
pub const FEATURE_MCP_HOSTED: &str = "mcp_hosted";
/// The prompt status bar's engine side: `provider`, `model_display_name`,
/// `reasoning_effort(s)`, `full_access_allowed` and `context` on [`UiEvent::Status`];
/// `provider`, `reasoning_efforts` and `supports_reasoning` on [`ModelInfo`];
/// [`EngineCommand::SetReasoningEffort`], [`EngineCommand::SetPermissionMode`],
/// [`EngineCommand::ListBranches`] (answered by [`UiEvent::Branches`]) and
/// [`EngineCommand::SwitchBranch`]; `/effort`, and `/context` as a report.
pub const FEATURE_STATUS_BAR: &str = "status_bar";

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
    FEATURE_LIVE_PROGRESS,
    FEATURE_GIT_VIEWS,
    FEATURE_CHECKPOINTS,
    FEATURE_MCP_TOGGLE,
    FEATURE_LIVE_OUTPUT,
    FEATURE_HOOK_PROGRESS,
    FEATURE_MCP_CATALOG,
    FEATURE_MCP_POLICY,
    FEATURE_MCP_HOSTED,
    FEATURE_STATUS_BAR,
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
    /// Where it is configured: `bundled`, `user`, `project`, `local` or `managed` (the
    /// machine's managed settings, or a server the connector policy requires).
    pub scope: String,
    /// `stdio`, `http`, `sse`, `ws`, `sdk` or `zerox` (a hosted connector through
    /// Zero-X; older engines said `managed_proxy`).
    pub transport: String,
    /// `starting`, `ready`, `failed`, `pending` (started, tools not found),
    /// `unsupported` (a transport Neuron does not speak yet), `not_started`,
    /// `disabled` (turned off in the settings), `needs_approval` (a workspace server
    /// that is new or changed: it starts once the user approves it), `blocked`
    /// (a deny rule or the connector policy refuses it) or `needs_connect` (a hosted
    /// connector not linked to the Zero-X account yet, see [`Self::connect_url`]);
    /// `error` says why for the last three.
    /// Older engines send none of the last three; a UI shows an unknown status as is.
    pub status: String,
    /// Its tools, without the `mcp__<server>__` prefix.
    pub tools: Vec<String>,
    /// Why it failed or is unsupported.
    pub error: Option<String>,
    /// Turned on ([`FEATURE_MCP_TOGGLE`]); older engines leave it out, meaning on.
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// The catalog connector it was added from, when its launch spec is exactly that
    /// entry's ([`FEATURE_MCP_CATALOG`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    /// The connector policy requires it ([`FEATURE_MCP_POLICY`]): it is on, and a UI
    /// shows it locked instead of offering to turn it off. Older engines leave it out.
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    /// A stdio server that is not a catalog connector and whose version and hashes are
    /// not recorded (`/mcp pin <name>` records them). A UI shows an `unpinned` badge.
    /// Older engines leave it out.
    #[serde(default, skip_serializing_if = "is_false")]
    pub unpinned: bool,
    /// Where the user links this hosted connector, while its status is `needs_connect`
    /// ([`FEATURE_MCP_HOSTED`]): a page on the Zero-X gateway. Older engines leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_url: Option<String>,
    /// The permission tier of each of its tools (`read`, `write`, `destructive` or
    /// `external`) as the connector catalog pins it, for a catalog or hosted connector
    /// ([`FEATURE_MCP_CATALOG`]): a UI shows it on the tool's calls. Other servers, and
    /// older engines, leave it out.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tool_tiers: BTreeMap<String, String>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// What the user typed for a connector's variables, secrets included
/// ([`EngineCommand::AddMcpServer`]). Its debug output names the variables and never
/// shows a value, so logging a command cannot leak one.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretValues(pub BTreeMap<String, String>);

impl fmt::Debug for SecretValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.0.keys()).finish()
    }
}

/// A variable a catalog connector needs ([`ConnectorInfo::variables`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorVariable {
    /// The environment variable or `${secret:NAME}` name.
    pub name: String,
    pub label: String,
    /// Kept in the OS keychain; a UI masks its field.
    pub secret: bool,
    pub required: bool,
    #[serde(default)]
    pub default: Option<String>,
    /// Where to get it (a token page).
    #[serde(default)]
    pub help_url: Option<String>,
}

/// One connector of the catalog ([`UiEvent::ConnectorCatalog`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub publisher: String,
    /// Published by the service's vendor or the MCP project.
    pub official: bool,
    pub licence: String,
    /// Its source repository.
    pub source: String,
    /// `stdio` (runs locally) or `http` (a remote server).
    pub transport: String,
    /// `<package>@<version>`, or the URL of a remote server.
    pub pinned: String,
    /// Signs in with OAuth once added (`neuron mcp login <server>`).
    pub oauth: bool,
    /// `user`, `project` or `local`.
    pub default_scope: String,
    pub variables: Vec<ConnectorVariable>,
    /// The configured servers added from it.
    pub added: Vec<String>,
}

fn enabled_by_default() -> bool {
    true
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

/// A UI capability ([`EngineCommand::Capabilities`]): questions from the agent come as
/// [`UiEvent::Question`] instead of a notice starting with "Question:".
pub const CAPABILITY_QUESTIONS: &str = "questions";

/// A UI capability: reports come as [`UiEvent::Report`] instead of a notice.
pub const CAPABILITY_REPORTS: &str = "reports";

/// A UI capability: send the live-progress events of [`FEATURE_LIVE_PROGRESS`].
pub const CAPABILITY_LIVE_PROGRESS: &str = "live_progress";

/// A UI capability: send changes, diffs, commit proposals, checkpoints and rewinds as
/// their own events instead of notices.
pub const CAPABILITY_GIT_VIEWS: &str = "git_views";

/// A UI capability: send the output of a running shell command as
/// [`UiEvent::ToolOutputDelta`] ([`FEATURE_LIVE_OUTPUT`]).
pub const CAPABILITY_LIVE_OUTPUT: &str = "live_output";

/// Which stream of a command a [`UiEvent::ToolOutputDelta`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// A UI capability: send the hook events of [`FEATURE_HOOK_PROGRESS`].
pub const CAPABILITY_HOOK_PROGRESS: &str = "hook_progress";

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
    /// The plan's rolling token window (Zero-X Free: 1M tokens per 5 hours), which
    /// refuses requests before the daily figures do, kept current from the quota headers
    /// of each reply; `None` for a plan without one, before the account is read, and from
    /// older engines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_window: Option<UsageWindow>,
}

/// A rolling usage window (`usage_window` in the gateway's account). Every part may be
/// missing; `tokens` and `remaining_tokens` are `None` while not known (also once
/// `resets_at` has passed, until the account is read again).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageWindow {
    /// Tokens used in the window.
    pub tokens: Option<u64>,
    pub limit_tokens: Option<u64>,
    pub remaining_tokens: Option<u64>,
    /// When the oldest usage leaves the window (ISO 8601, UTC).
    pub resets_at: Option<String>,
    /// The window's length: 18000 for five hours.
    pub window_seconds: Option<u64>,
}

/// What fills the context window ([`FEATURE_STATUS_BAR`]), as estimated tokens.
/// `used_tokens` is what each request sends (system prompt, tool schemas, instructions
/// and the conversation); `context_window - output_reserve` is what it may use.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBreakdown {
    pub context_window: u64,
    /// Kept free for the reply (`max_tokens`).
    pub output_reserve: u64,
    /// The system prompt without the project instructions.
    pub system_tokens: u64,
    pub tool_schema_tokens: u64,
    pub tool_count: u64,
    /// `NEURON.md` and the other instruction files and memory.
    pub instruction_tokens: u64,
    pub instruction_files: Vec<String>,
    /// Prompts, replies and tool calls.
    pub conversation_tokens: u64,
    /// Tool results.
    pub tool_output_tokens: u64,
    pub used_tokens: u64,
    /// Auto-compaction starts when `used_tokens` reaches this.
    pub threshold_tokens: u64,
}

/// One local branch, for the branch picker ([`UiEvent::Branches`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchInfo {
    pub name: String,
    /// Its upstream (`origin/main`), when it has one.
    pub upstream: Option<String>,
    /// Commits ahead of and behind the upstream; `None` without one.
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    /// Its last commit (Unix milliseconds).
    pub last_commit_ms: Option<u64>,
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
    /// `stdio`, `http`, `sse`, `ws`, `sdk` or `zerox`.
    pub transport: String,
    /// The program it starts, or the host it connects to; never arguments, environment
    /// or headers, which can hold secrets.
    pub target: Option<String>,
    pub source: String,
    /// What runs, for the user to read before approving: the program and its
    /// arguments, or the URL without credentials or query, with secret-looking values
    /// shown as `[REDACTED]`; never environment or headers. Added after protocol 1:
    /// older engines leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The digest of the server's launch spec as previewed. A UI that approves the
    /// server from this preview sends it back in [`EngineCommand::ApproveMcpServer`],
    /// and the engine refuses the approval if the config changed since. Added after
    /// protocol 1: older engines leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
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

/// A file and how it changed, with line counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    /// Relative to the workspace root, with `/`; absolute for files outside it.
    pub path: String,
    /// `created`, `modified`, `deleted` or `renamed`.
    pub status: String,
    /// Lines added and removed (0 for binary files).
    pub added: u64,
    pub removed: u64,
}

/// One file of a [`UiEvent::Diff`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffFile {
    /// Relative to the workspace root, with `/`.
    pub path: String,
    /// `created`, `modified`, `deleted` or `renamed`.
    pub status: String,
    pub added: u64,
    pub removed: u64,
    /// The unified diff's hunks (from the first `@@` line on); empty for binary files and
    /// for files left out to keep the event small.
    pub patch: String,
    pub binary: bool,
}

/// One entry of the "go back to" list ([`UiEvent::Checkpoints`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointInfo {
    pub id: u64,
    /// When the prompt was sent (Unix milliseconds).
    pub created_at_ms: u64,
    /// The prompt, shortened to one line.
    pub prompt_preview: String,
    /// Files the agent's file tools changed in that turn (workspace-relative, with `/`).
    pub files: Vec<String>,
    /// Shell commands that turn ran; a rewind cannot undo what they did.
    pub shell_commands: Vec<String>,
    /// Set on the checkpoint a code-only rewind saves first: the checkpoint that rewind
    /// went back to, so the rewind itself can be undone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_rewind_to: Option<u64>,
}

/// A file a rewind could not put back, and why (as people read it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotRestoredFile {
    pub path: String,
    pub reason: String,
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
    /// What the footer shows. Sent after `Ready`, after every `TurnFinished`, after a
    /// model switch, and after a tool call that may have changed files (for the git
    /// counts).
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
        /// Estimated input tokens of each model request the next turn sends (system
        /// prompt, tool definitions and the conversation): the same figure as
        /// `ContextBreakdown.used_tokens` where an engine sends both. A turn sends one
        /// request per step, so a long agent task spends this many times over. `None`
        /// from older engines. The plan's token window is on
        /// [`AccountDetails::usage_window`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_tokens_estimate: Option<u64>,
        /// Who serves the model ([`FEATURE_STATUS_BAR`]), as on [`ModelInfo`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        /// The catalog's name for the model ([`FEATURE_STATUS_BAR`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_display_name: Option<String>,
        /// The thinking level the next turn asks for; `None` for the model's default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        /// The levels the model takes; `None` when unknown, empty when it does not reason.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_efforts: Option<Vec<String>>,
        /// Full access may be picked: the engine started with it, or with
        /// `--dangerously-skip-permissions`.
        #[serde(default)]
        full_access_allowed: bool,
        /// What fills the context window ([`FEATURE_STATUS_BAR`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<ContextBreakdown>,
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
    Notice {
        message: String,
        /// What the notice is about, when the UI should draw it apart from status
        /// lines. Added after protocol 1: absent from older engines and left out of
        /// plain notices, so a UI that does not know it shows a plain notice.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<NoticeKind>,
    },
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
    /// Answer to [`EngineCommand::ListConnectorCatalog`] ([`FEATURE_MCP_CATALOG`]): the
    /// catalog's version and the connectors that match the query.
    ConnectorCatalog {
        version: String,
        connectors: Vec<ConnectorInfo>,
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
    /// Answer to [`EngineCommand::ListBranches`]: the local branches, newest commit first.
    Branches {
        /// The checked-out branch; `None` on a detached HEAD.
        current: Option<String>,
        branches: Vec<BranchInfo>,
        /// The working tree has uncommitted changes (switching is refused then).
        dirty: bool,
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
    /// The model started a tool call and is still writing its input: show the call now.
    /// [`UiEvent::ToolInputProgress`] follows while the input grows, then
    /// [`UiEvent::ToolCallStarted`] with the same `id` and the complete input. With
    /// [`CAPABILITY_LIVE_PROGRESS`] only.
    ToolInputStarted { id: String, name: String },
    /// How far the model got writing a tool call's input. Sent at most every 100 ms or
    /// so per call while it grows, and once more when the input is complete. With
    /// [`CAPABILITY_LIVE_PROGRESS`] only.
    ToolInputProgress {
        id: String,
        name: String,
        /// Bytes of input (JSON text) so far.
        bytes: u64,
        /// Lines of the call's main text so far: a written file's content, an edit's
        /// new text, a command. 0 before it starts.
        lines: u64,
        /// What the call is about, once the model has written it: a path, a command, a
        /// search pattern, a URL.
        target: Option<String>,
        /// The last lines of the main text so far (a few lines, at most about 1 KB),
        /// unescaped.
        preview: Option<String>,
    },
    /// Output tokens of the model response now streaming (text, reasoning and tool
    /// input), estimated from its length. Starts again from 0 with each model request
    /// of a turn. With [`CAPABILITY_LIVE_PROGRESS`] only.
    OutputProgress { output_tokens: u64 },
    /// One model request of the turn ended: its tokens as the provider reported them.
    /// [`UiEvent::Usage`] still closes the turn with its totals. With
    /// [`CAPABILITY_LIVE_PROGRESS`] only.
    RequestUsage { usage: TokenCounts },
    /// More of what a running `bash` or `PowerShell` call printed: the text since the
    /// last delta of that stream, at most every 100 ms or so per call and at most
    /// 8 KB (a command that printed more is reported by the tail). The call's
    /// [`UiEvent::ToolResult`] still carries the whole output when it ends. With
    /// [`CAPABILITY_LIVE_OUTPUT`] only.
    ToolOutputDelta {
        tool_use_id: String,
        stream: OutputStream,
        chunk: String,
    },
    /// A hook command started. `event` is its settings name (`PreToolUse`, `Stop`, ...),
    /// `command` the command shortened with anything that looks like a secret hidden,
    /// `tool_use_id` the tool call it runs for. With [`CAPABILITY_HOOK_PROGRESS`] only.
    HookStarted {
        event: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        matcher: Option<String>,
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_use_id: Option<String>,
    },
    /// A hook command ended: `outcome` is `ok`, `blocked` (exit 2 or a block decision),
    /// `error` or `cancelled`; `message` is the start of what it said, when it said
    /// something. With [`CAPABILITY_HOOK_PROGRESS`] only.
    HookFinished {
        event: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        matcher: Option<String>,
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_use_id: Option<String>,
        outcome: String,
        duration_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// Every file the agent created, changed or deleted in this session, with lines
    /// added and removed since the session first touched it. Sent when the list changes
    /// (after a tool call, after a rewind) and after [`EngineCommand::Capabilities`] when
    /// it is not empty. With [`CAPABILITY_GIT_VIEWS`] only.
    Changes { files: Vec<ChangedFile> },
    /// Answer to `/diff [path]`: the workspace's changes against `HEAD` (staged,
    /// unstaged and untracked), or outside git the agent's changes this session. With
    /// [`CAPABILITY_GIT_VIEWS`]; otherwise a notice with the diff as text.
    Diff {
        /// What was compared, as a heading (`Changes against HEAD on main`).
        title: String,
        files: Vec<DiffFile>,
        /// Files or hunks were left out to keep the event small.
        truncated: bool,
    },
    /// Answer to `/commit` without a message: a drafted message and what a commit would
    /// take. Commit with [`EngineCommand::Commit`] (the message may be edited); nothing
    /// is committed until then. With [`CAPABILITY_GIT_VIEWS`]; otherwise a notice.
    CommitProposal {
        message: String,
        branch: Option<String>,
        files: Vec<ChangedFile>,
        /// Nothing is staged, so the commit takes every change (`git add -A`, which
        /// respects `.gitignore`); otherwise only what is staged.
        stage_all: bool,
    },
    /// Answer to `/rewind`: the session's checkpoints, newest first. Go back with
    /// [`EngineCommand::Rewind`]. With [`CAPABILITY_GIT_VIEWS`]; otherwise a notice.
    Checkpoints { items: Vec<CheckpointInfo> },
    /// Answer to [`EngineCommand::Rewind`]: what it put back (or, for a dry run, would).
    /// With [`CAPABILITY_GIT_VIEWS`]; otherwise a notice.
    Rewound {
        checkpoint: u64,
        /// `code_and_chat`, `chat_only` or `code_only`.
        mode: String,
        dry_run: bool,
        /// Files written back, and files removed because they did not exist then.
        restored: Vec<String>,
        removed: Vec<String>,
        not_restored: Vec<NotRestoredFile>,
        /// Shell commands run since the checkpoint; what they did was not undone.
        shell_commands: Vec<String>,
        /// Prompts taken out of the conversation (the checkpoint's and every later one):
        /// the UI drops its last `removed_turns` prompts with everything after them. 0
        /// when the chat was kept.
        removed_turns: usize,
        /// The checkpoint's prompt, for the input box ("edit and send again"); `None`
        /// when the chat was kept.
        prompt: Option<String>,
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

/// What a [`UiEvent::Notice`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    /// The model declined to continue the turn's request (a refusal; the
    /// OpenAI-compatible `content_filter`). The message is the one line to show, with
    /// the provider's explanation when it gave one. It is not assistant text.
    Refusal,
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
    /// [`CAPABILITY_QUESTIONS`], [`CAPABILITY_REPORTS`], [`CAPABILITY_LIVE_PROGRESS`] and
    /// [`CAPABILITY_GIT_VIEWS`]. Replaces what an earlier one said; it may come at any
    /// time, also during a turn.
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
    /// Turn the MCP server `name` on or off ([`FEATURE_MCP_TOGGLE`]): a notice and a
    /// fresh [`UiEvent::Extensions`] follow, or an error. Servers restart from the next
    /// turn.
    SetMcpServerEnabled { name: String, enabled: bool },
    /// Ask for the catalog connectors whose id, name, publisher or description contain
    /// every word of `query` (all of them when empty): [`UiEvent::ConnectorCatalog`]
    /// ([`FEATURE_MCP_CATALOG`]).
    ListConnectorCatalog {
        #[serde(default)]
        query: String,
    },
    /// Add the catalog connector `id` as the server `name` (the id when `None`) to the
    /// settings of `scope` (`user`, `project` or `local`; the connector's default when
    /// `None`). `secrets` holds what the user typed for its variables: secrets go to the
    /// OS keychain and never into a settings file. A notice and a fresh
    /// [`UiEvent::Extensions`] follow, or an error. A connector that signs in with OAuth
    /// ([`ConnectorInfo::oauth`]) then opens the browser: a notice with the sign-in link,
    /// later one with the result (and, once signed in, another [`UiEvent::Extensions`]).
    AddMcpServer {
        id: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        scope: Option<String>,
        #[serde(default)]
        secrets: SecretValues,
    },
    /// Remove the server `name` from the settings Neuron writes (those of `scope` when
    /// given), with its keychain secrets and sign-in. A notice and a fresh
    /// [`UiEvent::Extensions`] follow, or an error.
    RemoveMcpServer {
        name: String,
        #[serde(default)]
        scope: Option<String>,
    },
    /// Approve the workspace server `name` (status `needs_approval`) as it is configured
    /// now, as `/mcp approve` does. With `fingerprint` (from [`McpServerPreview`]), only
    /// if it is still configured as that preview showed it: else an error, and it keeps
    /// waiting. A notice and a fresh [`UiEvent::Extensions`] follow, or an error.
    ApproveMcpServer {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fingerprint: Option<String>,
    },
    /// Ask what trusting the workspace would turn on ([`UiEvent::TrustPreview`]).
    TrustPreview,
    /// Read the account's plan and usage again ([`UiEvent::Account`]).
    RefreshAccount,
    /// Commit with this message (after a [`UiEvent::CommitProposal`], or typed by the
    /// user): what is staged, or every change when nothing is. A notice and a `Status`
    /// follow, or an error. `/commit <message>` does the same.
    Commit { message: String },
    /// Go back to a checkpoint from [`UiEvent::Checkpoints`]. `mode` is `code_and_chat`,
    /// `chat_only` or `code_only`; `dry_run` reports what it would do and changes
    /// nothing. Answered with [`UiEvent::Rewound`] (or an error), then `TurnFinished`.
    Rewind {
        checkpoint: u64,
        mode: String,
        #[serde(default)]
        dry_run: bool,
    },
    /// The thinking level of the next turns ([`FEATURE_STATUS_BAR`]); `None` for the
    /// model's default. A `Status` follows, or an `Error`.
    SetReasoningEffort {
        #[serde(default)]
        effort: Option<String>,
    },
    /// `read-only`, `workspace-write`, `danger-full-access` (only when the status says
    /// `full_access_allowed`), or `plan` / `plan-off` for plan mode
    /// ([`FEATURE_STATUS_BAR`]). No notice and no `TurnFinished`: a `Status` follows, or
    /// an `Error`.
    SetPermissionMode { mode: String },
    /// Ask for the local branches ([`UiEvent::Branches`]).
    ListBranches,
    /// Check out `name` (`create`: a new branch from HEAD). Refused while a turn runs or
    /// the working tree has changes. A notice and a `Status` follow, or an `Error`.
    SwitchBranch {
        name: String,
        #[serde(default)]
        create: bool,
    },
}

/// The user's answer to a permission request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PermissionDecision {
    /// Run this call.
    AllowOnce,
    /// Run this call and similar ones for the rest of the session.
    AllowForSession,
    /// Run this call, and save [`PermissionView::project_rules`] so matching calls run
    /// in this project from now on. Sent only when the request offered project rules
    /// (so only to engines that know it).
    AllowForProject,
    /// Do not run it; `note` is passed to the model. With `session`, matching calls
    /// (ones whose asking part starts with the same command prefix, such as `git push`,
    /// or this exact call) are refused for the rest of the session without asking. Added after protocol 1: an older engine ignores
    /// `session` and denies this call only.
    Deny {
        note: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        session: bool,
    },
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
                session: false,
            },
        };
        let json = serde_json::to_value(&reply).unwrap();
        assert_eq!(json["decision"]["kind"], "deny");
        assert!(json["decision"].get("session").is_none(), "{json}");
        assert_eq!(
            serde_json::from_value::<PermissionReply>(json).unwrap(),
            reply
        );
    }

    /// "Deny for this session" and "always allow in this project" are additive: an
    /// older UI's deny has no `session`, and `project_rules` is left out when empty.
    #[test]
    fn session_denials_and_project_rules_are_additive() {
        let deny: PermissionDecision =
            serde_json::from_value(serde_json::json!({ "kind": "deny", "note": null })).unwrap();
        assert_eq!(
            deny,
            PermissionDecision::Deny {
                note: None,
                session: false
            }
        );
        let for_session = PermissionDecision::Deny {
            note: None,
            session: true,
        };
        let json = serde_json::to_value(&for_session).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "kind": "deny", "note": null, "session": true })
        );
        assert_eq!(
            serde_json::to_value(PermissionDecision::AllowForProject).unwrap(),
            serde_json::json!({ "kind": "allow_for_project" })
        );

        let mut view = PermissionView {
            tool: "bash".into(),
            current_mode: "workspace-write".into(),
            required_mode: "danger-full-access".into(),
            reason: None,
            input: r#"{"command":"docker ps"}"#.into(),
            session_prefixes: vec!["docker ps".into()],
            project_rules: Vec::new(),
        };
        let json = serde_json::to_value(&view).unwrap();
        assert!(json.get("project_rules").is_none(), "{json}");
        assert_eq!(
            serde_json::from_value::<PermissionView>(json).unwrap(),
            view
        );
        view.project_rules = vec!["bash(docker:*)".into()];
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["project_rules"], serde_json::json!(["bash(docker:*)"]));
    }

    /// A refusal is a notice with a kind; a plain notice is written as before.
    #[test]
    fn refusal_notices_carry_their_kind_and_plain_ones_do_not() {
        let refusal = UiEvent::Notice {
            message: "The model declined to continue this request.".into(),
            kind: Some(NoticeKind::Refusal),
        };
        let json = serde_json::to_value(&refusal).unwrap();
        assert_eq!(json["type"], "notice");
        assert_eq!(json["kind"], "refusal");
        assert_eq!(serde_json::from_value::<UiEvent>(json).unwrap(), refusal);

        let plain = UiEvent::Notice {
            message: "Interrupting".into(),
            kind: None,
        };
        assert_eq!(
            serde_json::to_string(&plain).unwrap(),
            r#"{"type":"notice","message":"Interrupting"}"#
        );
        let older: UiEvent =
            serde_json::from_str(r#"{"type":"notice","message":"Interrupting"}"#).unwrap();
        assert_eq!(older, plain);
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
                request_tokens_estimate: None,
                provider: None,
                reasoning_effort: None,
                reasoning_efforts: None,
                full_access_allowed: false,
                context: None,
                ..
            }
        ));
    }

    /// Protocol 1 additions: engine features (absent from older engines), the commands
    /// they add, and their events, as JSON lines.
    #[test]
    #[allow(clippy::too_many_lines)]
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
            (
                r#"{"kind":"command","type":"set_mcp_server_enabled","name":"docs","enabled":false}"#,
                EngineCommand::SetMcpServerEnabled {
                    name: "docs".into(),
                    enabled: false,
                },
            ),
            (
                r#"{"kind":"command","type":"list_connector_catalog"}"#,
                EngineCommand::ListConnectorCatalog {
                    query: String::new(),
                },
            ),
            (
                r#"{"kind":"command","type":"add_mcp_server","id":"github","scope":"user","secrets":{"GITHUB_PERSONAL_ACCESS_TOKEN":"ghp_x"}}"#,
                EngineCommand::AddMcpServer {
                    id: "github".into(),
                    name: None,
                    scope: Some("user".into()),
                    secrets: SecretValues(BTreeMap::from([(
                        "GITHUB_PERSONAL_ACCESS_TOKEN".to_string(),
                        "ghp_x".to_string(),
                    )])),
                },
            ),
            (
                r#"{"kind":"command","type":"add_mcp_server","id":"fetch"}"#,
                EngineCommand::AddMcpServer {
                    id: "fetch".into(),
                    name: None,
                    scope: None,
                    secrets: SecretValues::default(),
                },
            ),
            (
                r#"{"kind":"command","type":"remove_mcp_server","name":"github"}"#,
                EngineCommand::RemoveMcpServer {
                    name: "github".into(),
                    scope: None,
                },
            ),
            (
                r#"{"kind":"command","type":"approve_mcp_server","name":"docs"}"#,
                EngineCommand::ApproveMcpServer {
                    name: "docs".into(),
                    fingerprint: None,
                },
            ),
            (
                r#"{"kind":"command","type":"approve_mcp_server","name":"docs","fingerprint":"ab12"}"#,
                EngineCommand::ApproveMcpServer {
                    name: "docs".into(),
                    fingerprint: Some("ab12".into()),
                },
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

    /// Live progress, todos, sub-agents, changes and checkpoints: the tags and field
    /// names the UIs read, and `ready` from an engine before `features`.
    /// A running command's output, as the UI reads it.
    #[test]
    fn tool_output_delta_as_json() {
        let delta = UiEvent::ToolOutputDelta {
            tool_use_id: "call_7".into(),
            stream: OutputStream::Stderr,
            chunk: "warning: unused\n".into(),
        };
        let json = serde_json::to_value(&delta).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "tool_output_delta",
                "tool_use_id": "call_7",
                "stream": "stderr",
                "chunk": "warning: unused\n",
            })
        );
        assert_eq!(serde_json::from_value::<UiEvent>(json).unwrap(), delta);
        assert!(ENGINE_FEATURES.contains(&FEATURE_LIVE_OUTPUT));
        assert_eq!(CAPABILITY_LIVE_OUTPUT, FEATURE_LIVE_OUTPUT);
    }

    #[test]
    fn live_progress_and_git_view_events_as_json() {
        let ready: UiEvent =
            serde_json::from_str(r#"{"type":"ready","protocol":1,"version":"7.0.0"}"#).unwrap();
        assert_eq!(
            ready,
            UiEvent::Ready {
                protocol: 1,
                version: "7.0.0".into(),
                features: Vec::new(),
            }
        );

        let progress = serde_json::to_value(UiEvent::ToolInputProgress {
            id: "call_1".into(),
            name: "write_file".into(),
            bytes: 2048,
            lines: 40,
            target: Some("shop/style.css".into()),
            preview: Some("body {\n  margin: 0;".into()),
        })
        .unwrap();
        assert_eq!(progress["type"], "tool_input_progress");
        assert_eq!(progress["target"], "shop/style.css");
        assert_eq!(
            serde_json::to_value(UiEvent::OutputProgress { output_tokens: 12 }).unwrap(),
            serde_json::json!({"type": "output_progress", "output_tokens": 12})
        );

        let todos = serde_json::to_value(UiEvent::Todos {
            items: vec![TodoItem {
                content: "Write the CSS".into(),
                status: "in_progress".into(),
                active_form: Some("Writing the CSS".into()),
            }],
        })
        .unwrap();
        assert_eq!(todos["items"][0]["active_form"], "Writing the CSS");

        let rewound = UiEvent::Rewound {
            checkpoint: 3,
            mode: "code_only".into(),
            dry_run: true,
            restored: vec!["src/app.ts".into()],
            removed: Vec::new(),
            not_restored: vec![NotRestoredFile {
                path: "dist/app.js".into(),
                reason: "created by a shell command; left in place".into(),
            }],
            shell_commands: vec!["npm run build".into()],
            removed_turns: 0,
            prompt: None,
        };
        let json = serde_json::to_value(&rewound).unwrap();
        assert_eq!(json["type"], "rewound");
        assert_eq!(serde_json::from_value::<UiEvent>(json).unwrap(), rewound);

        let checkpoint = serde_json::to_value(CheckpointInfo {
            id: 1,
            created_at_ms: 5,
            prompt_preview: "fix it".into(),
            files: Vec::new(),
            shell_commands: Vec::new(),
            before_rewind_to: None,
        })
        .unwrap();
        assert!(checkpoint.get("before_rewind_to").is_none());

        assert_eq!(
            serde_json::from_str::<ClientMessage>(
                r#"{"kind":"command","type":"capabilities","features":["live_progress","git_views"]}"#
            )
            .unwrap(),
            ClientMessage::Command(EngineCommand::Capabilities {
                features: vec![CAPABILITY_LIVE_PROGRESS.into(), CAPABILITY_GIT_VIEWS.into()],
                client: None,
            })
        );
        assert_eq!(
            serde_json::from_str::<EngineCommand>(
                r#"{"type":"rewind","checkpoint":2,"mode":"chat_only"}"#
            )
            .unwrap(),
            EngineCommand::Rewind {
                checkpoint: 2,
                mode: "chat_only".into(),
                dry_run: false,
            }
        );
        assert_eq!(
            serde_json::to_value(EngineCommand::Commit {
                message: "Add the shop page".into()
            })
            .unwrap()["type"],
            "commit"
        );
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
        assert_eq!(preview["mcp_servers"][0]["fingerprint"], "4f1c0de5");
        assert_eq!(preview["permission_rules"][0]["effect"], "allow");

        // A preview from an engine before `command` and `fingerprint` still reads.
        let older: McpServerPreview = serde_json::from_str(
            r#"{"name":"docs","transport":"stdio","target":"npx","source":"/w/.mcp.json"}"#,
        )
        .unwrap();
        assert_eq!((older.command, older.fingerprint), (None, None));
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

    #[test]
    fn hook_events_have_a_stable_shape_and_omit_absent_fields() {
        let started = UiEvent::HookStarted {
            event: "PreToolUse".into(),
            matcher: Some("bash".into()),
            command: "guard.sh".into(),
            tool_use_id: Some("call-1".into()),
        };
        assert_eq!(
            serde_json::to_value(&started).unwrap(),
            serde_json::json!({"type":"hook_started","event":"PreToolUse","matcher":"bash","command":"guard.sh","tool_use_id":"call-1"})
        );
        let finished = UiEvent::HookFinished {
            event: "Stop".into(),
            matcher: None,
            command: "check.sh".into(),
            tool_use_id: None,
            outcome: "blocked".into(),
            duration_ms: 12,
            message: Some("tests fail".into()),
        };
        let value = serde_json::to_value(&finished).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type":"hook_finished","event":"Stop","command":"check.sh","outcome":"blocked","duration_ms":12,"message":"tests fail"})
        );
        assert_eq!(serde_json::from_value::<UiEvent>(value).unwrap(), finished);
        assert!(ENGINE_FEATURES.contains(&FEATURE_HOOK_PROGRESS));
        assert_eq!(CAPABILITY_HOOK_PROGRESS, "hook_progress");
    }

    /// The token window is `AccountDetails.usage_window`, every part optional, in the
    /// gateway's field order: the shape the full-screen footer reads.
    #[test]
    fn the_usage_window_rides_on_the_account_with_every_part_optional() {
        let window: UsageWindow = serde_json::from_str(
            r#"{"tokens":5,"limit_tokens":null,"remaining_tokens":null,"resets_at":null,"window_seconds":18000}"#,
        )
        .unwrap();
        assert_eq!(
            window,
            UsageWindow {
                tokens: Some(5),
                window_seconds: Some(18_000),
                ..UsageWindow::default()
            }
        );
        assert_eq!(
            serde_json::from_str::<UsageWindow>("{}").unwrap(),
            UsageWindow::default()
        );
        assert_eq!(
            serde_json::to_string(&UsageWindow::default()).unwrap(),
            r#"{"tokens":null,"limit_tokens":null,"remaining_tokens":null,"resets_at":null,"window_seconds":null}"#
        );

        let UiEvent::Account {
            account: Some(free),
            ..
        } = crate::fake_engine::samples::free_account()
        else {
            panic!("a signed-in account");
        };
        let json = serde_json::to_value(&free).unwrap();
        assert_eq!(json["usage_window"]["limit_tokens"], 1_000_000);
        assert!(json.get("window").is_none());
        let mut older = json.clone();
        older.as_object_mut().unwrap().remove("usage_window");
        let older: AccountDetails = serde_json::from_value(older).unwrap();
        assert_eq!(older.usage_window, None, "an older engine sends none");
    }

    #[test]
    fn connector_catalog_messages_round_trip_and_secrets_never_reach_debug_output() {
        let command = EngineCommand::AddMcpServer {
            id: "github".into(),
            name: Some("gh".into()),
            scope: Some("project".into()),
            secrets: SecretValues(BTreeMap::from([(
                "GITHUB_PERSONAL_ACCESS_TOKEN".to_string(),
                "ghp_SENTINEL_protocol".to_string(),
            )])),
        };
        let debug = format!("{command:?}");
        assert!(!debug.contains("ghp_SENTINEL_protocol"), "{debug}");
        assert!(debug.contains("GITHUB_PERSONAL_ACCESS_TOKEN"), "{debug}");
        let json = serde_json::to_value(&command).unwrap();
        assert_eq!(
            json["secrets"]["GITHUB_PERSONAL_ACCESS_TOKEN"],
            "ghp_SENTINEL_protocol"
        );
        assert_eq!(
            serde_json::from_value::<EngineCommand>(json).unwrap(),
            command
        );

        let event = UiEvent::ConnectorCatalog {
            version: "2026-11-04.1".into(),
            connectors: vec![ConnectorInfo {
                id: "github".into(),
                name: "GitHub".into(),
                description: "Repositories".into(),
                publisher: "GitHub".into(),
                official: true,
                licence: "MIT".into(),
                source: "https://github.com/github/github-mcp-server".into(),
                transport: "http".into(),
                pinned: "https://api.githubcopilot.com/mcp/".into(),
                oauth: false,
                default_scope: "user".into(),
                variables: vec![ConnectorVariable {
                    name: "GITHUB_PERSONAL_ACCESS_TOKEN".into(),
                    label: "GitHub personal access token".into(),
                    secret: true,
                    required: true,
                    default: None,
                    help_url: None,
                }],
                added: vec!["gh".into()],
            }],
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "connector_catalog");
        assert_eq!(serde_json::from_value::<UiEvent>(json).unwrap(), event);

        // An older engine's server rows have no `catalog`, and a row without one sends none.
        let row: McpServerInfo = serde_json::from_str(
            r#"{"name":"docs","scope":"user","transport":"stdio","status":"ready","tools":[],"error":null}"#,
        )
        .unwrap();
        assert_eq!(row.catalog, None);
        assert!(serde_json::to_value(&row).unwrap().get("catalog").is_none());
        assert!(ENGINE_FEATURES.contains(&FEATURE_MCP_CATALOG));
    }

    #[test]
    fn a_required_server_row_says_so_and_older_rows_are_not_required() {
        let older: McpServerInfo = serde_json::from_str(
            r#"{"name":"docs","scope":"user","transport":"stdio","status":"ready","tools":[],"error":null}"#,
        )
        .unwrap();
        assert!(!older.required);
        assert!(serde_json::to_value(&older)
            .unwrap()
            .get("required")
            .is_none());

        let required = McpServerInfo {
            name: "codebase-memory".into(),
            scope: "managed".into(),
            transport: "stdio".into(),
            status: "ready".into(),
            tools: Vec::new(),
            error: None,
            enabled: true,
            catalog: Some("codebase-memory".into()),
            required: true,
            unpinned: false,
            connect_url: None,
            tool_tiers: BTreeMap::new(),
        };
        let json = serde_json::to_value(&required).unwrap();
        assert_eq!(json["required"], true);
        assert_eq!(json["scope"], "managed");
        assert!(json.get("unpinned").is_none());
        assert_eq!(
            serde_json::from_value::<McpServerInfo>(json).unwrap(),
            required
        );
        assert!(ENGINE_FEATURES.contains(&FEATURE_MCP_POLICY));
    }

    #[test]
    fn an_unpinned_server_row_says_so_and_older_rows_are_not_unpinned() {
        let older: McpServerInfo = serde_json::from_str(
            r#"{"name":"notes","scope":"user","transport":"stdio","status":"ready","tools":[],"error":null}"#,
        )
        .unwrap();
        assert!(!older.unpinned);
        let unpinned = McpServerInfo {
            unpinned: true,
            ..older
        };
        let json = serde_json::to_value(&unpinned).unwrap();
        assert_eq!(json["unpinned"], true);
        assert_eq!(
            serde_json::from_value::<McpServerInfo>(json).unwrap(),
            unpinned
        );
    }

    #[test]
    fn a_catalog_row_carries_its_tool_tiers_and_older_rows_none() {
        let older: McpServerInfo = serde_json::from_str(
            r#"{"name":"github","scope":"user","transport":"http","status":"ready","tools":["get_me"],"error":null}"#,
        )
        .unwrap();
        assert!(older.tool_tiers.is_empty());
        assert!(serde_json::to_value(&older)
            .unwrap()
            .get("tool_tiers")
            .is_none());
        let tiered = McpServerInfo {
            catalog: Some("github".into()),
            tool_tiers: BTreeMap::from([
                ("get_me".to_string(), "read".to_string()),
                ("merge_pull_request".to_string(), "destructive".to_string()),
            ]),
            ..older
        };
        let json = serde_json::to_value(&tiered).unwrap();
        assert_eq!(json["tool_tiers"]["merge_pull_request"], "destructive");
        assert_eq!(
            serde_json::from_value::<McpServerInfo>(json).unwrap(),
            tiered
        );
    }

    #[test]
    fn a_hosted_connector_row_carries_its_connect_page_and_older_rows_none() {
        let older: McpServerInfo = serde_json::from_str(
            r#"{"name":"github","scope":"managed","transport":"managed_proxy","status":"failed","tools":[],"error":null}"#,
        )
        .unwrap();
        assert_eq!(older.connect_url, None);
        assert!(serde_json::to_value(&older)
            .unwrap()
            .get("connect_url")
            .is_none());
        let waiting = McpServerInfo {
            transport: "zerox".into(),
            status: "needs_connect".into(),
            error: Some("not linked".into()),
            connect_url: Some("https://zero-x.live/dashboard#connectors".into()),
            ..older
        };
        let json = serde_json::to_value(&waiting).unwrap();
        assert_eq!(
            json["connect_url"],
            "https://zero-x.live/dashboard#connectors"
        );
        assert_eq!(
            serde_json::from_value::<McpServerInfo>(json).unwrap(),
            waiting
        );
        assert!(ENGINE_FEATURES.contains(&FEATURE_MCP_HOSTED));
    }
    /// The status bar's fields and commands ([`FEATURE_STATUS_BAR`]): they round-trip, and
    /// an older engine's JSON without them reads as their defaults.
    #[test]
    fn status_bar_fields_and_commands_round_trip() {
        assert!(ENGINE_FEATURES.contains(&FEATURE_STATUS_BAR));
        for (line, command) in [
            (
                r#"{"kind":"command","type":"set_reasoning_effort","effort":"high"}"#,
                EngineCommand::SetReasoningEffort {
                    effort: Some("high".into()),
                },
            ),
            (
                r#"{"kind":"command","type":"set_reasoning_effort"}"#,
                EngineCommand::SetReasoningEffort { effort: None },
            ),
            (
                r#"{"kind":"command","type":"set_permission_mode","mode":"plan"}"#,
                EngineCommand::SetPermissionMode {
                    mode: "plan".into(),
                },
            ),
            (
                r#"{"kind":"command","type":"list_branches"}"#,
                EngineCommand::ListBranches,
            ),
            (
                r#"{"kind":"command","type":"switch_branch","name":"main"}"#,
                EngineCommand::SwitchBranch {
                    name: "main".into(),
                    create: false,
                },
            ),
        ] {
            assert_eq!(
                serde_json::from_str::<ClientMessage>(line).unwrap(),
                ClientMessage::Command(command.clone())
            );
            let back = serde_json::to_string(&ClientMessage::Command(command.clone())).unwrap();
            assert_eq!(
                serde_json::from_str::<ClientMessage>(&back).unwrap(),
                ClientMessage::Command(command)
            );
        }

        let status: UiEvent = serde_json::from_str(
            r#"{"type":"status","model":"gemini-2.5-flash","permission_mode":"workspace-write","session_id":"s","cwd":".","workspace_trusted":true,"plan_mode":false,"context_percent":4,"git":null,"provider":"gemini","model_display_name":"Gemini 2.5 Flash","reasoning_effort":"high","reasoning_efforts":["none","low","medium","high"],"full_access_allowed":true,"context":{"context_window":1000,"output_reserve":100,"system_tokens":10,"tool_schema_tokens":20,"tool_count":3,"instruction_tokens":5,"instruction_files":["NEURON.md"],"conversation_tokens":40,"tool_output_tokens":25,"used_tokens":100,"threshold_tokens":720}}"#,
        )
        .unwrap();
        let UiEvent::Status {
            provider,
            reasoning_efforts,
            full_access_allowed,
            context,
            ..
        } = &status
        else {
            panic!("a status");
        };
        assert_eq!(provider.as_deref(), Some("gemini"));
        assert_eq!(reasoning_efforts.as_ref().map(Vec::len), Some(4));
        assert!(*full_access_allowed);
        assert_eq!(
            context.as_ref().map(|context| context.used_tokens),
            Some(100)
        );
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(serde_json::from_value::<UiEvent>(json).unwrap(), status);

        let branches = UiEvent::Branches {
            current: None,
            branches: vec![BranchInfo {
                name: "main".into(),
                upstream: Some("origin/main".into()),
                ahead: Some(1),
                behind: Some(0),
                last_commit_ms: Some(1),
            }],
            dirty: true,
        };
        let json = serde_json::to_value(&branches).unwrap();
        assert_eq!(json["type"], "branches");
        assert_eq!(serde_json::from_value::<UiEvent>(json).unwrap(), branches);

        let window: UsageWindow = serde_json::from_str(r#"{"tokens":5,"limit_tokens":null,"remaining_tokens":null,"resets_at":null,"window_seconds":18000}"#).unwrap();
        assert_eq!(window.window_seconds, Some(18_000));
        let model: ModelInfo = serde_json::from_str(
            r#"{"id":"m","display_name":null,"available":true,"unavailable_reason":null,"context_window":null,"tiers":[]}"#,
        )
        .unwrap();
        assert_eq!(
            (
                model.provider,
                model.reasoning_efforts,
                model.supports_reasoning
            ),
            (None, None, None)
        );
    }
}
