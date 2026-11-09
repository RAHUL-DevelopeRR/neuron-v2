/**
 * The JSON-lines contract of `neuron engine --stdio`, mirrored from
 * `rust/crates/neuron-tui/src/protocol.rs`. Every tag and field name here is
 * what serde writes there; `test/protocol.test.ts` reads protocol.rs and fails
 * when the two drift apart.
 *
 * - Engine → client: one {@link UiEvent} per line, tagged by `type`.
 * - Client → engine: one {@link ClientMessage} per line, tagged by `kind`.
 *
 * serde writes `Option::None` as `null`, so optional fields arrive as
 * `T | null`. Fields added after protocol 1 shipped are optional here: older
 * engines leave them out.
 *
 * Protocol v2 is a set of engine features (`ready.features`): `reports`,
 * `account`, `error_details`, `session_scope`, `trust_preview`,
 * `model_images`, `live_progress`, `live_output`, `git_views`,
 * `checkpoints` and `mcp_toggle`. Engine 7.0.0-alpha.5 adds `hook_progress`,
 * `mcp_catalog`, `mcp_policy`, `mcp_hosted` and `status_bar`. Send the
 * commands a feature adds only when the engine lists it.
 *
 * Being an {@link EngineCommand} says nothing about who may send it: a frame
 * from a remote peer meets {@link parseClientMessage} first, which refuses
 * every command that changes something.
 */

/** `PROTOCOL_VERSION` in protocol.rs, sent in {@link ReadyEvent}. */
export const PROTOCOL_VERSION = 1

export interface TokenCounts {
  input: number
  output: number
  cache_read: number
  cache_write: number
}

/** What a tool call needs approval for. */
export interface PermissionView {
  tool: string
  current_mode: string
  required_mode: string
  reason: string | null
  /** The tool input as JSON text. */
  input: string
  /** What "allow for this session" approves from now on; empty when only once is possible. */
  session_prefixes: string[]
  /**
   * The `permissions.allow` rules "always allow in this project" would save to the project's
   * `.neuron/settings.local.json`. Older engines leave it out, and the choice is offered only when it is not empty.
   */
  project_rules?: string[]
}

export interface GitStatus {
  branch: string | null
  changed: number
  staged: number
  unstaged: number
  untracked: number
  conflicted: number
}

export type ModelStatus = 'available' | 'degraded' | 'busy' | 'blurred'

export interface ModelInfo {
  id: string
  display_name: string | null
  available: boolean
  unavailable_reason: string | null
  context_window: number | null
  tiers: string[]
  status?: ModelStatus | null
  available_again_at?: string | null
  /** The catalog says the model reads images (feature `model_images`); absent when it does not say. */
  supports_images?: boolean | null
  /** Who serves it (the gateway's `owned_by` or `provider`: `gemini`, `groq`, `zero-x` for `auto`, ...), feature `status_bar`. */
  provider?: string | null
  /** The thinking levels it takes, lowest first (feature `status_bar`). */
  reasoning_efforts?: string[] | null
  /** The catalog says it reasons (feature `status_bar`); absent or null when it does not say. */
  supports_reasoning?: boolean | null
}

/** The Zero-X login as a client may show it. Never carries the token. */
export interface AccountInfo {
  email: string | null
  plan: string | null
  source: string
  expires_at: number | null
}

/**
 * Features an engine adds to protocol 1 (`ENGINE_FEATURES` in protocol.rs). Send
 * the commands a feature adds only when `ready.features` lists it.
 */
export const FEATURE_CAPABILITIES = 'capabilities'
export const FEATURE_SESSIONS = 'sessions'
export const FEATURE_TRUST_WORKSPACE = 'trust_workspace'
export const FEATURE_EXTENSIONS = 'extensions'
/** `report` events for `/usage`, `/init`, `/memory` and `/doctor` (with the `reports` capability). */
export const FEATURE_REPORTS = 'reports'
/** `account` events and the `refresh_account` command. */
export const FEATURE_ACCOUNT = 'account'
/** `detail` on `error` events. */
export const FEATURE_ERROR_DETAILS = 'error_details'
/** `list_sessions` takes a `scope`; session rows say where they belong and what they are doing. */
export const FEATURE_SESSION_SCOPE = 'session_scope'
/** The `trust_preview` command and event. */
export const FEATURE_TRUST_PREVIEW = 'trust_preview'
/** `supports_images` on models and on `status`. */
export const FEATURE_MODEL_IMAGES = 'model_images'

/** The engine reports a turn while the model writes it: `tool_input_started`, `tool_input_progress`, `output_progress` and `request_usage` (with the `live_progress` capability). */
export const FEATURE_LIVE_PROGRESS = 'live_progress'
/** `changes`, `diff` and `commit_proposal` events and the `commit` command (with the `git_views` capability; otherwise notices). */
export const FEATURE_GIT_VIEWS = 'git_views'
/** `tool_output_delta` events: what a running shell command prints (with the `live_output` capability). */
export const FEATURE_LIVE_OUTPUT = 'live_output'
/** A checkpoint per prompt: `/rewind` lists them (`checkpoints`), the `rewind` command goes back to one (`rewound`). Needs the `git_views` capability; otherwise notices. */
export const FEATURE_CHECKPOINTS = 'checkpoints'
/** The `set_mcp_server_enabled` command; `McpServerInfo.enabled` is set and turned-off servers have status `disabled`. */
export const FEATURE_MCP_TOGGLE = 'mcp_toggle'
/** `todos` events. */
export const FEATURE_TODOS = 'todos'
/** `subagent_started` and `subagent_finished` events. */
export const FEATURE_SUBAGENTS = 'subagents'
/** `hook_started` and `hook_finished` events while hook commands run (with the `hook_progress` capability). */
export const FEATURE_HOOK_PROGRESS = 'hook_progress'
/**
 * The connector catalog: the `list_connector_catalog` command (a `connector_catalog` event answers),
 * `add_mcp_server`, `remove_mcp_server` and `approve_mcp_server` (each answered with a notice and a fresh
 * `extensions` event, or an error), and `catalog` on MCP servers.
 */
