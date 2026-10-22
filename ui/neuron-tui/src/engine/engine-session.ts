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
  // A new turn has neither retried nor failed yet.
  if (active) forgetTurnRetries()
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

// The engine's interrupt notices. In the UI a Ctrl-C during a turn interrupts
// it, and the turn's message says "[response interrupted]" at once. So:
//   "■ Interrupting…" shows only while the engine is still stopping the turn:
//     `turn_finished` removes it;
//   "Interrupted." is dropped: the message already says so;
//   "Interrupted. N pending tool call(s) were not run." shows what was not run.
const INTERRUPTING = /^■ Interrupting/

export type EngineNotice =
  | { kind: 'drop' }
  | { kind: 'show'; text: string; untilTurnEnd: boolean }

/** How the UI shows an engine notice (see the interrupt notes above). */
export function engineNotice(message: string): EngineNotice {
  if (SIGNED_OUT_REASON.test(message)) return { kind: 'drop' }
  const text = inUiWording(message).trim()
  if (!text || text === 'Interrupted.') return { kind: 'drop' }
  return { kind: 'show', text, untilTurnEnd: INTERRUPTING.test(text) }
}

// The engine's retry notices: its API client's ("Zero-X upstream_timeout;
// retrying in 1.3s (retry 1 of 2)", api/src/retry.rs) and the retry of a
// stalled stream ("⚠ The model did not start responding within 90s —
// retrying once…", rusty-claude-cli/src/api_client.rs).
const RETRY_NOTICE = /\bretrying (?:in \d|now\b|once\b)/i

/** A notice that the engine is retrying the turn's request. */
export function isRetryNotice(text: string): boolean {
  return RETRY_NOTICE.test(text)
}

let noticeCount = 0

/** Shows a notice under an id of its own, so it can be taken back. */
function showTrackedNotice(text: string, ids: Set<string>): void {
  noticeCount += 1
  const id = `sys-turn-notice-${Date.now()}-${noticeCount}`
  ids.add(id)
  showSystemMessage(text, id)
}

function removeNotices(ids: Set<string>): void {
  if (ids.size === 0) return
  const stale = new Set(ids)
  ids.clear()
  useChatStore
    .getState()
    .setMessages((prev) => prev.filter((message) => !stale.has(message.id)))
}

/** Ids of notices that stop being true when the turn ends ("■ Interrupting…"). */
const turnScopedNotices = new Set<string>()

/**
 * The running turn's retry notices. The turn's message was drawn before any
 * of them, so a turn that fails for good would show its final error above
 * "retrying in 1.3s": once it fails they are taken back (its error says how
 * many attempts were made), and later ones are not shown.
 */
const turnRetryNotices = new Set<string>()
let turnFailed = false

function forgetTurnRetries(): void {
  turnRetryNotices.clear()
  turnFailed = false
}

function showNotice(notice: EngineNotice): void {
  if (notice.kind === 'drop') return
  if (notice.untilTurnEnd) {
    showTrackedNotice(notice.text, turnScopedNotices)
  } else if (turnActive && isRetryNotice(notice.text)) {
    if (!turnFailed) showTrackedNotice(notice.text, turnRetryNotices)
  } else {
    showSystemMessage(notice.text)
  }
}

function dropTurnScopedNotices(): void {
  removeNotices(turnScopedNotices)
}

/** The running turn failed for good: its retries are over. */
function markTurnFailed(): void {
  turnFailed = true
  removeNotices(turnRetryNotices)
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
      if (turnActive) {
        markTurnFailed()
      } else if (engine.connection === 'ready') {
        showSystemMessage(`Error: ${inUiWording(event.message)}`)
      }
      return
    case 'turn_finished':
      // Nothing waits for an answer once the turn is over (a `/login` ends
      // with one too, signed in or not), and "Interrupting…" is over too.
      // A turn that ended well keeps its retry notices.
      engine.clearPermissions()
      engine.setPendingQuestion(null)
      engine.setSignIn(null)
      dropTurnScopedNotices()
      forgetTurnRetries()
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
    // No turn runs in an engine that stopped; one that was running failed.
    dropTurnScopedNotices()
    if (turnActive) markTurnFailed()
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
