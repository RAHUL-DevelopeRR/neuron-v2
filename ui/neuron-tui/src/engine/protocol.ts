/**
 * The JSON-lines contract of `neuron engine --stdio`, mirrored from
 * `rust/crates/neuron-tui/src/protocol.rs`. Keep the two in step: every tag and
 * field name here is what serde writes there.
 *
 * - Engine → UI: one {@link UiEvent} per stdout line, tagged by `type`.
 * - UI → engine: one {@link ClientMessage} per stdin line, tagged by `kind`:
 *   `{"kind":"command","type":"submit",...}` or
 *   `{"kind":"permission_reply","id":1,"decision":{"kind":"allow_once"}}`.
 *
 * serde writes `Option::None` as `null` (no field is skipped), so optional
 * fields arrive as `T | null`, not as missing keys.
 */

/** `PROTOCOL_VERSION` in protocol.rs, sent in {@link ReadyEvent}. */
export const PROTOCOL_VERSION = 1

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
}

/** The Zero-X login, as the UI may show it. Never carries the token. */
export interface AccountInfo {
  /** Known right after a sign-in in this engine; not stored with the login. */
  email: string | null
  plan: string | null
  /** Where the login comes from: `NEURON_TOKEN`, or the stored login and where it is kept. */
  source: string
  /** When the session expires (Unix seconds), when the gateway said. */
  expires_at: number | null
}

// ---------------------------------------------------------------- Engine → UI

/** The first event: the engine is ready for commands. */
export interface ReadyEvent {
  type: 'ready'
  /** {@link PROTOCOL_VERSION} of the engine. */
  protocol: number
  /** The Neuron version. */
  version: string
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

/** A notice (retries, warnings, interrupts). ANSI-free; may be multi-line. */
export interface NoticeEvent {
  type: 'notice'
  message: string
}

/** The turn is over (finished, cancelled or failed). */
export interface TurnFinishedEvent {
  type: 'turn_finished'
}

/** The turn failed; `message` is what to show. */
export interface ErrorEvent {
  type: 'error'
  message: string
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