export const FEATURE_MCP_CATALOG = 'mcp_catalog'
/**
 * The connector policy (managed settings and the Zero-X plan): MCP servers with scope `managed`, status
 * `blocked` with the policy's reason in `error`, and `required` on servers that cannot be turned off.
 */
export const FEATURE_MCP_POLICY = 'mcp_policy'
/**
 * Hosted connectors through Zero-X: MCP servers with transport `zerox`, status `needs_connect` while the
 * connector is not linked to the Zero-X account, and `connect_url`, the page that links it.
 */
export const FEATURE_MCP_HOSTED = 'mcp_hosted'
/**
 * The prompt status bar's engine side: `provider`, `model_display_name`, `reasoning_effort(s)`,
 * `full_access_allowed` and `context` on `status`; `provider`, `reasoning_efforts` and `supports_reasoning`
 * on models; the `set_reasoning_effort`, `set_permission_mode`, `list_branches` and `switch_branch` commands
 * and the `branches` event; `/effort` and a `/context` report.
 */
export const FEATURE_STATUS_BAR = 'status_bar'

/** A UI capability: questions come as `question` events instead of a "Question:" notice. */
export const CAPABILITY_QUESTIONS = 'questions'
/** A UI capability: reports come as `report` events instead of a notice. */
export const CAPABILITY_REPORTS = 'reports'
/** A UI capability: send the live-progress events of {@link FEATURE_LIVE_PROGRESS}. */
export const CAPABILITY_LIVE_PROGRESS = 'live_progress'
/** A UI capability: send changes, diffs, commit proposals, checkpoints and rewinds as their own events instead of notices. */
export const CAPABILITY_GIT_VIEWS = 'git_views'
/** A UI capability: send the output of a running shell command as `tool_output_delta` events. */
export const CAPABILITY_LIVE_OUTPUT = 'live_output'
/** A UI capability: send `hook_started` and `hook_finished` events. Without it the engine sends neither. */
export const CAPABILITY_HOOK_PROGRESS = 'hook_progress'

/**
 * What the desktop app renders and so declares in its `capabilities` command. `hook_progress` is not
 * here: no screen shows hook commands yet, and an engine only sends the events to a UI that asks.
 */
export const DESKTOP_CAPABILITIES: readonly string[] = [
  CAPABILITY_QUESTIONS,
  CAPABILITY_REPORTS,
  CAPABILITY_LIVE_PROGRESS,
  CAPABILITY_LIVE_OUTPUT,
  CAPABILITY_GIT_VIEWS,
]

/** Which stream of a command a `tool_output_delta` carries. */
export type OutputStream = 'stdout' | 'stderr'

/** What a notice is about: `refusal`, the model declined to continue the request. */
export type NoticeKind = 'refusal'

/** What a rewind puts back: the files and the chat, only the chat, or only the files. */
export type RewindMode = 'code_and_chat' | 'chat_only' | 'code_only'
export const REWIND_MODES: readonly RewindMode[] = ['code_and_chat', 'chat_only', 'code_only']

/** Which saved chats `list_sessions` lists: this workspace's, or every workspace's on this device. */
export type SessionScope = 'project' | 'all'

/** `idle`: no other Neuron has it open; `open`: another one has it open; `running`: another one runs a turn in it. */
export type SessionRunStatus = 'idle' | 'open' | 'running'

/** One saved chat. */
export interface SessionSummary {
  id: string
  /** The first prompt, on one line and shortened. */
  title: string | null
  model: string | null
  message_count: number
  /** Unix milliseconds. */
  updated_at_ms: number
  current: boolean
  /** The branch name of a forked session. */
  branch: string | null
  /** The workspace folder it belongs to (feature `session_scope`). */
  project?: string | null
  /** The git branch when it last ran. */
  git_branch?: string | null
  /** The current session is `idle` here: this engine's own state is in `status`. */
  run_status?: SessionRunStatus
  /** Its last turn failed. */
  failed?: boolean
  /** The Neuron that last ran it (`terminal`, `desktop`), when known. */
  client?: string | null
}

/** A file and how it changed, with line counts. */
export interface ChangedFile {
  /** Relative to the workspace root, with `/`; absolute for files outside it. */
  path: string
  /** `created`, `modified`, `deleted` or `renamed`. */
  status: 'created' | 'modified' | 'deleted' | 'renamed' | string
  /** Lines added and removed (0 for binary files). */
  added: number
  removed: number
}

/** One file of a `diff` event. */
export interface DiffFile {
  /** Relative to the workspace root, with `/`. */
  path: string
  /** `created`, `modified`, `deleted` or `renamed`. */
  status: 'created' | 'modified' | 'deleted' | 'renamed' | string
  added: number
  removed: number
  /** The unified diff's hunks (from the first `@@` line on); empty for binary files and files left out to keep the event small. */
  patch: string
  binary: boolean
}

/** One entry of the "go back to" list (`checkpoints` event). */
export interface CheckpointInfo {
  id: number
  /** When the prompt was sent (Unix milliseconds). */
  created_at_ms: number
  /** The prompt, shortened to one line. */
  prompt_preview: string
  /** Files the agent's file tools changed in that turn (workspace-relative, with `/`). */
  files: string[]
  /** Shell commands that turn ran; a rewind cannot undo what they did. */
  shell_commands: string[]
  /** Set on the checkpoint a code-only rewind saves first: the checkpoint that rewind went back to, so the rewind itself can be undone. */
  before_rewind_to?: number | null
}

/** A file a rewind could not put back, and why (as people read it). */
export interface NotRestoredFile {
  path: string
  reason: string
}

/** One part of a `report` event. */
export interface ReportSection {
  heading: string | null
  /** GitHub-flavoured Markdown. */
  markdown: string
}

export interface ReportLink {
  label: string
  url: string
}

/** Requests and tokens: a plan's daily limits, or what was used today. */
export interface UsageAmounts {
  requests: number | null
  tokens: number | null
}

