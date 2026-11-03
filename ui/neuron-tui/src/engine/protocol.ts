/**
 * The JSON-lines contract of `neuron engine --stdio`, mirrored from
 * `rust/crates/neuron-tui/src/protocol.rs`. Keep the two in step: every tag and
 * field name here is what serde writes there, and
 * `__tests__/protocol.test.ts` reads protocol.rs and fails when they drift.
 *
 * - Engine → UI: one {@link UiEvent} per stdout line, tagged by `type`.
 * - UI → engine: one {@link ClientMessage} per stdin line, tagged by `kind`:
 *   `{"kind":"command","type":"submit",...}` or
 *   `{"kind":"permission_reply","id":1,"decision":{"kind":"allow_once"}}`.
 *
 * serde writes `Option::None` as `null`, so optional fields arrive as
 * `T | null`. Fields added after protocol 1 shipped are optional (`?`) here:
 * older engines leave them out.
 *
 * Additions after protocol 1 are engine features, listed in `ready.features`
 * (`FEATURE_*` below). Send the commands a feature adds only when the engine
 * lists it: an older engine skips them with a notice.
 */

/** `PROTOCOL_VERSION` in protocol.rs, sent in {@link ReadyEvent}. */
export const PROTOCOL_VERSION = 1

/** `capabilities`, `answer` and (with the `questions` capability) `question` events. */
export const FEATURE_CAPABILITIES = 'capabilities'
/** `list_sessions`, `resume`, and `transcript` events. */
export const FEATURE_SESSIONS = 'sessions'
/** `trust_workspace` and `workspace_trusted`. */
export const FEATURE_TRUST_WORKSPACE = 'trust_workspace'
/** `todos` events. */
export const FEATURE_TODOS = 'todos'
/** `subagent_started` and `subagent_finished` events. */
export const FEATURE_SUBAGENTS = 'subagents'
/** `list_extensions` and `extensions`. */
export const FEATURE_EXTENSIONS = 'extensions'
/**
 * `report` events for `/usage`, `/init`, `/memory` and `/doctor`, when the UI
 * sends the `reports` capability; without it they come as notices.
 */
export const FEATURE_REPORTS = 'reports'
/** `account` events and the `refresh_account` command. */
export const FEATURE_ACCOUNT = 'account'
/** `detail` on `error` events: what the user can do about the failure. */
export const FEATURE_ERROR_DETAILS = 'error_details'
/**
 * `list_sessions` takes a `scope`, and session rows say where they belong and
 * whether another Neuron runs them.
 */
export const FEATURE_SESSION_SCOPE = 'session_scope'
/** The `trust_preview` command and event. */
export const FEATURE_TRUST_PREVIEW = 'trust_preview'
/** `supports_images` on models and on `status`. */
export const FEATURE_MODEL_IMAGES = 'model_images'
/** Tool-input progress, output estimates and per-request usage while a turn streams. */
export const FEATURE_LIVE_PROGRESS = 'live_progress'
/** `changes`, `diff` and `commit_proposal` events; the `commit` command. */
export const FEATURE_GIT_VIEWS = 'git_views'
/** `/rewind`, the `checkpoints` and `rewound` events; the `rewind` command. */
export const FEATURE_CHECKPOINTS = 'checkpoints'

/** A UI capability: questions come as `question` events, not "Question:" notices. */
export const CAPABILITY_QUESTIONS = 'questions'
/** A UI capability: reports come as `report` events, not notices. */
export const CAPABILITY_REPORTS = 'reports'
/**
 * A UI capability: tool-input progress, output estimates and per-request usage
 * while a turn streams. Without it the engine sends none of these.
 */
export const CAPABILITY_LIVE_PROGRESS = 'live_progress'
/**
 * A UI capability: changes, diffs, commit proposals, checkpoints and rewinds as
 * their own events. Without it the engine answers `/diff`, `/commit` and
 * `/rewind` with notices.
 */
export const CAPABILITY_GIT_VIEWS = 'git_views'

