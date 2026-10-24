/**
 * Types the UI used to import from `@codebuff/sdk`, kept as local definitions
 * now that the SDK is gone. The Neuron engine runs the agent and keeps the
 * conversation; the UI only renders what it reports (see src/engine/).
 */

export type { ToolName } from '@neuron-tui/common/tools/constants'

/** A text part of a user prompt. */
export type TextContent = {
  type: 'text'
  text: string
}

/** An image part of a user prompt, base64 encoded. */
export type ImageContent = {
  type: 'image'
  image: string
  mediaType: string
}

export type MessageContent = TextContent | ImageContent

/**
 * What a turn leaves behind, saved beside the transcript so `--continue` can
 * pick the chat up again. Freebuff stored the SDK's whole run state here; the
 * engine keeps the conversation itself, so this holds only what the UI reads.
 * P2 records the engine's session id so `--continue` can resume it.
 */
export interface RunState {
  traceSessionId?: string
  /**
   * The engine session (from its `status` event), for `--resume`. Saved as
   * soon as the engine reports it, so a chat whose turns were all interrupted
   * still resumes its session (see engine/session-run-state.ts).
   */
  sessionId?: string
  /** The model that session was on (its `status` event). */
  model?: string
  /** How the turn ended: `lastMessage`, or `error` with its message. */
  output: {
    type: string
    message?: string
    [key: string]: unknown
  }
  sessionState?: {
    mainAgentState?: {
      contextTokenCount?: number
      messageHistory?: unknown[]
    }
  }
}

/** Shown when a turn ends before its response completed. */
export const STATE_SNAPSHOT_INTERRUPTION_MESSAGE =
  'The session ended before this response completed. Partial progress has been preserved.'