/** The Zero-X account for the account row, menu and usage panel. Never carries the token. */
export interface AccountDetails {
  email: string | null
  name: string | null
  /** The plan's id: `free`, `pro`, `max`, ... */
  plan: string | null
  /** The plan as people read it ("Pro"). */
  plan_name: string | null
  source: string
  /** Unix seconds. */
  expires_at: number | null
  limits: UsageAmounts | null
  usage_today: UsageAmounts | null
  /** ISO 8601, UTC. */
  resets_at: string | null
  credit_balance_cents: number | null
  suspended: boolean
  dashboard_url: string
  pricing_url: string
  /** When the plan and usage were last read (Unix seconds); null when only the stored login is known. */
  usage_updated_at: number | null
  /** Why the plan and usage could not be read. */
  usage_error: string | null
  /** The plan's rolling token window (Zero-X Free: 1M tokens per 5 hours); absent for a plan without one and from older engines. */
  usage_window?: UsageWindow | null
}

/** A rolling usage window (`usage_window` in `/api/account`). Every part may be missing. */
export interface UsageWindow {
  /** Tokens used in the window. */
  tokens: number | null
  limit_tokens: number | null
  remaining_tokens: number | null
  /** When the oldest usage leaves the window (ISO 8601, UTC). */
  resets_at: string | null
  /** The window's length: 18000 for five hours. */
  window_seconds: number | null
}

/**
 * What fills the context window (feature `status_bar`), as the engine's estimated tokens. `used_tokens` is
 * what each request sends (system prompt, tool schemas, instructions and the conversation);
 * `context_window - output_reserve` is what it may use.
 */
export interface ContextBreakdown {
  context_window: number
  /** Kept free for the reply (`max_tokens`). */
  output_reserve: number
  /** The system prompt without the project instructions. */
  system_tokens: number
  tool_schema_tokens: number
  tool_count: number
  /** `NEURON.md` and the other instruction files and memory. */
  instruction_tokens: number
  instruction_files: string[]
  /** Prompts, replies and tool calls. */
  conversation_tokens: number
  /** Tool results. */
  tool_output_tokens: number
  used_tokens: number
  /** Auto-compaction starts when `used_tokens` reaches this. */
  threshold_tokens: number
}

/** One local branch, for a branch picker (`branches` event, feature `status_bar`). */
export interface BranchInfo {
  name: string
  /** Its upstream (`origin/main`), when it has one. */
  upstream: string | null
  /** Commits ahead of and behind the upstream; null without one. */
  ahead: number | null
  behind: number | null
  /** Its last commit (Unix milliseconds). */
  last_commit_ms: number | null
}

/**
 * What a failed request needs from the user. `code` is the gateway's
 * (`invalid_session`, `upgrade_required`, `insufficient_quota`, `rate_limited`,
 * ...), `unreachable` when the server could not be reached, `not_available`
 * when it does not offer the call yet (a 404), or `sign_in_required` when the
 * call needs a Zero-X login and there is none.
 */
export interface ErrorDetail {
  code: string
  status: number | null
  upgrade_url: string | null
  /** ISO 8601, UTC. */
  resets_at: string | null
  retry_after_seconds: number | null
}

/** A hook a workspace's settings would turn on. */
export interface HookPreview {
  event: string
  /** The command it runs, in full (cut only past a very long length). */
  command: string
  source: string
  /** The tool name the hook is limited to; absent when it runs for every tool. */
  matcher?: string
}

/** An MCP server a workspace's settings would start. */
export interface McpServerPreview {
  name: string
  transport: string
  /** The program it starts or the host it connects to; never arguments or headers. */
  target: string | null
  source: string
  /**
   * What runs, with secret-looking values shown as `[REDACTED]`; never
   * environment or headers. Older engines leave it out.
   */
  command?: string
  /**
   * The launch spec's digest as previewed: an `approve_mcp_server` that sends
   * it back is refused if the config changed since. Older engines leave it out.
   */
  fingerprint?: string
}

/** A permission rule a workspace's settings would add. */
export interface PermissionRulePreview {
  effect: 'allow' | 'ask' | string
  rule: string
  source: string
}

/** One entry of a saved conversation. */
export type TranscriptEntry =
  | { kind: 'user'; text: string }
  | { kind: 'assistant'; text: string }
  | { kind: 'tool_call'; id: string; name: string; input: string }
  | { kind: 'tool_result'; id: string; name: string; output: string; is_error: boolean }

/** One item of the agent's task list. */
export interface TodoItem {
  content: string
  status: 'pending' | 'in_progress' | 'completed' | string
  active_form: string | null
}

/** One configured MCP server and what it is doing now. */
export interface McpServerInfo {
  name: string
  /** `bundled`, `user`, `project`, `local` or `managed` (the machine's managed settings, or a server the connector policy requires). */
  scope: string
  /** `stdio`, `http`, `sse`, `ws`, `sdk` or `zerox` (older engines: `managed_proxy`). */
  transport: string
  /**
   * `starting`, `ready`, `failed`, `pending` (started, tools not found),
   * `unsupported` (a transport Neuron does not speak yet), `not_started`,
   * `disabled` (turned off in the settings), `needs_approval` (a workspace server that is new or changed:
   * it starts once the user approves it), `blocked` (a deny rule or the connector policy refuses it) or
   * `needs_connect` (a hosted connector not linked to the Zero-X account yet, see `connect_url`); `error`
   * says why for the last three. Older engines send none of the last three; show an unknown status as is.
   */
  status:
    | 'starting'
    | 'ready'
    | 'failed'
    | 'pending'
    | 'unsupported'
    | 'not_started'
    | 'disabled'
    | 'needs_approval'
    | 'blocked'
    | 'needs_connect'
    | string
  /** Its tools, without the `mcp__<server>__` prefix. */
  tools: string[]
  /** Why it failed or is unsupported. */
  error: string | null
  /** Turned on (feature `mcp_toggle`); absent from older engines, meaning on. */
  enabled?: boolean
  /** The catalog connector it was added from, when its launch spec is exactly that entry's (feature `mcp_catalog`). */
  catalog?: string
  /** The connector policy requires it (feature `mcp_policy`): it is on, and cannot be turned off. Older engines leave it out. */
  required?: boolean
  /**
   * A stdio server that is not a catalog connector and whose version and hashes are not recorded
   * (`/mcp pin <name>` records them). Older engines leave it out.
   */
  unpinned?: boolean
  /** Where the user links this hosted connector, while its status is `needs_connect` (feature `mcp_hosted`). Older engines leave it out. */
  connect_url?: string
  /**
   * The permission tier of each of its tools (`read`, `write`, `destructive` or `external`) as the
   * connector catalog pins it, for a catalog or hosted connector (feature `mcp_catalog`). Other servers,
   * and older engines, leave it out.
   */
  tool_tiers?: Record<string, string>
}