/** Token counts as the provider reported them (or estimated before it did). */
export interface TokenCounts {
  input: number
  output: number
  cache_read: number
  cache_write: number
}

/** What a tool call needs approval for, as the permission dialog shows it. */
export interface PermissionView {
  tool: string
  current_mode: string
  required_mode: string
  reason: string | null
  /** The tool input as JSON text (commands, paths, diffs are inside it). */
  input: string
  /**
   * What "allow for this session" would approve from now on (command prefixes
   * such as `cargo test`, or a web domain). Empty when only a one-time
   * approval is possible.
   */
  session_prefixes: string[]
}

/** The workspace's git state, for the footer. */
export interface GitStatus {
  /** The branch, `detached HEAD`, or `null` when git could not tell. */
  branch: string | null
  /** Changed paths in all: staged, unstaged, untracked and conflicted ones. */
  changed: number
  staged: number
  unstaged: number
  untracked: number
  conflicted: number
}

/** The gateway's live health for a model (`status` in {@link ModelInfo}). */
export type ModelStatus = 'available' | 'degraded' | 'busy' | 'blurred'

/** One model the user can switch to. */
export interface ModelInfo {
  id: string
  display_name: string | null
  /**
   * Whether the user's plan can use it now; `unavailable_reason` says why not
   * (the gateway's `upgrade_required`, `region_unavailable`, `not_configured`,
   * or its own words).
   */
  available: boolean
  unavailable_reason: string | null
  context_window: number | null
  /** Plan tiers that include it (`free`, `pro`, ...). */
  tiers: string[]
  /**
   * The gateway's health for it: `degraded` and `busy` still work, `blurred`
   * is out of rotation until `available_again_at`. Absent from older engines,
   * `null` when the gateway did not say.
   */
  status?: ModelStatus | null
  /** When a `blurred` model can be used again (ISO-8601), when known. */
  available_again_at?: string | null
  /** The catalog says the model reads images; absent when it does not say. */
  supports_images?: boolean | null
}

/** The Zero-X login, as the UI may show it. Never carries the token. */
export interface AccountInfo {
  /** Known right after a sign-in in this engine, or stored with the login. */
  email: string | null
  plan: string | null
  /** Where the login comes from: `NEURON_TOKEN`, or the stored login and where it is kept. */
  source: string
  /** When the session expires (Unix seconds), when the gateway said. */
  expires_at: number | null
}

/** Which saved chats `list_sessions` lists. */
export type SessionScope = 'project' | 'all'

/**
 * What a saved chat is doing in another Neuron: `idle` (none has it open),
 * `open` (one has it open) or `running` (one is running a turn in it).
 */
export type SessionRunStatus = 'idle' | 'open' | 'running'

/** One saved chat, for a session list. */
export interface SessionSummary {
  id: string
  /** The first prompt, on one line and shortened. */
  title: string | null
  model: string | null
  message_count: number
  /** When it last changed (Unix milliseconds). */
  updated_at_ms: number
  /** The session the engine runs now. */
  current: boolean
  /** The branch name of a forked session. */
  branch: string | null
  /** The workspace folder it belongs to (feature `session_scope`). */
  project?: string | null
  /** The git branch when it last ran, when Neuron saw one. */
  git_branch?: string | null
  /** The current session is `idle` here: the engine's own state is in `status`. */
  run_status?: SessionRunStatus
  /** Its last turn failed. */
  failed?: boolean
  /** The Neuron that last ran it (`terminal`, `desktop`), when known. */
  client?: string | null
}

/** One entry of a saved conversation (`#[serde(tag = "kind")]`). */
export type TranscriptEntry =
  | { kind: 'user'; text: string }
  | { kind: 'assistant'; text: string }
  | { kind: 'tool_call'; id: string; name: string; input: string }
  | {
      kind: 'tool_result'
      id: string
      name: string
      output: string
      is_error: boolean
    }

/** One item of the agent's task list (its TodoWrite tool). */
export interface TodoItem {
  content: string
  /** `pending`, `in_progress` or `completed`. */
  status: string
  /** The item in the present tense, while it is in progress. */
  active_form: string | null
}

