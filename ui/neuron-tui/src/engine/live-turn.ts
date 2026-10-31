/**
 * The running turn as it streams, for everything outside its transcript: the
 * status line's verb, target and token count, a stalled stream, the todo
 * strip, and the footer's live totals. The turn's event sink
 * (./turn-events.ts) feeds every event of the turn through
 * {@link trackLiveTurnEvent}; the state lives in state/live-turn-store.ts.
 *
 * Engines with `live_progress` report tool input while the model writes it,
 * output estimates and per-request usage. Older ones send text, reasoning,
 * tool calls and results only: the same verbs follow from those, with no
 * token count until the turn's `usage`.
 */

import {
  INITIAL_LIVE_TURN,
  useLiveTurnStore,
  type LiveToolCall,
  type LiveTurnState,
} from '../state/live-turn-store'
import { formatElapsedTime } from '../utils/format-elapsed-time'
import { toTodoStatus, type TodoItem } from '../utils/todos'
import { displayPath, mapToolCall } from './tool-mapping'

import type { TodoItem as EngineTodoItem, UiEvent } from './protocol'

/** A streaming request that sent nothing for this long reads as stalled. */
export const STALL_AFTER_MS = 10_000

// ------------------------------------------------------------------- targets

const parseObject = (json: string): Record<string, unknown> | null => {
  try {
    const value = JSON.parse(json)
    return value && typeof value === 'object' && !Array.isArray(value)
      ? (value as Record<string, unknown>)
      : null
  } catch {
    return null
  }
}

const firstLine = (text: string): string => text.trim().split(/\r?\n/)[0]!.trim()

/** Which input field says what a call is about, by engine tool name. */
const TARGET_FIELDS: Record<string, string> = {
  write_file: 'path',
  edit_file: 'path',
  read_file: 'path',
  NotebookEdit: 'notebook_path',
  bash: 'command',
  PowerShell: 'command',
  glob_search: 'pattern',
  grep_search: 'pattern',
  WebFetch: 'url',
  WebSearch: 'query',
  Agent: 'description',
  Skill: 'skill',
}

/**
 * What a call is about, from its complete input (JSON text): the path,
 * command, pattern, URL or description; `null` when the tool has none.
 */
export function targetOfToolInput(name: string, input: string): string | null {
  const field = TARGET_FIELDS[name]
  if (!field) return null
  const value = parseObject(input)?.[field]
  if (typeof value !== 'string' || !value.trim()) return null
  return normalizeTarget(name, value)
}

/** A target as the status line shows it: one line, paths without `\\?\`. */
export function normalizeTarget(name: string, target: string): string {
  const line = firstLine(target)
  return TARGET_FIELDS[name] === 'path' ? displayPath(line) : line
}

// ---------------------------------------------------------------- todos

/** The engine's `todos` items as the UI's todo list. */
export function todosFromEvent(items: EngineTodoItem[]): TodoItem[] {
  return items.map((item) => ({
    task: item.content,
    completed: item.status === 'completed',
    status: toTodoStatus(item.status),
    ...(item.active_form && { activeForm: item.active_form }),
  }))
}

/** A `TodoWrite` call's list, for engines without the `todos` event. */
function todosFromToolInput(input: string): TodoItem[] | null {
  const todos = mapToolCall('TodoWrite', input).input.todos
  return Array.isArray(todos) ? (todos as TodoItem[]) : null
}

// --------------------------------------------------------------- reducer

const withCall = (
  calls: LiveToolCall[],
  id: string,
  update: (call: LiveToolCall | undefined) => LiveToolCall,
): LiveToolCall[] => {
  const index = calls.findIndex((call) => call.id === id)
  if (index < 0) return [...calls, update(undefined)]
  const next = [...calls]
  next[index] = update(calls[index])
  return next
}

/** A token-bearing event: the request streams, and its silence ends. */
const tokenArrived = (
  state: LiveTurnState,
  now: number,
  patch: Partial<LiveTurnState> = {},
): LiveTurnState => ({ ...state, ...patch, model: 'streaming', lastTokenAt: now })