/** A variable a catalog connector needs. */
export interface ConnectorVariable {
  /** The environment variable or `${secret:NAME}` name. */
  name: string
  label: string
  /** Kept in the OS keychain: mask its field, never log it. */
  secret: boolean
  required: boolean
  default: string | null
  /** Where to get it (a token page). */
  help_url: string | null
}

/** One connector of the catalog (`connector_catalog` event). */
export interface ConnectorInfo {
  id: string
  name: string
  description: string
  publisher: string
  /** Published by the service's vendor or the MCP project. */
  official: boolean
  licence: string
  /** Its source repository. */
  source: string
  /** `stdio` (runs locally) or `http` (a remote server). */
  transport: string
  /** `<package>@<version>`, or the URL of a remote server. */
  pinned: string
  /** Signs in with OAuth once added. */
  oauth: boolean
  /** `user`, `project` or `local`. */
  default_scope: string
  variables: ConnectorVariable[]
  /** The configured servers added from it. */
  added: string[]
}

/** One installed plugin. */
export interface PluginInfo {
  id: string
  name: string
  version: string
  description: string
  /** `builtin`, `bundled` or `external`. */
  kind: string
  enabled: boolean
}

/** One skill the agent can load, or one agent definition. */
export interface DefinitionInfo {
  name: string
  description: string | null
  /** Where it comes from, as people read it (Project, User, ...). */
  source: string
  /** False when another definition of the same name wins. */
  active: boolean
}

// ---------------------------------------------------------------- engine → client

export interface ReadyEvent {
  type: 'ready'
  protocol: number
  version: string
  /** What the engine adds to protocol 1; absent from older engines. */
  features?: string[]
}

export interface StatusEvent {
  type: 'status'
  model: string
  permission_mode: string
  session_id: string
  cwd: string
  workspace_trusted: boolean
  plan_mode: boolean
  context_percent: number
  git: GitStatus | null
  signed_in?: boolean
  sign_in_required?: boolean
  account?: AccountInfo | null
  /** The catalog says the current model reads images; hide the image-paste hint unless true. */
  supports_images?: boolean | null
  /**
   * The model that answered the last request, when it is not `model` (`auto` picks
   * one per request; a failover can switch). Absent or null before the first answer,
   * from providers that do not name the model, and from older engines.
   */
  served_model?: string | null
  /** Estimated input tokens of each model request the next turn sends; absent from older engines. */
  request_tokens_estimate?: number | null
  /**
   * How a person reads `served_model`: its catalog name without the gateway's route to it. Sent with
   * `served_model`; absent from older engines.
   */
  served_model_display_name?: string | null
  /** Who serves the model (feature `status_bar`), as on {@link ModelInfo}. */
  provider?: string | null
  /** The catalog's name for the model (feature `status_bar`), without the gateway's route to it from engines after 7.0.0-alpha.4. */
  model_display_name?: string | null
  /** The thinking level the next turn asks for; null for the model's default. */
  reasoning_effort?: string | null
  /** The levels the model takes; null when unknown, empty when it does not reason. */
  reasoning_efforts?: string[] | null
  /** Full access may be picked: the engine started with it, or with `--dangerously-skip-permissions`. Absent from older engines. */
  full_access_allowed?: boolean
  /** What fills the context window (feature `status_bar`); absent from older engines. */
  context?: ContextBreakdown | null
}

export interface SessionStartingEvent {
  type: 'session_starting'
}

export interface SignInCodeEvent {
  type: 'sign_in_code'
  url: string
  url_complete: string | null
  user_code: string
  expires_in_secs: number
  browser_opened: boolean
}

export interface ModelsEvent {
  type: 'models'
  current: string
  models: ModelInfo[]
}

export interface TextDeltaEvent {
  type: 'text_delta'
  text: string
}

export interface TextBlockEndEvent {
  type: 'text_block_end'
}

export interface ReasoningStartedEvent {
  type: 'reasoning_started'
  chars: number | null
  redacted: boolean
}

export interface ReasoningDeltaEvent {
  type: 'reasoning_delta'
  text: string
}

export interface ToolCallStartedEvent {
  type: 'tool_call_started'
  id: string
  name: string
  /** The complete tool input as JSON text. */
  input: string
  /** The output limit truncated the call; it will not run. */
  cut_off: boolean
}

export interface ToolResultEvent {
  type: 'tool_result'
  /** The `tool_call_started` id; absent from engines before ids were added. */
  id?: string
  name: string
  output: string
  is_error: boolean
}

export interface UsageEvent {
  type: 'usage'
  turn: TokenCounts
  cumulative: TokenCounts
  estimated: boolean
}

export interface PermissionRequestedEvent {
  type: 'permission_requested'
  id: number
  request: PermissionView
}

/** ANSI-free; may be multi-line. A question from the agent starts with "Question:". */
export interface NoticeEvent {
  type: 'notice'
  message: string
  /** What the notice is about, when it should be drawn apart from status lines; absent from plain notices and older engines. */
  kind?: NoticeKind | null
}

export interface TurnFinishedEvent {
  type: 'turn_finished'
}

export interface ErrorEvent {
  type: 'error'
  message: string
  /** What the failure needs from the user (feature `error_details`). */
  detail?: ErrorDetail | null
}

/** Answer to `list_sessions`: the saved chats in `scope`, newest first. */
export interface SessionsEvent {
  type: 'sessions'
  current: string
  sessions: SessionSummary[]
  /** Absent from engines before `session_scope`: `project`. */
  scope?: SessionScope
}

/**
 * The conversation of the session the engine now runs, from its start: after
 * `--resume`, `resume`, `/resume` and `/clear`.
 */
