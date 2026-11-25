/**
 * A session as a client shows it, built from the engine's events and the
 * user's own actions by a pure reducer. The desktop app, the mobile app and a
 * remote host all keep their state this way, so every front end reads the
 * same transcript from the same events.
 */

import type {
  AccountDetails,
  AccountInfo,
  Attachment,
  ChangedFile,
  CheckpointInfo,
  CommitProposalEvent,
  DefinitionInfo,
  DiffEvent,
  McpServerInfo,
  ModelInfo,
  NoticeKind,
  OutputStream,
  PermissionDecision,
  PermissionView,
  PluginInfo,
  ReportLink,
  ReportSection,
  RewoundEvent,
  SessionSummary,
  SignInCodeEvent,
  StatusEvent,
  TodoItem,
  TokenCounts,
  TranscriptEntry,
  TrustPreviewEvent,
  UiEvent,
} from './protocol'
import { gatewayProblem, type GatewayProblem } from './gateway'
import { relativeTo, toolView, withResult, type ToolView } from './tools'

/** `writing`: the model is still writing the call's input (`tool_input_started`). */
export type ToolState = 'writing' | 'running' | 'awaiting_permission' | 'done' | 'error' | 'denied' | 'cut_off'

/** The most characters of a running command's output a tool item keeps. */
export const LIVE_OUTPUT_LIMIT = 64 * 1024

/** What a running command printed so far, from `tool_output_delta` events. */
export interface LiveOutput {
  chunks: Array<{ stream: OutputStream; text: string }>
  /** Characters dropped from the front to stay within {@link LIVE_OUTPUT_LIMIT}. */
  dropped: number
}

/** How far the model got writing a call's input (`tool_input_progress`). */
export interface ToolInputProgress {
  bytes: number
  lines: number
  target: string | null
  preview: string | null
}

/** The sub-agent an `Agent` call runs. */
export interface Subagent {
  description: string
  type: string | null
  running: boolean
  /** The sub-agent's own status once it ended (`completed`, `failed`, `running` in the background). */
  status: string | null
  result: string | null
  note: string | null
}

export type TranscriptItem =
  | {
      kind: 'user'
      key: string
      text: string
      attachments: Attachment[]
      /** Sent as a prompt that started a turn (not a slash command or an answer). Absent from items built before the flag. */
      prompt?: boolean
    }
  | { kind: 'assistant'; key: string; text: string; streaming: boolean }
  | { kind: 'reasoning'; key: string; text: string; chars: number | null; redacted: boolean; streaming: boolean }
  | {
      kind: 'tool'
      key: string
      /** The engine's tool call id. */
      id: string
      name: string
      input: string
      state: ToolState
      view: ToolView
      output: string | null
      permissionId: number | null
      subagent?: Subagent
      /** While the model writes the input (state `writing`), and after. */
      progress?: ToolInputProgress | null
      /** What a running shell command printed so far. */
      live?: LiveOutput | null
      /** When the call appeared (ms, the time its first event was received); null when unknown. */
      startedAt?: number | null
      /** When its result arrived (ms); null while it has none. */
      endedAt?: number | null
    }
  | { kind: 'notice'; key: string; text: string; noticeKind?: NoticeKind }
  | { kind: 'report'; key: string; command: string; title: string; sections: ReportSection[]; links: ReportLink[] }
  | {
      kind: 'question'
      key: string
      text: string
      answered: boolean
      /** The engine's question id (structured questions); answer with `answer`. */
      id?: number
      options?: string[]
    }
  | {
      kind: 'error'
      key: string
      text: string
      /** A gateway failure the user can act on (sign in, plan, quota, retry, connection). */
      problem?: GatewayProblem
    }

export interface Extensions {
  mcpServers: McpServerInfo[]
  plugins: PluginInfo[]
  skills: DefinitionInfo[]
  agents: DefinitionInfo[]
}

export interface PendingPermission {
  id: number
  request: PermissionView
  /** The tool call it is for, when one is in the transcript. */
  toolKey: string | null
}

export type Connection = 'connecting' | 'starting' | 'ready' | 'exited'