/** One configured MCP server and what it is doing now. */
export interface McpServerInfo {
  name: string
  /** `bundled`, `user`, `project` or `local`. */
  scope: string
  /** `stdio`, `http`, `sse`, `ws`, `sdk` or `managed_proxy`. */
  transport: string
  /** `starting`, `ready`, `failed`, `pending`, `unsupported` or `not_started`. */
  status: string
  /** Its tools, without the `mcp__<server>__` prefix. */
  tools: string[]
  /** Why it failed or is unsupported. */
  error: string | null
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
  /** Where it comes from (`project`, `user`, `bundled`, ...), as people read it. */
  source: string
  /** False when another definition of the same name wins. */
  active: boolean
}

/** One part of a {@link ReportEvent}. */
export interface ReportSection {
  heading: string | null
  /** GitHub-flavoured Markdown. */
  markdown: string
}

/** A link a report offers (the usage dashboard, the plans page). */
export interface ReportLink {
  label: string
  url: string
}

/** Requests and tokens: a plan's daily limits, or what was used today. */
export interface UsageAmounts {
  requests: number | null
  tokens: number | null
}

/**
 * The Zero-X account as the usage panel and `/account` show it. Never carries
 * the token.
 */
export interface AccountDetails {
  email: string | null
  /** The display name the account gave. */
  name: string | null
  /** The plan's id: `free`, `pro`, `max`, ... */
  plan: string | null
  /** The plan as people read it ("Pro"). */
  plan_name: string | null
  /** Where the login comes from. */
  source: string
  /** When the session expires (Unix seconds), when the gateway said. */
  expires_at: number | null
  /** The plan's daily limits. */
  limits: UsageAmounts | null
  /** What was used today, from the account or the last reply's quota headers. */
  usage_today: UsageAmounts | null
  /** When today's usage resets (ISO 8601, UTC). */
  resets_at: string | null
  /** Prepaid credit, in US cents. */
  credit_balance_cents: number | null
  suspended: boolean
  /** The account dashboard and the plans page, to open in a browser. */
  dashboard_url: string
  pricing_url: string
  /**
   * When the plan and usage were last read (Unix seconds); `null` when only
   * the stored login is known so far.
   */
  usage_updated_at: number | null
  /** Why the plan and usage could not be read, when they could not. */
  usage_error: string | null
}

/**
 * What a failed request needs from the user, for the UI to offer the way out:
 * sign in again, upgrade, wait for a reset, or retry later.
 */
export interface ErrorDetail {
  /**
   * The gateway's code (`invalid_session`, `upgrade_required`,
   * `insufficient_quota`, `insufficient_credits`, `rate_limited`,
   * `capacity_exhausted`, ...), `unreachable` when the server could not be
   * reached, `not_available` when it does not offer the call yet (a 404), or
   * `sign_in_required` when the call needs a Zero-X login and there is none.
   */
  code: string
  /** The HTTP status, when there was an answer. */
  status: number | null
  upgrade_url: string | null
  /** When a quota resets (ISO 8601, UTC). */
  resets_at: string | null
  retry_after_seconds: number | null
}

/** A hook a workspace's settings would turn on. */
export interface HookPreview {
  /** `PreToolUse`, `PostToolUse` or `PostToolUseFailure`. */
  event: string
  /** The command it runs (shortened when long). */
  command: string
  /** The settings file it comes from. */
  source: string
}

/** An MCP server a workspace's settings would start. */
export interface McpServerPreview {
  name: string
  transport: string
  /**
   * The program it starts, or the host it connects to; never arguments,
   * environment or headers, which can hold secrets.
   */
  target: string | null
  source: string
}

/** A permission rule a workspace's settings would add. */
export interface PermissionRulePreview {
  /** `allow` or `ask` (deny rules apply untrusted too). */
  effect: string
  rule: string
  source: string
}

// ---------------------------------------------------------------- Engine → UI

/** A file and how it changed, with line counts. */
export interface ChangedFile {
  /** Relative to the workspace root, with `/`; absolute for files outside it. */
  path: string
  /** `created`, `modified`, `deleted` or `renamed`. */
  status: string
  added: number
  removed: number
}