export interface TranscriptEvent {
  type: 'transcript'
  session_id: string
  entries: TranscriptEntry[]
  /** The oldest entries were left out. */
  truncated: boolean
}

/** The workspace is trusted now; its settings, hooks and MCP servers load on a restart. */
export interface WorkspaceTrustedEvent {
  type: 'workspace_trusted'
  cwd: string
  restart_required: boolean
  /** The folder trust was recorded for: the git root, or the folder itself. */
  root?: string | null
  /** Trusted for this engine only (the home folder): restart with NEURON_TRUST_WORKSPACE=1 and ask again next launch. */
  session_only?: boolean
}

/** The agent asks something (with the `questions` capability); answer with `answer`. */
export interface QuestionEvent {
  type: 'question'
  id: number
  question: string
  options: string[]
}

/** The agent's whole task list, after its TodoWrite tool ran. */
export interface TodosEvent {
  type: 'todos'
  items: TodoItem[]
}

/** An Agent call began running; `id` is its tool call id. */
export interface SubagentStartedEvent {
  type: 'subagent_started'
  id: string
  description: string
  subagent_type: string | null
}

/** An Agent call ended. */
export interface SubagentFinishedEvent {
  type: 'subagent_finished'
  id: string
  status: string
  is_error: boolean
  result: string | null
  note: string | null
}

/** Answer to `list_extensions`. */
export interface ExtensionsEvent {
  type: 'extensions'
  mcp_servers: McpServerInfo[]
  plugins: PluginInfo[]
  skills: DefinitionInfo[]
  agents: DefinitionInfo[]
}

/** Answer to `list_connector_catalog` (feature `mcp_catalog`): the catalog's version and the connectors that match the query. */
export interface ConnectorCatalogEvent {
  type: 'connector_catalog'
  version: string
  connectors: ConnectorInfo[]
}

/** What a report command found (`/usage`, `/init`, `/memory`, `/doctor`); `turn_finished` follows. */
export interface ReportEvent {
  type: 'report'
  /** The command's name without the slash. */
  command: string
  title: string
  sections: ReportSection[]
  links: ReportLink[]
}

/** The Zero-X account; `account` is null when there is no login. */
export interface AccountEvent {
  type: 'account'
  signed_in: boolean
  account: AccountDetails | null
}

/** Answer to `list_branches` (feature `status_bar`): the local branches, newest commit first. */
export interface BranchesEvent {
  type: 'branches'
  /** The checked-out branch; null on a detached HEAD. */
  current: string | null
  branches: BranchInfo[]
  /** The working tree has uncommitted changes (switching is refused then). */
  dirty: boolean
}

/** Answer to `trust_preview`: what trusting the workspace would turn on. Nothing was started. */
export interface TrustPreviewEvent {
  type: 'trust_preview'
  cwd: string
  /** The folder trust would be recorded for: the git root, or the folder itself. */
  root: string
  /** `root` is the top folder of a git repository (else it is `cwd`, outside git). */
  is_git_root: boolean
  /** Trust would last for this engine only (the home folder, or above it). */
  session_only: boolean
  trusted: boolean
  /** Trusted before, but its hooks, MCP servers or allow rules changed since. */
  changed_since_trust?: boolean
  /** With `changed_since_trust`: the settings keys (`env`, `model`, `hooks`, ...) that differ from what was trusted. */
  changed_settings?: string[]
  settings_files: string[]
  hooks: HookPreview[]
  mcp_servers: McpServerPreview[]
  permission_rules: PermissionRulePreview[]
  /** Other settings the files would apply, by key. */
  other_settings: string[]
}

/** The model started a tool call and is still writing its input: show the call now. `tool_input_progress` follows while the input grows, then `tool_call_started` with the same `id` and the complete input. With the `live_progress` capability only. */
export interface ToolInputStartedEvent {
  type: 'tool_input_started'
  id: string
  name: string
}

/** How far the model got writing a tool call's input; at most every 100 ms or so per call, and once more when the input is complete. */
export interface ToolInputProgressEvent {
  type: 'tool_input_progress'
  id: string
  name: string
  /** Bytes of input (JSON text) so far. */
  bytes: number
  /** Lines of the call's main text so far (a written file's content, an edit's new text, a command); 0 before it starts. */
  lines: number
  /** What the call is about, once the model has written it: a path, a command, a search pattern, a URL. */
  target: string | null
  /** The last lines of the main text so far (a few lines, at most about 1 KB), unescaped. */
  preview: string | null
}

/** Output tokens of the model response now streaming (text, reasoning and tool input), estimated from its length. Starts again from 0 with each model request of a turn. */
export interface OutputProgressEvent {
  type: 'output_progress'
  output_tokens: number
}

/** One model request of the turn ended: its tokens as the provider reported them. `usage` still closes the turn with its totals. */
export interface RequestUsageEvent {
  type: 'request_usage'
  usage: TokenCounts
}

/** More of what a running `bash` or `PowerShell` call printed: the text since the last delta of that stream, at most 8 KB each. The call's `tool_result` still carries the whole output. With the `live_output` capability only. */
export interface ToolOutputDeltaEvent {
  type: 'tool_output_delta'
  tool_use_id: string
  stream: OutputStream
  chunk: string
}

/**
 * A hook command started. `event` is its settings name (`PreToolUse`, `Stop`, ...); `command` is shortened,
 * with anything that looks like a secret hidden; `tool_use_id` is the tool call it runs for. With the
 * `hook_progress` capability only.
 */
export interface HookStartedEvent {
  type: 'hook_started'
  event: string
  /** The tool name the hook is limited to; absent when it runs for every tool. */
  matcher?: string | null
  command: string
  tool_use_id?: string | null
}

/** How a hook command ended: `blocked` is exit 2 or a block decision. */
export type HookOutcome = 'ok' | 'blocked' | 'error' | 'cancelled'

/** A hook command ended; `message` is the start of what it said, when it said something. With the `hook_progress` capability only. */
export interface HookFinishedEvent {
  type: 'hook_finished'
  event: string
  matcher?: string | null
  command: string
  tool_use_id?: string | null
  outcome: HookOutcome | string
  duration_ms: number
  message?: string | null
}