export interface SessionState {
  connection: Connection
  /** Why the engine is not running, when it stopped or never started. */
  connectionError: string | null
  version: string | null
  protocol: number | null
  /** What the engine adds to protocol 1 (`ready.features`). */
  features: string[]
  status: StatusEvent | null
  /** The workspace's saved chats, once asked for. */
  sessions: SessionSummary[] | null
  /** The agent's task list. */
  todos: TodoItem[]
  /** The workspace was trusted; its settings load once the engine restarts. */
  restartRequired: boolean
  /** MCP servers, plugins, skills and agents, once asked for (feature `extensions`). */
  extensions: Extensions | null
  /** The Zero-X account, from `account` events (feature `account`); `null` before one or when signed out. */
  account: AccountDetails | null
  /** What trusting the workspace would turn on, once asked for (feature `trust_preview`). */
  trustPreview: TrustPreviewEvent | null
  models: ModelInfo[]
  /** The current turn (or slash command) has not finished. */
  busy: boolean
  /** The engine is opening the session after a sign-in. */
  sessionStarting: boolean
  signInCode: SignInCodeEvent | null
  items: TranscriptItem[]
  permissions: PendingPermission[]
  usage: { turn: TokenCounts; cumulative: TokenCounts; estimated: boolean } | null
  /** The turn as it streams (feature `live_progress`); kept after the turn ends, reset by the next one. */
  turnProgress: { outputTokens: number; requests: TokenCounts[] } | null
  /** Every file the agent changed this session; null until a `changes` event. */
  changes: ChangedFile[] | null
  /** The last `/diff` answer. */
  diff: DiffEvent | null
  /** A drafted commit, until it is sent or dismissed. */
  commitProposal: CommitProposalEvent | null
  /** The "go back to" list, once asked for (`/rewind`). */
  checkpoints: CheckpointInfo[] | null
  /** The last rewind answer: a dry-run preview or a real rewind. */
  rewound: RewoundEvent | null
  /** The latest gateway failure, until the next prompt or sign-in. */
  problem: GatewayProblem | null
  /**
   * A prompt a gateway failure stopped before any reply: UIs put it back in the
   * message box, so nothing typed is lost.
   */
  unsent: { text: string; attachments: Attachment[] } | null
  nextKey: number
}

export const initialSession = (): SessionState => ({
  connection: 'connecting',
  connectionError: null,
  version: null,
  protocol: null,
  features: [],
  status: null,
  sessions: null,
  todos: [],
  restartRequired: false,
  extensions: null,
  account: null,
  trustPreview: null,
  models: [],
  busy: false,
  sessionStarting: false,
  signInCode: null,
  items: [],
  permissions: [],
  usage: null,
  turnProgress: null,
  changes: null,
  diff: null,
  commitProposal: null,
  checkpoints: null,
  rewound: null,
  problem: null,
  unsent: null,
  nextKey: 1,
})

/** Things the client does that the engine does not echo back. */
export type LocalAction =
  | { type: 'local/connecting' }
  | { type: 'local/disconnected'; error: string | null }
  | { type: 'local/submitted'; text: string; attachments?: Attachment[] }
  | { type: 'local/slash'; command: string; args: string }
  | { type: 'local/permission_answered'; id: number; decision: PermissionDecision }
  /** Another client answered it (remote sessions). */
  | { type: 'local/permission_resolved'; id: number }
  /** The structured question with this id was answered. */
  | { type: 'local/answered'; id: number; text: string }
  /** The commit command was sent: the proposal is done with. */
  | { type: 'local/commit_sent'; message: string }
  /** The user closed the commit proposal without committing. */
  | { type: 'local/commit_dismissed' }
  /** Another project: nothing of the old session carries over. */
  | { type: 'local/reset' }

export type SessionAction = UiEvent | LocalAction

const QUESTION_PREFIX = 'Question:'

function last<T>(items: readonly T[]): T | undefined {
  return items[items.length - 1]
}

function replaceAt<T>(items: readonly T[], index: number, item: T): T[] {
  const copy = items.slice()
  copy[index] = item
  return copy
}

