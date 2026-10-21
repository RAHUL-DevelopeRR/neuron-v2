/**
 * The UI's one engine: started for the chosen project folder, restarted when
 * the folder changes, shut down on exit. Every event it sends outside a turn's
 * transcript lands here: footer status, models, usage, permission requests,
 * notices (shown as system lines, in the UI's words) and errors that belong
 * to no turn.
 */

import { useChatStore } from '../state/chat-store'
import { useEngineStore } from '../state/engine-store'
import { logger } from '../utils/logger'
import { getSystemMessage } from '../utils/message-history'
import {
  getNeuronEngineClient,
  setNeuronEngineClient,
  type EngineLaunchOptions,
} from './neuron-engine-client'
import { StdioEngineClient } from './stdio-engine-client'

import type { UiEvent } from './protocol'

/** AskUserQuestion reaches the UI as a notice with this prefix. */
export const QUESTION_PREFIX = 'Question:'

let turnActive = false

/** While a turn runs, its own error is shown on its message instead. */
export function setEngineTurnActive(active: boolean): void {
  turnActive = active
}

function showSystemMessage(text: string, id?: string): void {
  const message = { ...getSystemMessage(text), ...(id && { id }) }
  useChatStore.getState().setMessages((prev) => [...prev, message])
}

// ------------------------------------------------------------ engine wording

// The engine words sign-in for a shell: `neuron auth login` (`--with-token`
// for CI) or NEURON_TOKEN. In the UI the way to sign in is /login, which the
// sign-in panel says; messages from a running engine say the same.
// `neuron auth login --with-token` alone is kept: there it is the only way.
const SHELL_SIGN_IN =
  /(?:\b(run|use)\s+)?`?neuron auth login`?(?! --with-token)(?: \(or `neuron auth login --with-token` for CI\))?(?:,? or set NEURON_TOKEN| \(or set NEURON_TOKEN\))?/gi

/** The signed-out reason, which the sign-in panel already answers. */
const SIGNED_OUT_REASON = /^Sign in with `neuron auth login`/

/**
 * A running engine's message in the UI's words: sign in with `/login`, not
 * the shell command. Not for launch failures: with no engine running, the
 * shell is the only place to sign in.
 */
export function inUiWording(message: string): string {
  return message.replace(SHELL_SIGN_IN, (_match, verb: string | undefined) =>
    verb ? `${verb[0] === verb[0]!.toUpperCase() ? 'Type' : 'type'} /login` : '/login',
  )
}

// The engine's interrupt notices come from its line REPL, where a second
// Ctrl-C quits. In the UI a Ctrl-C during a turn interrupts it, and the turn's
// message says "[response interrupted]" at once; at the prompt one Ctrl-C arms
// "Press Ctrl-C again to exit" in the status bar and a second exits. So:
//   "■ Interrupting… (press Ctrl-C again to quit)" shows without the hint,
//     only while the engine is still stopping the turn: `turn_finished`
//     removes it;
//   "Interrupted. Back at the prompt; press Ctrl-C again to exit." is
//     dropped: the message already says so, and its hint is wrong here;
//   "Interrupted. N pending tool call(s) were not run. Press Ctrl-C again to
//     exit." keeps what was not run, without the hint.
const INTERRUPTING = /^■ Interrupting/
const CTRL_C_HINT =
  /\s*(?:\(press Ctrl-C again to (?:quit|exit)\)|(?:Back at the prompt; )?press Ctrl-C again to (?:quit|exit)\.?)/gi

export type EngineNotice =
  | { kind: 'drop' }
  | { kind: 'show'; text: string; untilTurnEnd: boolean }

/** How the UI shows an engine notice (see the interrupt notes above). */
export function engineNotice(message: string): EngineNotice {
  if (SIGNED_OUT_REASON.test(message)) return { kind: 'drop' }
  const text = inUiWording(message).replace(CTRL_C_HINT, '').trim()
  if (!text || text === 'Interrupted.') return { kind: 'drop' }
  return { kind: 'show', text, untilTurnEnd: INTERRUPTING.test(text) }
}

/** Ids of notices that stop being true when the turn ends ("■ Interrupting…"). */
const turnScopedNotices = new Set<string>()
let turnScopedCount = 0

function showTurnScopedNotice(text: string): void {
  turnScopedCount += 1
  const id = `sys-turn-notice-${Date.now()}-${turnScopedCount}`
  turnScopedNotices.add(id)
  showSystemMessage(text, id)
}