/** One file of a {@link DiffEvent}. */
export interface DiffFile extends ChangedFile {
  /**
   * The unified diff's hunks (from the first `@@` line on); empty for binary
   * files and for files left out to keep the event small.
   */
  patch: string
  binary: boolean
}

/** One entry of the "go back to" list ({@link CheckpointsEvent}). */
export interface CheckpointInfo {
  id: number
  /** When the prompt was sent (Unix milliseconds). */
  created_at_ms: number
  /** The prompt, shortened to one line. */
  prompt_preview: string
  /** Files the agent's file tools changed in that turn. */
  files: string[]
  /** Shell commands that turn ran; a rewind cannot undo what they did. */
  shell_commands: string[]
  /** On the checkpoint a code-only rewind saves first: where that rewind went. */
  before_rewind_to?: number
}

/** A file a rewind could not put back, and why. */
export interface NotRestoredFile {
  path: string
  reason: string
}

/** The first event: the engine is ready for commands. */
export interface ReadyEvent {
  type: 'ready'
  /** {@link PROTOCOL_VERSION} of the engine. */
  protocol: number
  /** The Neuron version. */
  version: string
  /** Engine features (`FEATURE_*`); absent from engines before them. */
  features?: string[]
}

/**
 * What the footer shows. Sent after `ready`, after every `turn_finished`, and
 * after a model switch.
 */
export interface StatusEvent {
  type: 'status'
  model: string
  permission_mode: string
  session_id: string
  cwd: string
  /** Untrusted workspaces run without their own settings, hooks and MCP servers. */
  workspace_trusted: boolean
  plan_mode: boolean
  /** Estimated share of the model's input budget the conversation uses. */
  context_percent: number
  /** `null` outside a git repository. */
  git: GitStatus | null
  /**
   * A usable Zero-X login: `NEURON_TOKEN`, or a stored session that has not
   * expired. Added after protocol 1 shipped: absent from older engines.
   */
  signed_in?: boolean
  /** The login, also when its session expired; `null` when there is none. */
  account?: AccountInfo | null
  /**
   * Prompts need a Zero-X sign-in first: the engine's signed-out stage, or a
   * Zero-X session without a usable login (after `/logout`). False for a
   * provider with its own API key. Absent from engines before it.
   */
  sign_in_required?: boolean
  /**
   * The catalog says the current model reads images (feature
   * `model_images`). Hide the image-paste hint unless it is `true`.
   */
  supports_images?: boolean | null
  /**
   * The model that answered the last request, when it is not `model` (`auto`
   * picks one per request; a failover can switch). Absent before the first
   * answer, for providers that do not name it, and from older engines.
   */
  served_model?: string | null
}

/**
 * After a successful `/login` from the signed-out stage, right before the
 * engine opens the session (which starts MCP servers and can take a while).
 * `turn_finished` and a `status` follow, or an `error`.
 */
export interface SessionStartingEvent {
  type: 'session_starting'
}

/**
 * `/login` started a device sign-in: open `url` (or `url_complete`, which has
 * the code filled in) and enter `user_code`. The engine waits for approval; an
 * `interrupt` cancels it. A notice and a `status` follow, then `turn_finished`.
 */
export interface SignInCodeEvent {
  type: 'sign_in_code'
  url: string
  url_complete: string | null
  user_code: string
  expires_in_secs: number
  /** The engine opened `url_complete` (or `url`) in the browser. */
  browser_opened: boolean
}

/**
 * Answer to `list_models`. Empty when the provider has no catalog (only the
 * Zero-X gateway publishes one).
 */
export interface ModelsEvent {
  type: 'models'
  current: string
  models: ModelInfo[]
}

/** A streamed piece of assistant text (Markdown, not yet rendered). */
export interface TextDeltaEvent {
  type: 'text_delta'
  text: string
}

/** The current text block ended. */
export interface TextBlockEndEvent {
  type: 'text_block_end'
}