function findLastIndex<T>(items: readonly T[], predicate: (item: T) => boolean): number {
  for (let index = items.length - 1; index >= 0; index--) if (predicate(items[index])) return index
  return -1
}

/** `Omit` for each member of a union. */
type WithoutKey<T> = T extends unknown ? Omit<T, 'key'> : never

function push(state: SessionState, item: WithoutKey<TranscriptItem>): SessionState {
  const key = `i${state.nextKey}`
  return {
    ...state,
    nextKey: state.nextKey + 1,
    items: [...state.items, { ...item, key } as TranscriptItem],
  }
}

/** Ends whatever streams: the open text or reasoning block. */
function closeStreams(items: TranscriptItem[]): TranscriptItem[] {
  const tail = last(items)
  if (tail && (tail.kind === 'assistant' || tail.kind === 'reasoning') && tail.streaming) {
    return replaceAt(items, items.length - 1, { ...tail, streaming: false })
  }
  return items
}

type ToolItem = Extract<TranscriptItem, { kind: 'tool' }>

/** The views the engine's git and checkpoint answers fill; a new session or engine starts without them. */
const NO_GIT_VIEWS = { changes: null, diff: null, commitProposal: null, checkpoints: null, rewound: null } as const

const isPrompt = (item: TranscriptItem): boolean =>
  item.kind === 'user' && (item.prompt ?? !item.text.startsWith('/'))

/** The transcript after a real rewind: the last `removedTurns` prompts are gone, with everything after them. */
function dropRewoundTurns(items: TranscriptItem[], removedTurns: number): TranscriptItem[] {
  if (removedTurns <= 0) return items
  const prompts: number[] = []
  items.forEach((item, index) => {
    if (isPrompt(item)) prompts.push(index)
  })
  if (prompts.length === 0) return items
  return items.slice(0, prompts[Math.max(0, prompts.length - removedTurns)])
}

/** `live` with one more chunk, kept within {@link LIVE_OUTPUT_LIMIT} by dropping from the front. */
function appendLive(live: LiveOutput | null | undefined, stream: OutputStream, text: string): LiveOutput {
  const chunks = live ? live.chunks.slice() : []
  let dropped = live?.dropped ?? 0
  const tail = chunks[chunks.length - 1]
  if (tail && tail.stream === stream) chunks[chunks.length - 1] = { stream, text: tail.text + text }
  else chunks.push({ stream, text })
  let excess = chunks.reduce((sum, chunk) => sum + chunk.text.length, 0) - LIVE_OUTPUT_LIMIT
  while (excess > 0 && chunks.length > 0) {
    const head = chunks[0]
    if (head.text.length <= excess) {
      chunks.shift()
      excess -= head.text.length
      dropped += head.text.length
    } else {
      // Never start in the middle of a surrogate pair.
      let cut = excess
      const code = head.text.charCodeAt(cut)
      if (code >= 0xdc00 && code <= 0xdfff) cut += 1
      chunks[0] = { stream: head.stream, text: head.text.slice(cut) }
      dropped += cut
      excess = 0
    }
  }
  return { chunks, dropped }
}

const isOpenTool = (item: TranscriptItem): item is ToolItem =>
  item.kind === 'tool' && (item.state === 'writing' || item.state === 'running' || item.state === 'awaiting_permission')

/**
 * The tool call this id names, with its index. Some providers reuse ids
 * (`call_0` in every response), so the last open call wins, then the last call.
 */
function findTool(items: readonly TranscriptItem[], id: string): { index: number; item: ToolItem } | null {
  let index = findLastIndex(items, (item) => isOpenTool(item) && item.id === id)
  if (index < 0) index = findLastIndex(items, (item) => item.kind === 'tool' && item.id === id)
  return index < 0 ? null : { index, item: items[index] as ToolItem }
}

const closeOpenTools = (items: TranscriptItem[]): TranscriptItem[] =>
  items.map((item) =>
    item.kind === 'tool' && (item.state === 'writing' || item.state === 'running' || item.state === 'awaiting_permission')
      ? { ...item, state: 'error' as const }
      : item,
  )

