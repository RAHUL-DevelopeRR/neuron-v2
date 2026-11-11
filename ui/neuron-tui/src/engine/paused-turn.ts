/**
 * A paused turn: it reached its model-request cap, or kept repeating itself,
 * and the engine ended it calmly instead of failing it (a `notice` of kind
 * `paused`, with the `turn_pause` capability). The work so far is kept in the
 * session; the next prompt carries on from there.
 *
 * Until the user sends something else, Enter on an empty prompt sends
 * {@link CONTINUE_PROMPT}: one key to keep going, as the notice says.
 */

/** What Enter on an empty prompt sends after a pause. */
export const CONTINUE_PROMPT = 'continue'

let pending = false

/** The engine paused the turn that just ended. */
export function notePausedTurn(): void {
  pending = true
}

/** Something else was sent (a prompt, a command): the pause is answered. */
export function clearPausedTurn(): void {
  pending = false
}

/** A paused turn waits for the user to continue it. */
export function hasPausedTurn(): boolean {
  return pending
}

/**
 * Enter on an empty prompt: the prompt that continues a paused turn, once,
 * or `null` when no turn is paused.
 */
export function takeContinuePrompt(): string | null {
  if (!pending) return null
  pending = false
  return CONTINUE_PROMPT
}