/**
 * A reasoning block started. `chars` is known for complete blocks; `redacted`
 * when the provider hides the reasoning.
 */
export interface ReasoningStartedEvent {
  type: 'reasoning_started'
  chars: number | null
  redacted: boolean
}

/** Reasoning text, when the provider streams it. */
export interface ReasoningDeltaEvent {
  type: 'reasoning_delta'
  text: string
}

/**
 * A tool call with its complete input (JSON text). `cut_off`: the output
 * limit truncated it and it will not run.
 */
export interface ToolCallStartedEvent {
  type: 'tool_call_started'
  id: string
  name: string
  input: string
  cut_off: boolean
}

/**
 * A tool call's result as the model sees it. Current engines send one for
 * every call: one that ran, was denied or blocked by a hook, or never started.
 * `id` is the `tool_call_started` id. Older engines sent no `id` and no result
 * for a denied call, so the UI still matches by name and closes open rows at
 * `turn_finished`.
 */
export interface ToolResultEvent {
  type: 'tool_result'
  id?: string
  name: string
  output: string
  is_error: boolean
}

/** Token usage: this turn so far, the session total, and whether it is an estimate. */
export interface UsageEvent {
  type: 'usage'
  turn: TokenCounts
  cumulative: TokenCounts
  estimated: boolean
}

/** A tool call waits for approval; answer with a {@link PermissionReply} carrying `id`. */
export interface PermissionRequestedEvent {
  type: 'permission_requested'
  id: number
  request: PermissionView
}

/** What a {@link NoticeEvent} is about. `NoticeKind` in protocol.rs. */
export type NoticeKind = 'refusal'

/** A notice (retries, warnings, interrupts). ANSI-free; may be multi-line. */
export interface NoticeEvent {
  type: 'notice'
  message: string
  /**
   * Set when the notice is more than a status line; left out of plain notices
   * (and absent from engines older than the field). `refusal`: the model
   * declined to continue the turn's request (`message` is the one line to
   * show, with the provider's note); it is not assistant text.
   */
  kind?: NoticeKind | null
}

/** The turn is over (finished, cancelled or failed). */
export interface TurnFinishedEvent {
  type: 'turn_finished'
}

/** The turn failed; `message` is what to show. */
export interface ErrorEvent {
  type: 'error'
  message: string
  /**
   * What the failure needs from the user, when a server said (feature
   * `error_details`): drives the sign-in-again, upgrade and wait cards.
   */
  detail?: ErrorDetail | null
}

/** Answer to `list_sessions`: the saved chats in `scope`, newest first. */
export interface SessionsEvent {
  type: 'sessions'
  current: string
  sessions: SessionSummary[]
  /** Absent from engines before `session_scope`, which list the workspace's. */
  scope?: SessionScope
}

/**
 * The conversation of the session the engine now runs, from its start: after
 * `--resume`, `resume`, `/resume` and `/clear`. Long outputs are shortened;
 * `truncated` when the oldest entries were left out.
 */
export interface TranscriptEvent {
  type: 'transcript'
  session_id: string
  entries: TranscriptEntry[]
  truncated: boolean
}

/**
 * `trust_workspace` recorded the workspace as trusted. Its settings, hooks and
 * MCP servers load when the session starts, so the UI restarts the engine
 * (`--resume` keeps the chat) when `restart_required`.
 */
export interface WorkspaceTrustedEvent {
  type: 'workspace_trusted'
  cwd: string
  restart_required: boolean
  /** The folder trust was recorded for: the git root, or the folder itself. */
  root?: string | null
  /**
   * Trusted for this engine only (the home folder): restart the engine with
   * `NEURON_TRUST_WORKSPACE=1`, and ask again at the next launch.
   */
  session_only?: boolean
}

/** The agent asks something (with the `questions` capability); answer with `answer`. */
export interface QuestionEvent {
  type: 'question'
  id: number
  question: string
  /** Choices, when the agent offered some; the answer may be free text too. */
  options: string[]
}

/** The agent's whole task list, after its TodoWrite tool ran. */
export interface TodosEvent {
  type: 'todos'
  items: TodoItem[]
}

