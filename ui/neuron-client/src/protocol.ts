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
 * `checkpoints` and `mcp_toggle`. Send the commands a feature adds only when
 * the engine lists it.
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

/** What the desktop app renders and so declares in its `capabilities` command. */
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
  command: string
  source: string
}

/** An MCP server a workspace's settings would start. */
export interface McpServerPreview {
  name: string
  transport: string
  /** The program it starts or the host it connects to; never arguments or headers. */
  target: string | null
  source: string
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
  /** `bundled`, `user`, `project` or `local`. */
  scope: string
  /** `stdio`, `http`, `sse`, `ws`, `sdk` or `managed_proxy`. */
  transport: string
  /**
   * `starting`, `ready`, `failed`, `pending` (started, tools not found),
   * `unsupported` (a transport Neuron does not speak yet), `not_started` or
   * `disabled` (turned off in the settings).
   */
  status: 'starting' | 'ready' | 'failed' | 'pending' | 'unsupported' | 'not_started' | 'disabled' | string
  /** Its tools, without the `mcp__<server>__` prefix. */
  tools: string[]
  /** Why it failed or is unsupported. */
  error: string | null
  /** Turned on (feature `mcp_toggle`); absent from older engines, meaning on. */
  enabled?: boolean
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
]

export type PermissionDecision =
  | { kind: 'allow_once' }
  | { kind: 'allow_for_session' }
  | { kind: 'deny'; note: string | null }

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
      const note = typeof decision.note === 'string' ? decision.note : null
      return { kind: 'permission_reply', id: message.id as number, decision: { kind: 'deny', note } }
    }
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
      return { kind: 'command', type: message.type }
    case 'list_sessions':
      if (message.scope === undefined) return { kind: 'command', type: 'list_sessions' }
      return message.scope === 'project' || message.scope === 'all'
        ? { kind: 'command', type: 'list_sessions', scope: message.scope }
        : null
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
    default:
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
