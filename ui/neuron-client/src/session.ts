/**
 * A session as a client shows it, built from the engine's events and the
 * user's own actions by a pure reducer. The desktop app, the mobile app and a
 * remote host all keep their state this way, so every front end reads the
 * same transcript from the same events.
 */

import type {
  AccountInfo,
  Attachment,
  ModelInfo,
  PermissionDecision,
  PermissionView,
  SignInCodeEvent,
  StatusEvent,
  TokenCounts,
  UiEvent,
} from './protocol'
import { toolView, withResult, type ToolView } from './tools'

export type ToolState = 'running' | 'awaiting_permission' | 'done' | 'error' | 'denied' | 'cut_off'

export type TranscriptItem =
  | { kind: 'user'; key: string; text: string; attachments: Attachment[] }
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
    }
  | { kind: 'notice'; key: string; text: string }
  | { kind: 'question'; key: string; text: string; answered: boolean }
  | { kind: 'error'; key: string; text: string }

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
  status: StatusEvent | null
  models: ModelInfo[]
  /** The current turn (or slash command) has not finished. */
  busy: boolean
  /** The engine is opening the session after a sign-in. */
  sessionStarting: boolean
  signInCode: SignInCodeEvent | null
  items: TranscriptItem[]
  permissions: PendingPermission[]
  usage: { turn: TokenCounts; cumulative: TokenCounts; estimated: boolean } | null
  nextKey: number
}

export const initialSession = (): SessionState => ({
  connection: 'connecting',
  connectionError: null,
  version: null,
  protocol: null,
  status: null,
  models: [],
  busy: false,
  sessionStarting: false,
  signInCode: null,
  items: [],
  permissions: [],
  usage: null,
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

export function reduceSession(state: SessionState, action: SessionAction): SessionState {
  switch (action.type) {
    // -------------------------------------------------------------- local
    case 'local/connecting':
      return { ...state, connection: 'connecting', connectionError: null }
    case 'local/disconnected': {
      const items = closeStreams(state.items).map((item) =>
        item.kind === 'tool' && (item.state === 'running' || item.state === 'awaiting_permission')
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
      next = push(next, { kind: 'user', text: action.text, attachments: action.attachments ?? [] })
      return unanswered >= 0 ? next : { ...next, busy: true }
    }
    case 'local/slash':
      return { ...push(state, { kind: 'user', text: `/${action.command}${action.args ? ` ${action.args}` : ''}`, attachments: [] }), busy: true }
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

    // -------------------------------------------------------------- engine
    case 'ready':
      return { ...state, connection: 'ready', connectionError: null, version: action.version, protocol: action.protocol }
    case 'status': {
      // `/clear` and `/resume` switch sessions; the protocol can't replay the new one's
      // history yet, so the transcript says where the old one ended.
      const before = state.status?.session_id
      const next = { ...state, status: action }
      return before && action.session_id && before !== action.session_id
        ? push(next, { kind: 'notice', text: `New session ${action.session_id}` })
        : next
    }
    case 'session_starting':
      return { ...state, sessionStarting: true, signInCode: null }
    case 'sign_in_code':
      return { ...state, signInCode: action, busy: true }
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
    case 'tool_call_started':
      return push(
        { ...state, items: closeStreams(state.items) },
        {
          kind: 'tool',
          id: action.id,
          name: action.name,
          input: action.input,
          state: action.cut_off ? 'cut_off' : 'running',
          view: toolView(action.name, action.input, state.status?.cwd),
          output: null,
          permissionId: null,
        },
      )
    case 'tool_result': {
      // By id; engines before ids match the oldest open call with the name.
      const open = (item: TranscriptItem) =>
        item.kind === 'tool' && (item.state === 'running' || item.state === 'awaiting_permission' || item.state === 'cut_off')
      let index = action.id
        ? state.items.findIndex((item) => item.kind === 'tool' && item.id === action.id)
        : state.items.findIndex((item) => open(item) && item.kind === 'tool' && item.name === action.name)
      if (index < 0) {
        // A result for a call this client never saw (it attached late).
        const started = reduceSession(state, {
          type: 'tool_call_started',
          id: action.id ?? '',
          name: action.name,
          input: '{}',
          cut_off: false,
        })
        return reduceSession(started, action)
      }
      const item = state.items[index] as Extract<TranscriptItem, { kind: 'tool' }>
      const denied = item.state === 'denied' || (action.is_error && /denied by (the )?user/.test(action.output))
      const next: TranscriptItem = {
        ...item,
        state: denied ? 'denied' : action.is_error ? (item.state === 'cut_off' ? 'cut_off' : 'error') : 'done',
        output: action.output,
        view: withResult(item.view, item.name, action.output, action.is_error),
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
      return push(state, { kind: 'notice', text: action.message })
    case 'turn_finished': {
      const items = closeStreams(state.items).map((item) =>
        item.kind === 'tool' && (item.state === 'running' || item.state === 'awaiting_permission')
          ? { ...item, state: 'error' as const }
          : item,
      )
      return { ...state, items, busy: false, sessionStarting: false, signInCode: null, permissions: [] }
    }
    case 'error':
      if (state.connection !== 'ready') {
        return push({ ...state, connectionError: action.message }, { kind: 'error', text: action.message })
      }
      return push({ ...state, items: closeStreams(state.items) }, { kind: 'error', text: action.message })
    default:
      return state
  }
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
export function needsSignIn(state: SessionState): boolean {
  return state.status?.sign_in_required === true
}

export function account(state: SessionState): AccountInfo | null {
  return state.status?.account ?? null
}

/** The agent waits for an answer to a question. */
export function openQuestion(state: SessionState): string | null {
  const item = findLastIndex(state.items, (entry) => entry.kind === 'question' && !entry.answered)
  return item >= 0 ? (state.items[item] as Extract<TranscriptItem, { kind: 'question' }>).text : null
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