/** A new model request was sent: nothing back from it yet. */
const requestSent = (state: LiveTurnState, now: number): LiveTurnState => ({
  ...state,
  model: 'waiting',
  stream: null,
  requestSentAt: now,
  lastTokenAt: null,
  streamingOutput: 0,
})

/** A turn starts: its first request is being sent. */
export function startedLiveTurn(state: LiveTurnState, now: number): LiveTurnState {
  return requestSent(
    { ...state, active: true, calls: [], turnOutput: 0, todosThisTurn: false },
    now,
  )
}

/**
 * The turn ended. The footer keeps the unsettled tokens until a `usage`
 * event settles them; the task list stays for the next turn and `/todo`.
 */
export function endedLiveTurn(state: LiveTurnState): LiveTurnState {
  return {
    ...INITIAL_LIVE_TURN,
    unsettledInput: state.unsettledInput,
    unsettledOutput: state.unsettledOutput,
    todos: state.todos,
  }
}

/**
 * The engine sends the request again: what the failed attempt streamed is
 * dropped, tool input it was writing included, and the wait starts over.
 */
export function retriedLiveTurn(state: LiveTurnState, now: number): LiveTurnState {
  return requestSent(
    { ...state, calls: state.calls.filter((call) => call.phase !== 'input') },
    now,
  )
}

/** The state after one engine event of the running turn, at `now`. */
export function reduceLiveTurn(
  state: LiveTurnState,
  event: UiEvent,
  now: number,
): LiveTurnState {
  switch (event.type) {
    case 'text_delta':
      return event.text ? tokenArrived(state, now, { stream: 'responding' }) : state
    case 'reasoning_delta':
      return event.text ? tokenArrived(state, now, { stream: 'thinking' }) : state
    case 'reasoning_started':
      // The model began a reasoning block: its tokens are arriving, also
      // when the provider hides them.
      return tokenArrived(state, now, { stream: 'thinking' })
    case 'tool_input_started':
      return tokenArrived(state, now, {
        calls: withCall(state.calls, event.id, (call) =>
          call ?? { id: event.id, name: event.name, phase: 'input', target: null, lines: 0, bytes: 0 },
        ),
      })
    case 'tool_input_progress':
      return tokenArrived(state, now, {
        calls: withCall(state.calls, event.id, (call) => ({
          id: event.id,
          name: event.name,
          phase: call?.phase ?? 'input',
          target: event.target ? normalizeTarget(event.name, event.target) : (call?.target ?? null),
          lines: event.lines,
          bytes: event.bytes,
        })),
      })
    case 'output_progress':
      return tokenArrived(state, now, { streamingOutput: event.output_tokens })
    case 'tool_call_started': {
      const todos = event.name === 'TodoWrite' ? todosFromToolInput(event.input) : null
      const withTodos = todos ? { todos, todosThisTurn: true } : {}
      // The response is complete (calls are reported once it ends): its
      // calls run now, or, cut off, never will.
      const calls = event.cut_off
        ? state.calls.filter((call) => call.id !== event.id)
        : withCall(state.calls, event.id, (call) => ({
            id: event.id,
            name: event.name,
            phase: 'running',
            target: targetOfToolInput(event.name, event.input) ?? call?.target ?? null,
            lines: call?.lines ?? 0,
            bytes: call?.bytes ?? 0,
          }))
      return { ...state, ...withTodos, calls, model: 'idle', stream: null }
    }
    case 'tool_result': {
      const index = event.id
        ? state.calls.findIndex((call) => call.id === event.id)
        : state.calls.findIndex((call) => call.name === event.name)
      const calls =
        index < 0 ? state.calls : state.calls.filter((_, position) => position !== index)
      const next = { ...state, calls }
      // The last result goes back to the model in a new request.
      return calls.length === 0 ? requestSent(next, now) : next
    }
    case 'subagent_started':
      return {
        ...state,
        calls: state.calls.map((call) =>
          call.id === event.id && event.description
            ? { ...call, target: firstLine(event.description) }
            : call,
        ),
      }
    case 'request_usage': {
      const { usage } = event
      return {
        ...state,
        model: 'idle',
        stream: null,
        turnOutput: state.turnOutput + usage.output,
        streamingOutput: 0,
        unsettledInput:
          state.unsettledInput + usage.input + usage.cache_read + usage.cache_write,
        unsettledOutput: state.unsettledOutput + usage.output,
      }
    }
    case 'usage':
      // The engine's totals now include every request so far.
      return { ...state, streamingOutput: 0, unsettledInput: 0, unsettledOutput: 0 }
    case 'todos':
      return { ...state, todos: todosFromEvent(event.items), todosThisTurn: true }
    case 'turn_finished':
      return endedLiveTurn(state)
    default:
      return state
  }
}