/**
 * `now` is the time (ms) the action was received: it stamps tool calls. The
 * reducer reads no clock itself, so the same actions and times give the same state.
 */
export function reduceSession(state: SessionState, action: SessionAction, now?: number): SessionState {
  switch (action.type) {
    // -------------------------------------------------------------- local
    case 'local/connecting':
      return { ...state, connection: 'connecting', connectionError: null }
    case 'local/reset':
      return { ...initialSession(), nextKey: state.nextKey }
    case 'local/disconnected': {
      const items = closeStreams(state.items).map((item) =>
        item.kind === 'tool' && (item.state === 'writing' || item.state === 'running' || item.state === 'awaiting_permission')
          ? { ...item, state: 'error' as const, output: item.output ?? 'The engine stopped before this call finished.' }
          : item,
      )
      return { ...state, connection: 'exited', connectionError: action.error, busy: false, permissions: [], items }
    }
    case 'local/submitted': {
      const unanswered = findLastIndex(state.items, (item) => item.kind === 'question' && !item.answered)
      // An answer to the agent's question is a submit, but no new turn.
      let next = state
      if (unanswered >= 0) {
        const question = state.items[unanswered] as Extract<TranscriptItem, { kind: 'question' }>
        next = { ...state, items: replaceAt(state.items, unanswered, { ...question, answered: true }) }
      }
      next = push(
        { ...next, problem: null, unsent: null },
        { kind: 'user', text: action.text, attachments: action.attachments ?? [], prompt: unanswered < 0 },
      )
      return unanswered >= 0 ? next : { ...next, busy: true, turnProgress: null }
    }
    case 'local/slash':
      return {
        ...push(
          { ...state, problem: null },
          { kind: 'user', text: `/${action.command}${action.args ? ` ${action.args}` : ''}`, attachments: [], prompt: false },
        ),
        busy: true,
        turnProgress: null,
      }
    case 'local/permission_answered':
    case 'local/permission_resolved': {
      const pending = state.permissions.find((permission) => permission.id === action.id)
      if (!pending) return state
      let items = state.items
      if (pending.toolKey) {
        const index = items.findIndex((item) => item.key === pending.toolKey)
        const item = items[index]
        if (item?.kind === 'tool' && item.state === 'awaiting_permission') {
          const denied = action.type === 'local/permission_answered' && action.decision.kind === 'deny'
          items = replaceAt(items, index, { ...item, state: denied ? 'denied' : 'running' })
        }
      }
      return { ...state, items, permissions: state.permissions.filter((permission) => permission.id !== action.id) }
    }
    case 'local/answered': {
      const index = findLastIndex(state.items, (item) => item.kind === 'question' && item.id === action.id)
      if (index < 0) return state
      const question = state.items[index] as Extract<TranscriptItem, { kind: 'question' }>
      const answered = { ...state, items: replaceAt(state.items, index, { ...question, answered: true }) }
      return push(answered, { kind: 'user', text: action.text, attachments: [], prompt: false })
    }
    case 'local/commit_sent':
    case 'local/commit_dismissed':
      return state.commitProposal ? { ...state, commitProposal: null } : state

    // -------------------------------------------------------------- engine
    case 'ready':
      return {
        ...state,
        connection: 'ready',
        connectionError: null,
        version: action.version,
        protocol: action.protocol,
        features: action.features ?? [],
        restartRequired: false,
        // A restarted engine may load other servers and plugins, in another folder.
        extensions: null,
        trustPreview: null,
        turnProgress: null,
        ...NO_GIT_VIEWS,
        problem: null,
      }
    case 'status': {
      // `/clear` and `/resume` switch sessions. Engines with transcripts say so first
      // (the session id already matches); with older ones the transcript marks it.
      const before = state.status?.session_id
      const next = { ...state, status: action }
      return before && action.session_id && before !== action.session_id
        ? push(next, { kind: 'notice', text: `New session ${action.session_id}` })
        : next
    }
    case 'session_starting':
      return { ...state, sessionStarting: true, signInCode: null }
    case 'sign_in_code':
      return { ...state, signInCode: action, busy: true, problem: null }
    case 'models':
      return { ...state, models: action.models }
    case 'text_delta': {
      const tail = last(state.items)
      if (tail?.kind === 'assistant' && tail.streaming) {
        return { ...state, items: replaceAt(state.items, state.items.length - 1, { ...tail, text: tail.text + action.text }) }
      }
      return push({ ...state, items: closeStreams(state.items) }, { kind: 'assistant', text: action.text, streaming: true })
    }
    case 'text_block_end':
      return { ...state, items: closeStreams(state.items) }
    case 'reasoning_started':
      return push(
        { ...state, items: closeStreams(state.items) },
        { kind: 'reasoning', text: '', chars: action.chars, redacted: action.redacted, streaming: true },
      )
    case 'reasoning_delta': {
      const tail = last(state.items)
      if (tail?.kind === 'reasoning' && tail.streaming) {
        return { ...state, items: replaceAt(state.items, state.items.length - 1, { ...tail, text: tail.text + action.text }) }
      }
      return push(
        { ...state, items: closeStreams(state.items) },
        { kind: 'reasoning', text: action.text, chars: null, redacted: false, streaming: true },
      )
    }
    case 'tool_input_started': {
      if (action.id && state.items.some((item) => isOpenTool(item) && item.id === action.id)) return state
      return push(
        { ...state, items: closeStreams(state.items) },
        {
          kind: 'tool',
          id: action.id,
          name: action.name,
          input: '',
          state: 'writing',
          view: toolView(action.name, '{}', state.status?.cwd),
          output: null,
          permissionId: null,
          progress: null,
          live: null,
          startedAt: now ?? null,
          endedAt: null,
        },
      )
    }
    case 'tool_input_progress': {
      const found = findTool(state.items, action.id)
      if (!found) return state
      const { item, index } = found
      const progress = { bytes: action.bytes, lines: action.lines, target: action.target, preview: action.preview }
      let view = item.view
      if (action.target && item.state === 'writing') {
        const fileCall = view.kind === 'read' || view.kind === 'write' || view.kind === 'edit'
        view = { ...view, target: fileCall ? relativeTo(state.status?.cwd, action.target) : action.target }
      }
      return { ...state, items: replaceAt(state.items, index, { ...item, progress, view }) }
    }
    case 'output_progress':
      return { ...state, turnProgress: { outputTokens: action.output_tokens, requests: state.turnProgress?.requests ?? [] } }
    case 'request_usage':
      return { ...state, turnProgress: { outputTokens: 0, requests: [...(state.turnProgress?.requests ?? []), action.usage] } }
    case 'tool_output_delta': {
      if (!action.chunk) return state
      const found = findTool(state.items, action.tool_use_id)
      if (!found) return state
      const live = appendLive(found.item.live, action.stream, action.chunk)
      return { ...state, items: replaceAt(state.items, found.index, { ...found.item, live }) }
    }
    case 'tool_call_started': {
      const view = toolView(action.name, action.input, state.status?.cwd)
      const lastIndex = action.id ? findLastIndex(state.items, (item) => item.kind === 'tool' && item.id === action.id) : -1
      const found = lastIndex < 0 ? null : { index: lastIndex, item: state.items[lastIndex] as ToolItem }
      // A finished call with this id is an earlier one: a reused id starts a new card.
      if (found && (found.item.state === 'running' || found.item.state === 'awaiting_permission')) return state
      if (found && found.item.state === 'writing') {
        // The call the model was writing: the input is complete now.
        const item: ToolItem = {
          ...found.item,
          input: action.input,
          view,
          state: action.cut_off ? 'cut_off' : 'running',
          startedAt: found.item.startedAt ?? now ?? null,
        }
        return { ...state, items: replaceAt(state.items, found.index, item) }
      }
      return push(
        { ...state, items: closeStreams(state.items) },
        {
          kind: 'tool',
          id: action.id,
          name: action.name,
          input: action.input,
          state: action.cut_off ? 'cut_off' : 'running',
          view,
          output: null,
          permissionId: null,
          progress: null,
          live: null,
          startedAt: now ?? null,
          endedAt: null,
        },
      )
    }
    case 'tool_result': {
      // By id; engines before ids match the oldest open call with the name.
      const open = (item: TranscriptItem) =>
        item.kind === 'tool' && (item.state === 'running' || item.state === 'awaiting_permission' || item.state === 'cut_off')
      let index = action.id
        ? (findTool(state.items, action.id)?.index ?? -1)
        : state.items.findIndex((item) => open(item) && item.kind === 'tool' && item.name === action.name)
      if (index < 0) {
        // A result for a call this client never saw (it attached late).
        const started = reduceSession(
          state,
          { type: 'tool_call_started', id: action.id ?? '', name: action.name, input: '{}', cut_off: false },
          now,
        )
        return reduceSession(started, action, now)
      }
      const item = state.items[index] as Extract<TranscriptItem, { kind: 'tool' }>
      const denied = item.state === 'denied' || (action.is_error && /denied by (the )?user/.test(action.output))
      const next: TranscriptItem = {
        ...item,
        state: denied ? 'denied' : action.is_error ? (item.state === 'cut_off' ? 'cut_off' : 'error') : 'done',
        output: action.output,
        // The full output is in `output`: free the live copy.
        live: null,
        view: withResult(item.view, item.name, action.output, action.is_error),
        endedAt: now ?? null,
      }
      // Its permission request is moot once it has a result.
      const permissions = state.permissions.filter((permission) => permission.toolKey !== item.key)
      return { ...state, items: replaceAt(state.items, index, next), permissions }
    }
    case 'usage':
      return { ...state, usage: { turn: action.turn, cumulative: action.cumulative, estimated: action.estimated } }
    case 'permission_requested': {
      if (state.permissions.some((permission) => permission.id === action.id)) return state
      // The call waiting for it: the newest open call of that tool with that input.
      const index = findLastIndex(
        state.items,
        (item) =>
          item.kind === 'tool' &&
          item.state === 'running' &&
          item.name === action.request.tool &&
          (item.input === action.request.input || sameJson(item.input, action.request.input)),
      )
      let items = state.items
      let toolKey: string | null = null
      if (index >= 0) {
        const item = items[index] as Extract<TranscriptItem, { kind: 'tool' }>
        toolKey = item.key
        items = replaceAt(items, index, { ...item, state: 'awaiting_permission', permissionId: action.id })
      }
      return { ...state, items, permissions: [...state.permissions, { id: action.id, request: action.request, toolKey }] }
    }
    case 'notice':
      if (action.message.startsWith(QUESTION_PREFIX)) {
        return push(state, { kind: 'question', text: action.message.slice(QUESTION_PREFIX.length).trim(), answered: false })
      }
      return push(
        state,
        action.kind ? { kind: 'notice', text: action.message, noticeKind: action.kind } : { kind: 'notice', text: action.message },
      )
    case 'report':
      return push(
        { ...state, items: closeStreams(state.items) },
        { kind: 'report', command: action.command, title: action.title, sections: action.sections, links: action.links },
      )
    case 'turn_finished': {
      const items = closeOpenTools(closeStreams(state.items))
      return { ...state, items, busy: false, sessionStarting: false, signInCode: null, permissions: [] }
    }
    case 'error': {
      const problem = gatewayProblem(action)
      const item = problem ? { kind: 'error' as const, text: action.message, problem } : { kind: 'error' as const, text: action.message }
      const next = problem ? { ...state, problem, unsent: unsentPrompt(state) ?? state.unsent } : state
      if (state.connection !== 'ready') {
        return push({ ...next, connectionError: action.message }, item)
      }
      return push({ ...next, items: closeStreams(next.items) }, item)
    }
    case 'changes':
      return { ...state, changes: action.files }
    case 'diff':
      return { ...state, diff: action }
    case 'commit_proposal':
      return { ...state, commitProposal: action }
    case 'checkpoints':
      return { ...state, checkpoints: action.items }
    case 'rewound': {
      if (action.dry_run) return { ...state, rewound: action }
      const items = dropRewoundTurns(state.items, action.removed_turns)
      const keys = new Set(items.map((item) => item.key))
      return {
        ...state,
        rewound: action,
        items,
        permissions: state.permissions.filter((permission) => permission.toolKey === null || keys.has(permission.toolKey)),
        unsent: action.prompt ? { text: action.prompt, attachments: [] } : state.unsent,
        // The list changed: the UI asks again.
        checkpoints: null,
      }
    }
    case 'sessions':
      return { ...state, sessions: action.sessions }
    case 'transcript':
      return restoreTranscript(state, action.session_id, action.entries, action.truncated)
    case 'account':
      return { ...state, account: action.signed_in ? action.account : null }
    case 'trust_preview':
      return { ...state, trustPreview: action }
    case 'workspace_trusted':
      return {
        ...state,
        trustPreview: null,
        restartRequired: action.restart_required,
        status: state.status ? { ...state.status, workspace_trusted: true } : state.status,
      }
    case 'question':
      return push(state, { kind: 'question', id: action.id, text: action.question, options: action.options, answered: false })
    case 'todos':
      return { ...state, todos: action.items }
    case 'extensions':
      return {
        ...state,
        extensions: { mcpServers: action.mcp_servers, plugins: action.plugins, skills: action.skills, agents: action.agents },
      }
    case 'subagent_started':
    case 'subagent_finished': {
      const index = findLastIndex(state.items, (item) => item.kind === 'tool' && item.id === action.id)
      if (index < 0) return state
      const item = state.items[index] as Extract<TranscriptItem, { kind: 'tool' }>
      const subagent: Subagent =
        action.type === 'subagent_started'
          ? { description: action.description, type: action.subagent_type, running: true, status: null, result: null, note: null }
          : {
              description: item.subagent?.description ?? item.view.target,
              type: item.subagent?.type ?? null,
              running: false,
              status: action.status,
              result: action.result,
              note: action.note,
            }
      return { ...state, items: replaceAt(state.items, index, { ...item, subagent }) }
    }
    // Answers and progress that no screen of the desktop or the phone shows yet, so nothing is kept.
    // (An engine sends hook events only to a UI that declares `hook_progress`, and none does.)
    case 'branches':
    case 'connector_catalog':
    case 'hook_started':
    case 'hook_finished':
      return state
    default: {
      // Exhaustive: a `UiEvent` or `LocalAction` without a case above fails `bun run typecheck` here. The
      // runtime `return state` stays all the same, because a newer host can send an action this client does not know.
      const unreachable: never = action
      void unreachable
      return state
    }
  }
}