function showNotice(notice: EngineNotice): void {
  if (notice.kind === 'drop') return
  if (notice.untilTurnEnd) showTurnScopedNotice(notice.text)
  else showSystemMessage(notice.text)
}

function dropTurnScopedNotices(): void {
  if (turnScopedNotices.size === 0) return
  const stale = new Set(turnScopedNotices)
  turnScopedNotices.clear()
  useChatStore
    .getState()
    .setMessages((prev) => prev.filter((message) => !stale.has(message.id)))
}

export function handleEngineEvent(event: UiEvent): void {
  const engine = useEngineStore.getState()
  switch (event.type) {
    case 'ready':
      engine.setReady(event.version)
      return
    case 'status':
      engine.setStatus(event)
      if (event.signed_in) engine.setSignIn(null)
      return
    case 'sign_in_code':
      engine.setSignIn(event)
      return
    case 'session_starting':
      // The sign-in worked; the panel says the session is starting.
      engine.markSignInApproved()
      return
    case 'models':
      engine.setModels(event)
      return
    case 'usage':
      engine.setUsage(event)
      return
    case 'permission_requested':
      engine.enqueuePermission(event)
      return
    case 'notice':
      // During a sign-in the only notice is its success ("Signed in to
      // Zero-X ..."), so engines without `session_starting` are read from it.
      if (engine.signIn && !engine.signIn.approved) engine.markSignInApproved()
      if (event.message.startsWith(QUESTION_PREFIX)) {
        engine.setPendingQuestion(event.message)
        showSystemMessage(event.message)
        return
      }
      showNotice(engineNotice(event.message))
      return
    case 'error':
      if (!turnActive && engine.connection === 'ready') {
        showSystemMessage(`Error: ${inUiWording(event.message)}`)
      }
      return
    case 'turn_finished':
      // Nothing waits for an answer once the turn is over (a `/login` ends
      // with one too, signed in or not), and "Interrupting…" is over too.
      engine.clearPermissions()
      engine.setPendingQuestion(null)
      engine.setSignIn(null)
      dropTurnScopedNotices()
      return
    default:
      return
  }
}

let pendingStart: Promise<void> | null = null

/**
 * Starts (or restarts) the engine in `launch.cwd`. Never throws: a failed
 * start is recorded in the engine store and shown in the chat.
 */
export function startEngineSession(launch: EngineLaunchOptions): Promise<void> {
  const start = launchEngineSession(launch)
  pendingStart = start
  void start.finally(() => {
    if (pendingStart === start) pendingStart = null
  })
  return start
}

/**
 * Resolves once a start in progress has finished, ready or failed, so a
 * prompt sent while the engine boots (`neuron "fix it"`) waits for it.
 */
export function whenEngineStarted(): Promise<void> {
  return pendingStart ?? Promise.resolve()
}

async function launchEngineSession(launch: EngineLaunchOptions): Promise<void> {
  const previous = getNeuronEngineClient()
  if (previous.connection === 'ready' || previous.connection === 'starting') {
    await previous.shutdown().catch(() => undefined)
  }

  const client = new StdioEngineClient({
    onStderr: (line) => logger.debug({ line }, '[engine] stderr'),
    onUnknownLine: (line, reason) =>
      logger.warn({ reason, line: line.slice(0, 500) }, '[engine] unexpected stdout line'),
  })
  setNeuronEngineClient(client)
  const engine = useEngineStore.getState()
  engine.reset()
  engine.setConnection('starting', null)

  client.subscribe(handleEngineEvent)
  client.onExit((exit) => {
    if (getNeuronEngineClient() !== client) return
    // No turn runs in an engine that stopped.
    dropTurnScopedNotices()
    const wasReady = useEngineStore.getState().connection === 'ready'
    const reason =
      client.lastError ?? `The Neuron engine stopped (exit code ${exit.code}).`
    useEngineStore.getState().setConnection('exited', reason)
    // A failed start is reported by the catch below; this is a later stop.
    if (wasReady && exit.code !== 0) showSystemMessage(reason)
  })

  try {
    await client.start(launch)
  } catch (error) {
    if (getNeuronEngineClient() !== client) return
    const message = error instanceof Error ? error.message : String(error)
    useEngineStore.getState().setConnection('exited', message)
    showSystemMessage(message)
  }
}

export async function stopEngineSession(): Promise<void> {
  await getNeuronEngineClient()
    .shutdown()
    .catch(() => undefined)
}