// ----------------------------------------------------------------- store

let clock: () => number = Date.now

/** The live-turn clock (milliseconds); tests set their own. */
export const liveTurnNow = (): number => clock()

/** Test seam: a fixed or stepping clock, or `null` for `Date.now`. */
export function setLiveTurnClockForTesting(next: (() => number) | null): void {
  clock = next ?? Date.now
}

const update = (next: (state: LiveTurnState) => LiveTurnState) => {
  const state = useLiveTurnStore.getState()
  const updated = next(state)
  if (updated !== state) useLiveTurnStore.setState(updated, true)
}

/** One turn's hold on the live view; see {@link startLiveTurn}. */
export interface LiveTurnHandle {
  /** One engine event of the turn. */
  track(event: UiEvent): void
  /** The engine retries the turn's request (its retry notice). */
  retry(): void
  /** The turn is over, also when it never got its `turn_finished`. */
  end(): void
}

let currentTurn = 0

/**
 * A turn's prompt is being sent: the live view is its own until the next
 * turn starts. A prompt sent right after an interrupt starts before the
 * interrupted turn's last events arrive; those no longer change the view,
 * except that its `usage` still settles the footer's totals.
 */
export function startLiveTurn(): LiveTurnHandle {
  currentTurn += 1
  const turn = currentTurn
  const owns = () => turn === currentTurn
  update((state) => startedLiveTurn(state, clock()))
  return {
    track(event) {
      if (owns()) {
        update((state) => reduceLiveTurn(state, event, clock()))
      } else if (event.type === 'usage') {
        update((state) => ({ ...state, unsettledInput: 0, unsettledOutput: 0 }))
      }
    },
    retry() {
      if (owns()) update((state) => retriedLiveTurn(state, clock()))
    },
    end() {
      if (owns()) update((state) => (state.active ? endedLiveTurn(state) : state))
    },
  }
}

// --------------------------------------------------------------- display

/** What the status line says the turn is doing. */
export interface LiveActivity {
  /** "Writing", "Running", "Thinking", "Waiting for the model", ... */
  verb: string
  /** What it is doing it to: a path, a command, a pattern. */
  target: string | null
  /** More about it: "412 lines · 12.3 KB", "no tokens for 32s". */
  detail: string | null
  /** The streaming request sent nothing for {@link STALL_AFTER_MS}. */
  stalled: boolean
}

/** Verbs for a call whose input the model is writing. */
const INPUT_VERBS: Record<string, string> = {
  write_file: 'Writing',
  edit_file: 'Editing',
  NotebookEdit: 'Editing',
}

/** Verbs for a call that runs. */
const RUNNING_VERBS: Record<string, string> = {
  bash: 'Running',
  PowerShell: 'Running',
  write_file: 'Writing',
  edit_file: 'Editing',
  NotebookEdit: 'Editing',
  read_file: 'Reading',
  glob_search: 'Searching',
  grep_search: 'Searching',
  WebFetch: 'Fetching',
  WebSearch: 'Searching the web for',
  TodoWrite: 'Updating the todo list',
  Agent: 'Running agent',
  Skill: 'Loading skill',
}

/** Tools whose verb says it all, with no target. */
const UNTARGETED = new Set(['TodoWrite'])

