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

// ---------------------------------------------------------------- engine → client

export interface ReadyEvent {
  type: 'ready'
  protocol: number
  version: string
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

export const ENGINE_COMMAND_TYPES: readonly EngineCommand['type'][] = [
  'submit',
  'slash',
  'set_model',
  'list_models',
  'interrupt',
  'shutdown',
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
      return { kind: 'command', type: message.type }
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