/** An Agent call began running; `id` is its `tool_call_started` id. */
export interface SubagentStartedEvent {
  type: 'subagent_started'
  id: string
  description: string
  subagent_type: string | null
}

/** An Agent call ended: `status` is the sub-agent's, `result` its final text. */
export interface SubagentFinishedEvent {
  type: 'subagent_finished'
  id: string
  status: string
  is_error: boolean
  result: string | null
  note: string | null
}

/** Answer to `list_extensions`. Nothing secret: no commands, arguments or headers. */
export interface ExtensionsEvent {
  type: 'extensions'
  mcp_servers: McpServerInfo[]
  plugins: PluginInfo[]
  skills: DefinitionInfo[]
  agents: DefinitionInfo[]
}

/**
 * What a report command (`/usage`, `/init`, `/memory`, `/doctor`) found, for
 * a scrollable panel, with the `reports` capability; `turn_finished` follows.
 */
export interface ReportEvent {
  type: 'report'
  /** The command's name without the slash. */
  command: string
  title: string
  sections: ReportSection[]
  links: ReportLink[]
}

/**
 * The Zero-X account (feature `account`): after `ready` when there is a
 * login, once the plan and usage were read, after `/login` and `/logout`, on
 * `refresh_account`, and after a turn whose quota headers changed today's
 * usage. `account` is `null` when there is no login.
 */
export interface AccountEvent {
  type: 'account'
  signed_in: boolean
  account: AccountDetails | null
}

/**
 * Answer to `trust_preview`: what trusting the workspace would turn on, for
 * the trust screen. Reading it starts nothing.
 */
export interface TrustPreviewEvent {
  type: 'trust_preview'
  cwd: string
  /** The folder trust would be recorded for: the git root, or the folder itself. */
  root: string
  /** `root` is the top folder of a git repository (else it is `cwd`, outside git). */
  is_git_root: boolean
  /** Trust would last for this engine only (the home folder, or above it). */
  session_only: boolean
  /** The workspace is trusted already. */
  trusted: boolean
  /** The workspace's settings files. */
  settings_files: string[]
  hooks: HookPreview[]
  mcp_servers: McpServerPreview[]
  permission_rules: PermissionRulePreview[]
  /** Other settings the files would apply, by key (`env`, `model`, ...). */
  other_settings: string[]
}

/**
 * The model started a tool call and is still writing its input: show the call
 * now. `tool_input_progress` follows while the input grows, then
 * `tool_call_started` with the same `id` and the complete input.
 */
export interface ToolInputStartedEvent {
  type: 'tool_input_started'
  id: string
  name: string
}

/**
 * How far the model got writing a tool call's input; at most every 100 ms or so
 * per call, and once more when the input is complete.
 */
export interface ToolInputProgressEvent {
  type: 'tool_input_progress'
  id: string
  name: string
  /** Bytes of input (JSON text) so far. */
  bytes: number
  /** Lines of the call's main text so far (file content, new text, command). */
  lines: number
  /** A path, command, search pattern or URL, once the model has written it. */
  target: string | null
  /** The last lines of the main text so far (at most about 1 KB), unescaped. */
  preview: string | null
}

/**
 * Estimated output tokens of the model response now streaming (text, reasoning
 * and tool input). Starts again from 0 with each model request of a turn.
 */
export interface OutputProgressEvent {
  type: 'output_progress'
  output_tokens: number
}

/** One model request of the turn ended: its tokens as the provider reported them. */
export interface RequestUsageEvent {
  type: 'request_usage'
  usage: TokenCounts
}

/**
 * Every file the agent created, changed or deleted in this session, with lines
 * added and removed since the session first touched it.
 */
export interface ChangesEvent {
  type: 'changes'
  files: ChangedFile[]
}

/**
 * Answer to `/diff [path]`: the workspace's changes against `HEAD`, or outside
 * git the agent's changes this session.
 */
export interface DiffEvent {
  type: 'diff'
  /** What was compared, as a heading. */
  title: string
  files: DiffFile[]
  /** Files or hunks were left out to keep the event small. */
  truncated: boolean
}