/**
 * The transcript of the session the engine now runs: the saved conversation in
 * place of what was shown, tool calls with their results folded in. The footer's
 * session id follows, so the status after it does not read as another switch.
 */
function restoreTranscript(state: SessionState, sessionId: string, entries: TranscriptEntry[], truncated: boolean): SessionState {
  let next: SessionState = {
    ...state,
    items: [],
    permissions: [],
    todos: [],
    ...NO_GIT_VIEWS,
    status: state.status ? { ...state.status, session_id: sessionId } : state.status,
  }
  if (truncated) next = push(next, { kind: 'notice', text: 'Earlier messages of this chat are not shown.' })
  for (const entry of entries) {
    switch (entry.kind) {
      case 'user':
        next = push(next, { kind: 'user', text: entry.text, attachments: [], prompt: true })
        break
      case 'assistant':
        next = push(next, { kind: 'assistant', text: entry.text, streaming: false })
        break
      case 'tool_call':
        next = push(next, {
          kind: 'tool',
          id: entry.id,
          name: entry.name,
          input: entry.input,
          // Without a result it never finished (interrupted, or the engine stopped).
          state: 'error',
          view: toolView(entry.name, entry.input, state.status?.cwd),
          output: null,
          permissionId: null,
          progress: null,
          live: null,
          startedAt: null,
          endedAt: null,
        })
        break
      case 'tool_result': {
        const index = findLastIndex(next.items, (item) => item.kind === 'tool' && item.id === entry.id)
        if (index < 0) break
        const item = next.items[index] as Extract<TranscriptItem, { kind: 'tool' }>
        const denied = entry.is_error && /denied by (the )?user/.test(entry.output)
        next = {
          ...next,
          items: replaceAt(next.items, index, {
            ...item,
            state: denied ? 'denied' : entry.is_error ? 'error' : 'done',
            output: entry.output,
            view: withResult(item.view, item.name, entry.output, entry.is_error),
          }),
        }
        break
      }
    }
  }
  return next
}