/** Every file the agent created, changed or deleted in this session, with lines added and removed. Sent when the list changes. With the `git_views` capability only. */
export interface ChangesEvent {
  type: 'changes'
  files: ChangedFile[]
}

/** Answer to `/diff [path]`: the workspace's changes against `HEAD`, or outside git the agent's changes this session. */
export interface DiffEvent {
  type: 'diff'
  /** What was compared, as a heading (`Changes against HEAD on main`). */
  title: string
  files: DiffFile[]
  /** Files or hunks were left out to keep the event small. */
  truncated: boolean
}

/** Answer to `/commit` without a message: a drafted message and what a commit would take. Nothing is committed until the `commit` command. */
export interface CommitProposalEvent {
  type: 'commit_proposal'
  message: string
  branch: string | null
  files: ChangedFile[]
  /** Nothing is staged, so the commit takes every change (`git add -A`); otherwise only what is staged. */
  stage_all: boolean
}

/** Answer to `/rewind`: the session's checkpoints, newest first. */
export interface CheckpointsEvent {
  type: 'checkpoints'
  items: CheckpointInfo[]
}

/** Answer to the `rewind` command: what it put back (or, for a dry run, would). */
export interface RewoundEvent {
  type: 'rewound'
  checkpoint: number
  mode: RewindMode
  dry_run: boolean
  /** Files written back. */
  restored: string[]
  /** Files removed because they did not exist then. */
  removed: string[]
  not_restored: NotRestoredFile[]
  /** Shell commands run since the checkpoint; what they did was not undone. */
  shell_commands: string[]
  /** Prompts taken out of the conversation: drop the last `removed_turns` prompts with everything after them. 0 when the chat was kept. */
  removed_turns: number
  /** The checkpoint's prompt, for the input box ("edit and send again"); null when the chat was kept. */
  prompt: string | null
}

export type UiEvent =
  | ReadyEvent
  | StatusEvent
  | SessionStartingEvent
  | SignInCodeEvent
  | ModelsEvent
  | TextDeltaEvent
  | TextBlockEndEvent
  | ReasoningStartedEvent
  | ReasoningDeltaEvent
  | ToolCallStartedEvent
  | ToolResultEvent
  | UsageEvent
  | PermissionRequestedEvent
  | NoticeEvent
  | TurnFinishedEvent
  | ErrorEvent
  | SessionsEvent
  | TranscriptEvent
  | WorkspaceTrustedEvent
  | QuestionEvent
  | TodosEvent
  | SubagentStartedEvent
  | SubagentFinishedEvent
  | ExtensionsEvent
  | ReportEvent
  | AccountEvent
  | TrustPreviewEvent
  | ToolInputStartedEvent
  | ToolInputProgressEvent
  | OutputProgressEvent
  | RequestUsageEvent
  | ToolOutputDeltaEvent
  | ChangesEvent
  | DiffEvent
  | CommitProposalEvent
  | CheckpointsEvent
  | RewoundEvent
  | ConnectorCatalogEvent
  | BranchesEvent
  | HookStartedEvent
  | HookFinishedEvent

export type UiEventType = UiEvent['type']

/** Every `UiEvent` tag this client knows; it ignores the others. */
export const UI_EVENT_TYPES: readonly UiEventType[] = [
  'ready',
  'status',
  'session_starting',
  'sign_in_code',
  'models',
  'text_delta',
  'text_block_end',
  'reasoning_started',
  'reasoning_delta',
  'tool_call_started',
  'tool_result',
  'usage',
  'permission_requested',
  'notice',
  'turn_finished',
  'error',
  'sessions',
  'transcript',
  'workspace_trusted',
  'question',
  'todos',
  'subagent_started',
  'subagent_finished',
  'extensions',
  'report',
  'account',
  'trust_preview',
  'tool_input_started',
  'tool_input_progress',
  'output_progress',
  'request_usage',
  'tool_output_delta',
  'changes',
  'diff',
  'commit_proposal',
  'checkpoints',
  'rewound',
  'connector_catalog',
  'branches',
  'hook_started',
  'hook_finished',
]

// ---------------------------------------------------------------- client → engine

export type Attachment =
  | { kind: 'file'; path: string }
  | { kind: 'folder'; path: string }
  | { kind: 'image'; path: string; media_type: string }