/**
 * Answer to `/commit` without a message: a drafted message and what a commit
 * would take. Nothing is committed until the UI sends a `commit` command.
 */
export interface CommitProposalEvent {
  type: 'commit_proposal'
  message: string
  branch: string | null
  files: ChangedFile[]
  /** Nothing is staged, so the commit takes every change (`git add -A`). */
  stage_all: boolean
}

/** Answer to `/rewind`: the session's checkpoints, newest first. */
export interface CheckpointsEvent {
  type: 'checkpoints'
  items: CheckpointInfo[]
}

/** What a rewind puts back. */
export type RewindMode = 'code_and_chat' | 'chat_only' | 'code_only'

/** Answer to a `rewind` command: what it put back (or, for a dry run, would). */
export interface RewoundEvent {
  type: 'rewound'
  checkpoint: number
  mode: RewindMode
  dry_run: boolean
  restored: string[]
  removed: string[]
  not_restored: NotRestoredFile[]
  /** Shell commands run since the checkpoint; what they did was not undone. */
  shell_commands: string[]
  /**
   * Prompts taken out of the conversation: drop the last `removed_turns`
   * prompts with everything after them. 0 when the chat was kept.
   */
  removed_turns: number
  /** The checkpoint's prompt, for the input box; null when the chat was kept. */
  prompt: string | null
}

/** Engine → UI. `UiEvent` in protocol.rs (`#[serde(tag = "type")]`). */
export type UiEvent =
  | ReadyEvent
  | StatusEvent
  | SignInCodeEvent
  | SessionStartingEvent
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
  | ToolInputStartedEvent
  | ToolInputProgressEvent
  | OutputProgressEvent
  | RequestUsageEvent
  | ChangesEvent
  | DiffEvent
  | CommitProposalEvent
  | CheckpointsEvent
  | RewoundEvent
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

export type UiEventType = UiEvent['type']

/** Every `UiEvent` tag this UI knows. A UI ignores events it does not know. */
export const UI_EVENT_TYPES: readonly UiEventType[] = [
  'ready',
  'status',
  'sign_in_code',
  'session_starting',
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
  'tool_input_started',
  'tool_input_progress',
  'output_progress',
  'request_usage',
  'changes',
  'diff',
  'commit_proposal',
  'checkpoints',
  'rewound',
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
]

// ---------------------------------------------------------------- UI → engine

/** A file, folder or image the user attached to a prompt (`#[serde(tag = "kind")]`). */
export type Attachment =
  | { kind: 'file'; path: string }
  | { kind: 'folder'; path: string }
  | { kind: 'image'; path: string; media_type: string }

/** UI → engine. `EngineCommand` in protocol.rs (`#[serde(tag = "type")]`). */
export type EngineCommand =
  /** Run a turn with this prompt. */
  | { type: 'submit'; text: string; attachments: Attachment[] }
  /** Run a slash command (`/model`, `/compact`, ...), without the leading slash. */
  | { type: 'slash'; command: string; args: string }
  /** Switch the model for the next turn. */
  | { type: 'set_model'; model: string }
  /** Ask for the models the user can switch to (a `models` event). */
  | { type: 'list_models' }
  /** Interrupt the running turn (a second one quits after saving, like Ctrl-C twice). */
  | { type: 'interrupt' }
  /** Save and stop. */
  | { type: 'shutdown' }
  /**
   * What this UI handles (`CAPABILITY_*`), when the engine lists
   * {@link FEATURE_CAPABILITIES}: `questions`, `reports`, `live_progress`,
   * `git_views`. Replaces an earlier one; may come at any time. `client` names
   * this UI (`terminal`), recorded with the sessions it runs.
   */
  | { type: 'capabilities'; features: string[]; client?: string | null }
  /** Answer the `question` event with this id. A stale id is ignored. */
  | { type: 'answer'; id: number; text: string }
  /**
   * The saved chats, as a `sessions` event (feature `sessions`); `scope`
   * needs feature `session_scope`.
   */
  | { type: 'list_sessions'; scope?: SessionScope }
  /** Continue a saved chat (an id, a path, or `latest`): `transcript` and `status` follow. */
  | { type: 'resume'; session: string }
  /** Trust the workspace (feature `trust_workspace`): `workspace_trusted` and `status` follow. */
  | { type: 'trust_workspace' }
  /** MCP servers, plugins, skills and agents, as an `extensions` event. */
  | { type: 'list_extensions' }
  /** What trusting the workspace would turn on (feature `trust_preview`). */
  | { type: 'trust_preview' }
  /** Read the plan and usage again, as an `account` event (feature `account`). */
  | { type: 'refresh_account' }
  /** Commit with this message: what is staged, or every change when nothing is. */
  | { type: 'commit'; message: string }
  /** Go back to a checkpoint from a `checkpoints` event. */
  | { type: 'rewind'; checkpoint: number; mode: RewindMode; dry_run?: boolean }

