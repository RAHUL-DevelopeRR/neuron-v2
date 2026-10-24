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

/** A UI capability: questions come as `question` events instead of a "Question:" notice. */
export const CAPABILITY_QUESTIONS = 'questions'

/** One saved chat in the workspace. */
export interface SessionSummary {
  id: string
  /** The first prompt, on one line and shortened. */
  title: string | null
  model: string | null
  message_count: number
  /** Unix milliseconds. */
  updated_at_ms: number
  current: boolean
  branch: string | null
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
   * `unsupported` (a transport Neuron does not speak yet) or `not_started`.
   */
  status: 'starting' | 'ready' | 'failed' | 'pending' | 'unsupported' | 'not_started' | string
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
}

export interface TurnFinishedEvent {
  type: 'turn_finished'
}

export interface ErrorEvent {
  type: 'error'
  message: string
}

/** Answer to `list_sessions`: the workspace's saved chats, newest first. */
export interface SessionsEvent {
  type: 'sessions'
  current: string
  sessions: SessionSummary[]
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
  /** What this UI handles (feature `capabilities`): `questions`. */
  | { type: 'capabilities'; features: string[] }
  /** Answers the `question` event with this id. */
  | { type: 'answer'; id: number; text: string }
  /** Feature `sessions`: the saved chats, as a `sessions` event. */
  | { type: 'list_sessions' }
  /** Feature `sessions`: continue a saved chat (an id, a path, or `latest`). */
  | { type: 'resume'; session: string }
  /** Feature `trust_workspace`. */
  | { type: 'trust_workspace' }
  /** Feature `extensions`: MCP servers, plugins, skills and agents, as an `extensions` event. */
  | { type: 'list_extensions' }

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
      if (typeof message.text !== 'string') return null
      const attachments = Array.isArray(message.attachments) ? message.attachments : []
      const valid = attachments.every(
        (a) =>
          typeof a === 'object' &&
          a !== null &&
          typeof (a as { path?: unknown }).path === 'string' &&
          ['file', 'folder', 'image'].includes((a as { kind?: string }).kind ?? ''),
      )
      return valid ? { kind: 'command', type: 'submit', text: message.text, attachments: attachments as Attachment[] } : null
    }
    case 'slash':
      return typeof message.command === 'string'
        ? { kind: 'command', type: 'slash', command: message.command, args: typeof message.args === 'string' ? message.args : '' }
        : null
    case 'set_model':
      return typeof message.model === 'string' ? { kind: 'command', type: 'set_model', model: message.model } : null
    case 'list_models':
    case 'interrupt':
    case 'shutdown':
    case 'list_sessions':
    case 'trust_workspace':
    case 'list_extensions':
      return { kind: 'command', type: message.type }
    case 'capabilities':
      return Array.isArray(message.features) && message.features.every((feature) => typeof feature === 'string')
        ? { kind: 'command', type: 'capabilities', features: message.features as string[] }
        : null
    case 'answer':
      return Number.isSafeInteger(message.id) && typeof message.text === 'string'
        ? { kind: 'command', type: 'answer', id: message.id as number, text: message.text }
        : null
    case 'resume':
      return typeof message.session === 'string' && message.session.trim()
        ? { kind: 'command', type: 'resume', session: message.session }
        : null
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