export type EngineCommand =
  | { type: 'submit'; text: string; attachments: Attachment[] }
  /** Without the leading slash. */
  | { type: 'slash'; command: string; args: string }
  | { type: 'set_model'; model: string }
  | { type: 'list_models' }
  /** A second one during a turn quits after saving. */
  | { type: 'interrupt' }
  | { type: 'shutdown' }
  /** What this UI handles (feature `capabilities`): `questions`, `reports`; `client` names this UI (`terminal`, `desktop`). */
  | { type: 'capabilities'; features: string[]; client?: string | null }
  /** Answers the `question` event with this id. */
  | { type: 'answer'; id: number; text: string }
  /** Feature `sessions`: the saved chats, as a `sessions` event; `scope` needs feature `session_scope`. */
  | { type: 'list_sessions'; scope?: SessionScope }
  /** Feature `sessions`: continue a saved chat (an id, a path, or `latest`). */
  | { type: 'resume'; session: string }
  /** Feature `trust_workspace`. */
  | { type: 'trust_workspace' }
  /** Feature `extensions`: MCP servers, plugins, skills and agents, as an `extensions` event. */
  | { type: 'list_extensions' }
  /** Feature `trust_preview`: what trusting the workspace would turn on, as a `trust_preview` event. */
  | { type: 'trust_preview' }
  /** Feature `account`: read the plan and usage again, as an `account` event. */
  | { type: 'refresh_account' }
  /** Feature `mcp_toggle`: turn an MCP server on or off; a notice and a fresh `extensions` event follow. */
  | { type: 'set_mcp_server_enabled'; name: string; enabled: boolean }
  /** Feature `git_views`: commit what is staged (everything when nothing is) with this message. */
  | { type: 'commit'; message: string }
  /** Feature `checkpoints`: go back to a checkpoint; `dry_run` reports what it would do and changes nothing. Answered with `rewound`. */
  | { type: 'rewind'; checkpoint: number; mode: RewindMode; dry_run?: boolean }
  /**
   * Feature `mcp_catalog`: the catalog connectors whose id, name, publisher or description contain every
   * word of `query` (all when empty), as a `connector_catalog` event. Read-only.
   */
  | { type: 'list_connector_catalog'; query?: string }
  /**
   * Feature `mcp_catalog`: add the catalog connector `id` as the server `name` (the id when null) to the
   * settings of `scope` (`user`, `project` or `local`; the connector's default when null). `secrets` holds
   * what the user typed for its variables: they go to the OS keychain, never into a settings file, and must
   * never be logged. A notice and a fresh `extensions` event follow, or an error. A remote peer may not send it.
   */
  | { type: 'add_mcp_server'; id: string; name?: string | null; scope?: string | null; secrets?: Record<string, string> }
  /** Feature `mcp_catalog`: remove the server `name` (from the settings of `scope` when given), with its keychain secrets and sign-in. A remote peer may not send it. */
  | { type: 'remove_mcp_server'; name: string; scope?: string | null }
  /**
   * Feature `mcp_catalog`: approve the workspace server `name` (status `needs_approval`) as configured now;
   * with `fingerprint` (from the trust preview), only if it is still configured as the preview showed it.
   * Starts the server. A remote peer may not send it.
   */
  | { type: 'approve_mcp_server'; name: string; fingerprint?: string }
  /** Feature `status_bar`: the thinking level of the next turns; null for the model's default. A `status` follows, or an `error`. A remote peer may not send it. */
  | { type: 'set_reasoning_effort'; effort: string | null }
  /**
   * Feature `status_bar`: `read-only`, `workspace-write`, `danger-full-access` (only when the status says
   * `full_access_allowed`), or `plan` / `plan-off` for plan mode. A `status` follows, or an `error`. A remote
   * peer may not send it: it can raise what the agent may do.
   */
  | { type: 'set_permission_mode'; mode: string }
  /** Feature `status_bar`: the local branches, as a `branches` event. Read-only. */
  | { type: 'list_branches' }
  /**
   * Feature `status_bar`: check out `name` (`create`: a new branch from HEAD). Refused while a turn runs or
   * the tree has changes. A notice and a `status` follow, or an `error`. A remote peer may not send it.
   */
  | { type: 'switch_branch'; name: string; create: boolean }

export const ENGINE_COMMAND_TYPES: readonly EngineCommand['type'][] = [
  'submit',
  'slash',
  'set_model',
  'list_models',
  'interrupt',
  'shutdown',
  'capabilities',
  'answer',
  'list_sessions',
  'resume',
  'trust_workspace',
  'list_extensions',
  'trust_preview',
  'refresh_account',
  'set_mcp_server_enabled',
  'commit',
  'rewind',
  'list_connector_catalog',
  'add_mcp_server',
  'remove_mcp_server',
  'approve_mcp_server',
  'set_reasoning_effort',
  'set_permission_mode',
  'list_branches',
  'switch_branch',
]

export type PermissionDecision =
  | { kind: 'allow_once' }
  | { kind: 'allow_for_session' }
  /**
   * Run this call and save the request's `project_rules`, so matching calls run in this project from now on.
   * Send it only when the request offered `project_rules`. A remote peer may not send it.
   */
  | { kind: 'allow_for_project' }
  /** `session`: matching calls are refused for the rest of the session without asking; an older engine denies this call only. */
  | { kind: 'deny'; note: string | null; session?: boolean }

export interface PermissionReply {
  id: number
  decision: PermissionDecision
}

/** One input line: serde puts the inner value's fields beside `kind`. */
export type ClientMessage =
  | ({ kind: 'command' } & EngineCommand)
  | ({ kind: 'permission_reply' } & PermissionReply)

export function commandMessage(command: EngineCommand): ClientMessage {
  return { kind: 'command', ...command }
}

export function permissionReplyMessage(reply: PermissionReply): ClientMessage {
  return { kind: 'permission_reply', id: reply.id, decision: reply.decision }
}

/** One input line, with its newline. */
export function encodeClientMessage(message: ClientMessage): string {
  return JSON.stringify(message) + '\n'
}

export type ParsedLine =
  | { ok: true; event: UiEvent }
  | { ok: false; reason: 'unknown_type'; type: string }
  | { ok: false; reason: 'invalid'; error: string }

/** Parses one output line. Unknown event types are reported, not thrown. */
export function parseUiEventLine(line: string): ParsedLine {
  let value: unknown
  try {
    value = JSON.parse(line)
  } catch (error) {
    return { ok: false, reason: 'invalid', error: error instanceof Error ? error.message : String(error) }
  }
  return parseUiEvent(value)
}

export function parseUiEvent(value: unknown): ParsedLine {
  if (typeof value !== 'object' || value === null || typeof (value as { type?: unknown }).type !== 'string') {
    return { ok: false, reason: 'invalid', error: 'missing "type" tag' }
  }
  const type = (value as { type: string }).type
  if (!(UI_EVENT_TYPES as readonly string[]).includes(type)) {
    return { ok: false, reason: 'unknown_type', type }
  }
  return { ok: true, event: value as UiEvent }
}

const MAX_TEXT = 256 * 1024
const MAX_ATTACHMENTS = 20
const IMAGE_TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp']

/** A string no longer than `max` UTF-16 units. */
function isText(value: unknown, max: number): value is string {
  return typeof value === 'string' && value.length <= max
}

/** Remote peers may only name paths inside the workspace: relative, no `..`, no NUL. */
function isWorkspaceRelative(path: unknown): path is string {
  if (typeof path !== 'string' || path === '' || path.length > 4096 || path.includes('\0')) return false
  if (/^[\\/]/.test(path) || /^[A-Za-z]:/.test(path)) return false
  return !path.split(/[\\/]/).includes('..')
}