/** Every `EngineCommand` tag. */
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
  'commit',
  'rewind',
]

/** The user's answer to a permission request (`#[serde(tag = "kind")]`). */
export type PermissionDecision =
  /** Run this call. */
  | { kind: 'allow_once' }
  /** Run this call and similar ones for the rest of the session. */
  | { kind: 'allow_for_session' }
  /** Do not run it; `note` is passed to the model. */
  | { kind: 'deny'; note: string | null }

/** Answer to `permission_requested` with the same `id`. */
export interface PermissionReply {
  id: number
  decision: PermissionDecision
}

/**
 * One stdin line of `neuron engine --stdio`: a command, or an answer to a
 * permission request. serde flattens the inner value beside `kind`.
 */
export type ClientMessage =
  | ({ kind: 'command' } & EngineCommand)
  | ({ kind: 'permission_reply' } & PermissionReply)

// --------------------------------------------------------------- JSON lines

/** Encodes one stdin line (with its trailing newline). */
export function encodeClientMessage(message: ClientMessage): string {
  return JSON.stringify(message) + '\n'
}

export function commandMessage(command: EngineCommand): ClientMessage {
  return { kind: 'command', ...command }
}

export function permissionReplyMessage(reply: PermissionReply): ClientMessage {
  return { kind: 'permission_reply', id: reply.id, decision: reply.decision }
}

export type ParsedLine =
  | { ok: true; event: UiEvent }
  /** A known-shaped line whose `type` this UI does not know: ignore it. */
  | { ok: false; reason: 'unknown_type'; type: string }
  | { ok: false; reason: 'invalid'; error: string }

/**
 * Parses one stdout line. Unknown event types are reported, not thrown: the
 * engine may add events before the UI knows them.
 */
export function parseUiEventLine(line: string): ParsedLine {
  let value: unknown
  try {
    value = JSON.parse(line)
  } catch (error) {
    return {
      ok: false,
      reason: 'invalid',
      error: error instanceof Error ? error.message : String(error),
    }
  }
  if (
    typeof value !== 'object' ||
    value === null ||
    typeof (value as { type?: unknown }).type !== 'string'
  ) {
    return { ok: false, reason: 'invalid', error: 'missing "type" tag' }
  }
  const type = (value as { type: string }).type
  if (!(UI_EVENT_TYPES as readonly string[]).includes(type)) {
    return { ok: false, reason: 'unknown_type', type }
  }
  return { ok: true, event: value as UiEvent }
}

/**
 * Splits a stdout byte stream into lines. Feed it chunks as they arrive; it
 * returns the complete lines and keeps the partial tail for the next chunk.
 */
export class JsonLineSplitter {
  private buffer = ''

  push(chunk: string): string[] {
    this.buffer += chunk
    const lines = this.buffer.split('\n')
    this.buffer = lines.pop() ?? ''
    return lines.map((line) => line.replace(/\r$/, '')).filter(Boolean)
  }

  /** The unterminated tail, if the stream ended mid-line. */
  flush(): string | null {
    const rest = this.buffer.replace(/\r$/, '')
    this.buffer = ''
    return rest ? rest : null
  }
}
