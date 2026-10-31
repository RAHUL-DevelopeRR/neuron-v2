import { create } from 'zustand'

import type { TodoItem } from '../utils/todos'

/**
 * A tool call of the running turn, for the status line: the model is writing
 * its input (`input`), or it started and has no result yet (`running`).
 */
export interface LiveToolCall {
  id: string
  /** The engine's tool name (`write_file`, `bash`, ...). */
  name: string
  phase: 'input' | 'running'
  /** A path, command, pattern or URL, once known. */
  target: string | null
  /** Lines and bytes of the input so far (while it streams). */
  lines: number
  bytes: number
}

/**
 * Where the current model request is: sent and nothing back yet (`waiting`),
 * tokens coming in (`streaming`), or over (`idle`: its tool calls run, or the
 * turn ends).
 */
export type ModelPhase = 'waiting' | 'streaming' | 'idle'

/**
 * The running turn as the status line, the todo strip and the footer show it
 * while it streams (see engine/live-turn.ts, which keeps it). Times are the
 * live-turn clock's milliseconds.
 */
export interface LiveTurnState {
  /** A turn is running. */
  active: boolean
  /** Its tool calls without a result, in the order they began. */
  calls: LiveToolCall[]
  model: ModelPhase
  /** What the model streams now, as far as the UI can tell. */
  stream: 'thinking' | 'responding' | null
  /** When the current request was sent (turn start, after tool results, a retry). */
  requestSentAt: number | null
  /**
   * When the last token-bearing event came: text, reasoning, tool input or an
   * output estimate. `null` before the current request's first one.
   */
  lastTokenAt: number | null
  /** Output tokens of this turn's finished requests (`request_usage`). */
  turnOutput: number
  /** The estimate for the response now streaming (`output_progress`). */
  streamingOutput: number
  /**
   * Tokens of the requests finished since the last `usage` event, which the
   * footer adds to that event's totals until the next one settles them.
   */
  unsettledInput: number
  unsettledOutput: number
  /** The agent's latest task list; `null` until it makes one. */
  todos: TodoItem[] | null
  /** The list changed during the running turn. */
  todosThisTurn: boolean
}

export const INITIAL_LIVE_TURN: LiveTurnState = {
  active: false,
  calls: [],
  model: 'idle',
  stream: null,
  requestSentAt: null,
  lastTokenAt: null,
  turnOutput: 0,
  streamingOutput: 0,
  unsettledInput: 0,
  unsettledOutput: 0,
  todos: null,
  todosThisTurn: false,
}

export const useLiveTurnStore = create<LiveTurnState>()(() => ({
  ...INITIAL_LIVE_TURN,
}))

/** Back to no turn and no task list (a new engine, a test). */
export function resetLiveTurnStore(): void {
  useLiveTurnStore.setState({ ...INITIAL_LIVE_TURN }, true)
}