function sameJson(a: string, b: string): boolean {
  try {
    return JSON.stringify(JSON.parse(a)) === JSON.stringify(JSON.parse(b))
  } catch {
    return false
  }
}

// ---------------------------------------------------------------- selectors

/** The sign-in screen is needed: use this, not `signed_in` (API-key sessions need none). */
/**
 * The prompt of the turn now failing, if nothing answered it yet: the last
 * user message, with no reply, tool call or question after it.
 */
function unsentPrompt(state: SessionState): { text: string; attachments: Attachment[] } | null {
  const index = findLastIndex(state.items, (item) => item.kind === 'user')
  if (index < 0) return null
  const user = state.items[index] as Extract<TranscriptItem, { kind: 'user' }>
  if (user.text.startsWith('/')) return null
  const answered = state.items
    .slice(index + 1)
    .some((item) => item.kind === 'assistant' || item.kind === 'tool' || item.kind === 'question')
  return answered ? null : { text: user.text, attachments: user.attachments }
}

export function needsSignIn(state: SessionState): boolean {
  return state.status?.sign_in_required === true
}

export function account(state: SessionState): AccountInfo | null {
  return state.status?.account ?? null
}

/** The agent waits for an answer to a question. */
export function openQuestion(state: SessionState): string | null {
  return openQuestionItem(state)?.text ?? null
}

