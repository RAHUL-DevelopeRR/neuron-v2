/**
 * The engine session a chat continues in, kept in the chat's run state
 * (`run-state.json`) so `--continue` can `--resume` it.
 *
 * The engine names its session in every `status` event (after `ready`, after
 * each turn and model switch). A turn used to record it only when it
 * completed, so a chat whose only turn was interrupted kept none, and
 * `--continue` started a fresh engine session under the old transcript. The
 * id is now taken from the engine store whenever the run state is saved, and
 * written into the saved chat as soon as a `status` names it.
 */

import { useEngineStore } from '../state/engine-store'

import type { StatusEvent } from './protocol'
import type { RunState } from '../types/agent-run'

/** An engine session: its id, and the model it was on. */
export interface EngineSessionRef {
  sessionId: string
  model?: string
}

/**
 * The session a `status` names. `null` before the engine has one: no status
 * yet, or the signed-out stage, whose `session_id` is empty.
 */
export function engineSessionOf(
  status: Pick<StatusEvent, 'session_id' | 'model'> | null | undefined,
): EngineSessionRef | null {
  const sessionId = status?.session_id?.trim()
  if (!sessionId) return null
  const model = status?.model?.trim()
  return model ? { sessionId, model } : { sessionId }
}

/** `runState` naming `session`; the same object when it already does. */
export function withEngineSession(
  runState: RunState,
  session: EngineSessionRef | null,
): RunState {
  if (!session) return runState
  const model = session.model ?? runState.model
  if (runState.sessionId === session.sessionId && runState.model === model) {
    return runState
  }
  return {
    ...runState,
    sessionId: session.sessionId,
    ...(model !== undefined && { model }),
  }
}

/** The engine's current session, from its last `status`. */
export function currentEngineSession(): EngineSessionRef | null {
  return engineSessionOf(useEngineStore.getState().status)
}

/** `runState` naming the engine's current session, when it has one. */
export function withCurrentEngineSession(runState: RunState): RunState {
  return withEngineSession(runState, currentEngineSession())
}