/** `1 KB`, `12.3 KB`, `2.1 MB`: one decimal, a trailing ".0" dropped. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  const format = (value: number, unit: string) => {
    const rounded = Math.round(value * 10) / 10
    return `${Number.isInteger(rounded) ? rounded : rounded.toFixed(1)} ${unit}`
  }
  if (bytes < 1024 * 1024) return format(bytes / 1024, 'KB')
  return format(bytes / (1024 * 1024), 'MB')
}

/** `412 lines · 12.3 KB` for input written so far; `null` before any. */
export function inputSizeDetail(lines: number, bytes: number): string | null {
  const parts: string[] = []
  if (lines > 0) parts.push(`${lines} ${lines === 1 ? 'line' : 'lines'}`)
  if (bytes > 0) parts.push(formatBytes(bytes))
  return parts.length > 0 ? parts.join(' · ') : null
}

const seconds = (ms: number) => formatElapsedTime(Math.max(0, Math.floor(ms / 1000)))

/** The streaming request's silence, when it is long enough to say. */
export function stalledFor(state: LiveTurnState, now: number): number | null {
  if (state.model !== 'streaming' || state.lastTokenAt === null) return null
  const silent = now - state.lastTokenAt
  return silent > STALL_AFTER_MS ? silent : null
}

/**
 * What the status line says at `now`. A tool that runs and a permission
 * dialog are expected silences: only a streaming request reads as stalled.
 */
export function describeLiveActivity(
  state: LiveTurnState,
  options: { now: number; awaitingApproval: boolean },
): LiveActivity {
  const { now } = options
  const running = state.calls.find((call) => call.phase === 'running')
  if (options.awaitingApproval) {
    return {
      verb: 'Waiting for approval',
      target: running?.target ?? null,
      detail: null,
      stalled: false,
    }
  }
  const silent = stalledFor(state, now)
  const stall: LiveActivity | null =
    silent === null
      ? null
      : {
          verb: 'Waiting for the model',
          target: null,
          detail: `no tokens for ${seconds(silent)}`,
          stalled: true,
        }
  const writing = [...state.calls].reverse().find((call) => call.phase === 'input')
  if (writing) {
    return (
      stall ?? {
        verb: INPUT_VERBS[writing.name] ?? 'Preparing',
        target: writing.target ?? (INPUT_VERBS[writing.name] ? null : writing.name),
        detail: inputSizeDetail(writing.lines, writing.bytes),
        stalled: false,
      }
    )
  }
  if (running) {
    return {
      verb: RUNNING_VERBS[running.name] ?? 'Running',
      target: UNTARGETED.has(running.name)
        ? null
        : (running.target ?? (RUNNING_VERBS[running.name] ? null : running.name)),
      detail: null,
      stalled: false,
    }
  }
  if (state.model === 'streaming') {
    return (
      stall ?? {
        verb: state.stream === 'thinking' ? 'Thinking' : 'Responding',
        target: null,
        detail: null,
        stalled: false,
      }
    )
  }
  if (state.model === 'waiting' || !state.active) {
    const waited = state.requestSentAt === null ? 0 : now - state.requestSentAt
    return {
      verb: 'Waiting for the model',
      target: null,
      detail: waited > STALL_AFTER_MS ? seconds(waited) : null,
      stalled: false,
    }
  }
  return { verb: 'Working', target: null, detail: null, stalled: false }
}

/** This turn's output tokens so far: finished requests plus the one streaming. */
export function liveOutputTokens(state: LiveTurnState): number {
  return state.turnOutput + state.streamingOutput
}

/**
 * What the footer adds to the last `usage` totals while a turn streams:
 * requests finished since then and the response now streaming. `null` when
 * there is nothing to add (older engines, or right after `usage`).
 */
export function liveFooterTokens(
  state: Pick<LiveTurnState, 'unsettledInput' | 'unsettledOutput' | 'streamingOutput'>,
): { input: number; output: number } | null {
  const output = state.unsettledOutput + state.streamingOutput
  if (state.unsettledInput === 0 && output === 0) return null
  return { input: state.unsettledInput, output }
}