function parseRemoteAttachment(entry: unknown): Attachment | null {
  if (typeof entry !== 'object' || entry === null) return null
  const a = entry as Record<string, unknown>
  if (!isWorkspaceRelative(a.path)) return null
  if (a.kind === 'file' || a.kind === 'folder') return { kind: a.kind, path: a.path }
  if (a.kind === 'image' && typeof a.media_type === 'string' && IMAGE_TYPES.includes(a.media_type)) {
    return { kind: 'image', path: a.path, media_type: a.media_type }
  }
  return null
}

/** Validates a client message from an untrusted peer (a remote client). */
export function parseClientMessage(value: unknown): ClientMessage | null {
  if (typeof value !== 'object' || value === null) return null
  const message = value as Record<string, unknown>
  if (message.kind === 'permission_reply') {
    const decision = message.decision as Record<string, unknown> | undefined
    if (!Number.isSafeInteger(message.id) || typeof decision !== 'object' || decision === null) return null
    if (decision.kind === 'allow_once' || decision.kind === 'allow_for_session') {
      return { kind: 'permission_reply', id: message.id as number, decision: { kind: decision.kind } }
    }
    if (decision.kind === 'deny') {
      // Rebuilt from `note` alone: `session` would refuse matching calls for the rest of the session, and a remote peer may not decide that.
      const note = typeof decision.note === 'string' ? decision.note : null
      return { kind: 'permission_reply', id: message.id as number, decision: { kind: 'deny', note } }
    }
    // Refused on purpose, `allow_for_project` among it: it saves allow rules into the project's settings for good.
    return null
  }
  if (message.kind !== 'command') return null
  switch (message.type) {
    case 'submit': {
      if (!isText(message.text, MAX_TEXT)) return null
      const raw = message.attachments === undefined ? [] : message.attachments
      if (!Array.isArray(raw) || raw.length > MAX_ATTACHMENTS) return null
      const attachments: Attachment[] = []
      for (const entry of raw) {
        const attachment = parseRemoteAttachment(entry)
        if (!attachment) return null
        attachments.push(attachment)
      }
      return { kind: 'command', type: 'submit', text: message.text, attachments }
    }
    case 'slash':
      return isText(message.command, 64) && (message.args === undefined || isText(message.args, 64 * 1024))
        ? { kind: 'command', type: 'slash', command: message.command, args: typeof message.args === 'string' ? message.args : '' }
        : null
    case 'set_model':
      return isText(message.model, 256) ? { kind: 'command', type: 'set_model', model: message.model } : null
    case 'list_models':
    case 'interrupt':
    case 'shutdown':
    case 'trust_workspace':
    case 'list_extensions':
    case 'trust_preview':
    case 'refresh_account':
    // Read-only: the engine lists its local branches and changes nothing.
    case 'list_branches':
      return { kind: 'command', type: message.type }
    case 'list_sessions':
      if (message.scope === undefined) return { kind: 'command', type: 'list_sessions' }
      return message.scope === 'project' || message.scope === 'all'
        ? { kind: 'command', type: 'list_sessions', scope: message.scope }
        : null
    // Read-only: a search over the engine's built-in connector catalog.
    case 'list_connector_catalog':
      if (message.query === undefined) return { kind: 'command', type: 'list_connector_catalog' }
      return isText(message.query, 256) ? { kind: 'command', type: 'list_connector_catalog', query: message.query } : null
    case 'capabilities': {
      if (
        !Array.isArray(message.features) ||
        message.features.length > 32 ||
        !message.features.every((feature) => isText(feature, 64))
      ) {
        return null
      }
      const features = message.features as string[]
      if (message.client !== undefined && !isText(message.client, 64)) return null
      return typeof message.client === 'string'
        ? { kind: 'command', type: 'capabilities', features, client: message.client }
        : { kind: 'command', type: 'capabilities', features }
    }
    case 'answer':
      return Number.isSafeInteger(message.id) && isText(message.text, MAX_TEXT)
        ? { kind: 'command', type: 'answer', id: message.id as number, text: message.text }
        : null
    case 'resume':
      return isText(message.session, 512) && message.session.trim()
        ? { kind: 'command', type: 'resume', session: message.session }
        : null
    case 'set_mcp_server_enabled':
      return isText(message.name, 256) && message.name && typeof message.enabled === 'boolean'
        ? { kind: 'command', type: 'set_mcp_server_enabled', name: message.name, enabled: message.enabled }
        : null
    case 'commit':
      return isText(message.message, 16 * 1024) && message.message.trim()
        ? { kind: 'command', type: 'commit', message: message.message }
        : null
    case 'rewind': {
      const mode = message.mode
      if (!Number.isSafeInteger(message.checkpoint) || (message.checkpoint as number) < 0) return null
      if (typeof mode !== 'string' || !(REWIND_MODES as readonly string[]).includes(mode)) return null
      if (message.dry_run === undefined) {
        return { kind: 'command', type: 'rewind', checkpoint: message.checkpoint as number, mode: mode as RewindMode }
      }
      return typeof message.dry_run === 'boolean'
        ? { kind: 'command', type: 'rewind', checkpoint: message.checkpoint as number, mode: mode as RewindMode, dry_run: message.dry_run }
        : null
    }
    // Refused on purpose: each of these changes something (a connector to run, a server started or
    // removed, the permission mode, the branch checked out, how turns run), and a remote peer has no
    // business doing any of it. `default` refuses them too; the names are here so the decision is on
    // record. `node/host.ts` refuses them again, and `PEER_REFUSED` in test/protocol.test.ts lists
    // them with the reasons.
    case 'add_mcp_server':
    case 'approve_mcp_server':
    case 'remove_mcp_server':
    case 'set_permission_mode':
    case 'switch_branch':
    case 'set_reasoning_effort':
      return null
    default:
      // Any other command, including one a newer engine adds, stays refused until someone decides on it.
      return null
  }
}

/** Splits a byte stream into lines; keeps the partial tail for the next chunk. */
export class JsonLineSplitter {
  private buffer = ''

  push(chunk: string): string[] {
    this.buffer += chunk
    const lines = this.buffer.split('\n')
    this.buffer = lines.pop() ?? ''
    return lines.map((line) => line.replace(/\r$/, '')).filter(Boolean)
  }

  flush(): string | null {
    const rest = this.buffer.replace(/\r$/, '')
    this.buffer = ''
    return rest || null
  }
}