/** The unanswered question, with its id and options when the engine sent them. */
export function openQuestionItem(state: SessionState): Extract<TranscriptItem, { kind: 'question' }> | null {
  const item = findLastIndex(state.items, (entry) => entry.kind === 'question' && !entry.answered)
  return item >= 0 ? (state.items[item] as Extract<TranscriptItem, { kind: 'question' }>) : null
}

/**
 * What a chat needs from the person looking at it: `needs_you` (a permission or
 * a question waits), `running` (a turn is going), `stopped` (the engine is gone)
 * or `idle`. A stopped engine is `stopped` even when a question is still open:
 * nothing can answer it.
 */
export function chatActivity(state: SessionState): 'needs_you' | 'running' | 'idle' | 'stopped' {
  if (state.connection === 'exited') return 'stopped'
  if (state.permissions.length > 0 || openQuestionItem(state)) return 'needs_you'
  return state.busy ? 'running' : 'idle'
}

/** What a running command printed so far, as one text (stdout and stderr in the order they came). */
export function liveText(live: LiveOutput | null | undefined): string {
  return live ? live.chunks.map((chunk) => chunk.text).join('') : ''
}

/** The engine lists this feature on `ready`. */
export function hasFeature(state: SessionState, feature: string): boolean {
  return state.features.includes(feature)
}

/** The `Agent` calls whose sub-agents run now. */
export function runningSubagents(state: SessionState): Array<Extract<TranscriptItem, { kind: 'tool' }>> {
  return state.items.filter(
    (item): item is Extract<TranscriptItem, { kind: 'tool' }> => item.kind === 'tool' && item.subagent?.running === true,
  )
}

/** The paths a session's tool calls wrote or edited, newest first, for a "changes" list. */
export function changedPaths(state: SessionState): string[] {
  const seen = new Set<string>()
  const paths: string[] = []
  for (let index = state.items.length - 1; index >= 0; index--) {
    const item = state.items[index]
    if (item.kind !== 'tool' || item.state !== 'done') continue
    if (item.view.kind !== 'write' && item.view.kind !== 'edit') continue
    if (!seen.has(item.view.target)) {
      seen.add(item.view.target)
      paths.push(item.view.target)
    }
  }
  return paths
}

export function formatTokens(count: number): string {
  if (count < 1000) return String(count)
  if (count < 1_000_000) return `${(count / 1000).toFixed(count < 10_000 ? 1 : 0)}k`
  return `${(count / 1_000_000).toFixed(1)}M`
}
