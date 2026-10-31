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
 * serde writes `Option::None` as `null`, so optional fields arrive as
 * `T | null`, not as missing keys; the few that are left out when unset are
 * marked `?` here.
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

/**
 * Engine features, listed in {@link ReadyEvent.features} (`FEATURE_*` in
 * protocol.rs). Send a feature's commands only when the engine lists it.
 */
export const FEATURE_CAPABILITIES = 'capabilities'
/** Tool-input progress, output estimates and per-request usage while a turn streams. */
export const FEATURE_LIVE_PROGRESS = 'live_progress'
/** `changes`, `diff` and `commit_proposal` events; the `commit` command. */
export const FEATURE_GIT_VIEWS = 'git_views'
/** `/rewind`, the `checkpoints` and `rewound` events; the `rewind` command. */
export const FEATURE_CHECKPOINTS = 'checkpoints'

/**
 * UI capabilities, declared with the `capabilities` command (`CAPABILITY_*` in
 * protocol.rs). Without them the engine sends none of the events they name and
 * answers `/diff`, `/commit` and `/rewind` with notices.
 */
export const CAPABILITY_LIVE_PROGRESS = 'live_progress'
export const CAPABILITY_GIT_VIEWS = 'git_views'

/** One item of the agent's task list (its `TodoWrite` tool). */
export interface TodoItem {
  content: string
  /** `pending`, `in_progress` or `completed`. */
  status: string
  /** The item in the present tense, while it is in progress. */
  active_form: string | null
}

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

/** The agent's task list changed (its `TodoWrite` tool ran): the whole list. */
export interface TodosEvent {
  type: 'todos'
  items: TodoItem[]
}

/** A sub-agent started; `id` is its `Agent` call's `tool_call_started` id. */
export interface SubagentStartedEvent {
  type: 'subagent_started'
  id: string
  description: string
  subagent_type: string | null
}

/**
 * A sub-agent's `Agent` call ended: `status` is `completed`, `failed`,
 * `running` (it went on in the background), ...
 */
export interface SubagentFinishedEvent {
  type: 'subagent_finished'
  id: string
  status: string
  is_error: boolean
  result: string | null
  note: string | null
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
  | TodosEvent
  | SubagentStartedEvent
  | SubagentFinishedEvent
  | ChangesEvent
  | DiffEvent
  | CommitProposalEvent
  | CheckpointsEvent
  | RewoundEvent

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
  'todos',
  'subagent_started',
  'subagent_finished',
  'changes',
  'diff',
  'commit_proposal',
  'checkpoints',
  'rewound',
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
   * {@link FEATURE_CAPABILITIES}. Replaces an earlier one; may come at any time.
   */
  | { type: 'capabilities'; features: string[]; client?: string | null }
  /** Commit with this message: what is staged, or every change when nothing is. */
  | { type: 'commit'; message: string }
  /** Go back to a checkpoint from a `checkpoints` event. */
  | { type: 'rewind'; checkpoint: number; mode: RewindMode; dry_run?: boolean }

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
