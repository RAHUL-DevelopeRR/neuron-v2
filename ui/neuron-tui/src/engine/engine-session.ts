/**
 * The UI's one engine: started for the chosen project folder, restarted when
 * the folder changes or is trusted, shut down on exit. Every event it sends
 * outside a turn's transcript lands here: footer status, models, usage,
 * permission requests, notices (shown as system lines, in the UI's words),
 * errors that belong to no turn, what trusting the folder would turn on, and
 * the reports of `/usage`, `/init`, `/memory` and `/doctor`.
 */

import { useChatStore } from '../state/chat-store'
import { hasEngineFeature, needsSignIn, useEngineStore } from '../state/engine-store'
import { resetLiveTurnStore } from '../state/live-turn-store'
import { useStartGate } from '../state/start-gate'
import { consoleSafeText } from '../utils/glyphs'
import { logger } from '../utils/logger'
import { getSystemMessage } from '../utils/message-history'
import {
  existingCurrentChatDir,
  recordChatEngineSession,
} from '../utils/run-state-storage'
import { afterConsoleCommand } from '../utils/console-guard'
import { declareUiCapabilities } from './capabilities'
import { noteEngineFeatures } from './live-turn'
import {
  getNeuronEngineClient,
  runExchange,
  setNeuronEngineClient,
  type EngineExit,
  type EngineLaunchOptions,
  type NeuronEngineClient,
  type TurnOutcome,
} from './neuron-engine-client'
import { handleGitViewsEvent, resetGitViews } from './git-views'
import { FEATURE_TRUST_PREVIEW } from './protocol'
import { QUESTION_PREFIX } from './questions'
import { engineSessionOf, withEngineSession } from './session-run-state'
import { StdioEngineClient } from './stdio-engine-client'

import type { StatusEvent, UiEvent, WorkspaceTrustedEvent } from './protocol'
import type { StdioEngineClientOptions } from './stdio-engine-client'

export { QUESTION_PREFIX }

let turnActive = false
/** The running turn's AI message, when it has one. */
let turnMessageId: string | null = null

/**
 * While a turn runs, its own error is shown on its message instead, its
 * retry notices go above that message, and its questions into it.
 */
export function setEngineTurnActive(active: boolean, messageId?: string): void {
  turnActive = active
  turnMessageId = active ? (messageId ?? null) : null
  // A new turn has neither retried nor failed yet.
  if (active) forgetTurnRetries()
}

/**
 * A system line at the end of the chat, or right above the message `beforeId`
 * when it is there.
 */
function showSystemMessage(text: string, id?: string, beforeId?: string | null): void {
  // The engine's markers (⚠, ✓...) in characters this terminal's font has.
  const message = { ...getSystemMessage(consoleSafeText(text)), ...(id && { id }) }
  useChatStore.getState().setMessages((prev) => {
    const index = beforeId ? prev.findIndex((entry) => entry.id === beforeId) : -1
    if (index < 0) return [...prev, message]
    return [...prev.slice(0, index), message, ...prev.slice(index)]
  })
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
  const text = consoleSafeText(inUiWording(message).trim())
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
function showTrackedNotice(
  text: string,
  ids: Set<string>,
  beforeId?: string | null,
): void {
  noticeCount += 1
  const id = `sys-turn-notice-${Date.now()}-${noticeCount}`
  ids.add(id)
  showSystemMessage(text, id, beforeId)
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
 * The running turn's retry notices. They go above the turn's message, which
 * is drawn (and saved) before any of them: below it they read as coming after
 * the answer. A turn that fails for good would show "retrying in 1.3s" next
 * to its final error: once it fails they are taken back (its error says how
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
    if (!turnFailed) showTrackedNotice(notice.text, turnRetryNotices, turnMessageId)
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

/**
 * The engine's session goes into the chat's run state as soon as the engine
 * names it, not when a turn completes: a chat whose only turn was interrupted
 * must still resume it on `--continue` (see session-run-state.ts).
 */
export function recordEngineSessionInChat(status: StatusEvent): void {
  const session = engineSessionOf(status)
  if (!session) return
  const chat = useChatStore.getState()
  if (chat.runState) {
    const next = withEngineSession(chat.runState, session)
    if (next !== chat.runState) chat.setRunState(next)
  }
  // Only a chat that is already saved (a prompt was sent) is written: an
  // empty one stays unsaved, and no chat directory is created for it.
  const chatDir = existingCurrentChatDir()
  if (chatDir) recordChatEngineSession(session, chatDir)
}

export function handleEngineEvent(event: UiEvent): void {
  // Changes, diffs, commit proposals and checkpoints (./git-views.ts).
  if (handleGitViewsEvent(event)) return
  const engine = useEngineStore.getState()
  switch (event.type) {
    case 'ready':
      engine.setReady(event.version, event.features ?? [])
      // What this UI draws, for engines that ask (./capabilities.ts).
      declareUiCapabilities(getNeuronEngineClient(), event)
      noteEngineFeatures(event.features ?? [])
      return
    case 'status':
      engine.setStatus(event)
      recordEngineSessionInChat(event)
      useStartGate.getState().noteStatus(event)
      requestTrustPreview(event)
      // A revoked stored login still reads as signed in while the engine
      // waits for a sign-in: that is not the end of one.
      if (event.signed_in && !needsSignIn(event)) engine.setSignIn(null)
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
      if (event.kind === 'refusal') {
        // A turn's refusal ends its message (turn-events.ts); anywhere else
        // it is a chat line.
        if (!(turnActive && turnMessageId)) showSystemMessage(event.message)
        return
      }
      // The trust screen says what trusting did, and the engine restarts at once.
      if (useStartGate.getState().trustStep.kind !== 'asking' && /^Trusted /.test(event.message)) {
        return
      }
      if (event.message.startsWith(QUESTION_PREFIX)) {
        engine.setPendingQuestion(event.message)
        // A turn's question goes into its message (turn-events.ts), where
        // the agent asked it; anywhere else it is a chat line.
        if (!(turnActive && turnMessageId)) {
          showSystemMessage(event.message)
        }
        return
      }
      showNotice(engineNotice(event.message))
      return
    case 'error':
      // `trust_workspace` failed: the trust screen shows why.
      if (useStartGate.getState().trustStep.kind === 'saving') {
        useStartGate.getState().setTrustStep({ kind: 'failed', error: inUiWording(event.message) })
        return
      }
      if (turnActive) {
        markTurnFailed()
      } else if (engine.connection === 'ready') {
        showSystemMessage(`Error: ${inUiWording(event.message)}`)
      }
      return
    case 'tool_result':
      // The tool ran in the user's console: a command's cmd.exe retitled the
      // window ("npm test"), a PowerShell changed its font. Put them back.
      afterConsoleCommand()
      return
    case 'turn_finished':
      // Nothing waits for an answer once the turn is over (a `/login` ends
      // with one too, signed in or not), and "Interrupting…" is over too.
      // A turn that ended well keeps its retry notices. Its hooks ran in the
      // console too (see tool_result).
      afterConsoleCommand()
      engine.clearPermissions()
      engine.setPendingQuestion(null)
      engine.setSignIn(null)
      dropTurnScopedNotices()
      forgetTurnRetries()
      return
    case 'trust_preview':
      engine.setTrustPreview(event)
      return
    case 'workspace_trusted':
      void restartWithTrust(event)
      return
    case 'report':
      // A scrollable panel above the input, closed with Esc (report-panel.tsx).
      engine.setReport(event)
      useChatStore.getState().setInputMode('report')
      return
    default:
      return
  }
}

/** What this UI handles itself: `report` events, not reports as notices. */
/** The engines asked for their trust preview already. */
const trustPreviewAsked = new WeakSet<NeuronEngineClient>()

/** An untrusted folder: ask once per engine what trusting it would turn on. */
function requestTrustPreview(status: StatusEvent): void {
  const engine = useEngineStore.getState()
  if (status.workspace_trusted || !hasEngineFeature(engine, FEATURE_TRUST_PREVIEW)) return
  const client = getNeuronEngineClient()
  if (trustPreviewAsked.has(client)) return
  trustPreviewAsked.add(client)
  try {
    client.send({ type: 'trust_preview' })
  } catch (error) {
    logger.warn({ error }, '[engine] could not ask for the trust preview')
  }
}

/**
 * "Allow Neuron in this folder": the engine records trust for the folder (its
 * git root), or for this session only in the home folder, and answers with
 * `workspace_trusted` ({@link restartWithTrust}) or an `error`.
 */
export function trustWorkspace(): void {
  const gate = useStartGate.getState()
  gate.setTrustStep({ kind: 'saving' })
  try {
    getNeuronEngineClient().send({ type: 'trust_workspace' })
  } catch (error) {
    gate.setTrustStep({
      kind: 'failed',
      error: error instanceof Error ? error.message : String(error),
    })
  }
}

/** The chat has a prompt in it: a restart must keep its conversation. */
function chatHasPrompts(): boolean {
  return useChatStore.getState().messages.some((message) => message.variant === 'user')
}

/**
 * The folder's settings, hooks and MCP servers load when a session starts, so
 * the engine restarts with them, on the same conversation when there is one to
 * keep (a `--continue`, or prompts sent already). A folder trusted for this
 * session only (the home folder) is trusted by the restarted engine through
 * `NEURON_TRUST_WORKSPACE`, and asked about again at the next launch.
 */
async function restartWithTrust(event: WorkspaceTrustedEvent): Promise<void> {
  const gate = useStartGate.getState()
  if (!event.restart_required || !lastLaunch) {
    gate.setTrustStep({ kind: 'asking' })
    return
  }
  gate.setTrustStep({ kind: 'restarting' })
  const launch = lastLaunch
  const sessionId = useEngineStore.getState().status?.session_id || undefined
  const resume =
    launch.resume !== undefined || chatHasPrompts() ? (sessionId ?? launch.resume) : undefined
  const env = event.session_only
    ? { ...launch.env, NEURON_TRUST_WORKSPACE: '1' }
    : launch.env
  await startEngineSession({ ...launch, resume, env })
  useStartGate.getState().setTrustStep({ kind: 'asking' })
}

/**
 * `/login` on `engine`: the engine starts a device sign-in (a `sign_in_code`
 * event, then `session_starting` once approved). A sign-in that could not
 * start because Zero-X was unreachable sets the sign-in panel's problem.
 */
export function runEngineLogin(engine: NeuronEngineClient = getNeuronEngineClient()): Promise<TurnOutcome> {
  useEngineStore.getState().setSignInProblem(null)
  return runExchange(engine, () => engine.slash('login', ''), () => {}).then((outcome) => {
    if (outcome.error && isUnreachableSignIn(outcome.error)) {
      useEngineStore.getState().setSignInProblem(outcome.error)
    }
    return outcome
  })
}

let pendingStart: Promise<void> | null = null
/** The latest launch, with its flags and folder, for a restart that resumes a chat. */
let lastLaunch: EngineLaunchOptions | null = null
/** Test seam: how each new engine client spawns (a fake process). */
let clientOptionsForTesting: StdioEngineClientOptions = {}

export function setEngineClientOptionsForTesting(options: StdioEngineClientOptions): void {
  clientOptionsForTesting = options
  lastLaunch = null
}

/**
 * Starts (or restarts) the engine in `launch.cwd`. Never throws: a failed
 * start is recorded in the engine store and shown in the chat.
 */
export function startEngineSession(launch: EngineLaunchOptions): Promise<void> {
  lastLaunch = launch
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
    ...clientOptionsForTesting,
  })
  setNeuronEngineClient(client)
  const engine = useEngineStore.getState()
  engine.reset()
  resetGitViews()
  // No turn, unsettled tokens or task list of the last engine.
  resetLiveTurnStore()
  engine.setConnection('starting', null)

  client.subscribe(handleEngineEvent)
  client.onExit((exit) => {
    if (getNeuronEngineClient() !== client) return
    handleEngineExit(exit, {
      lastError: client.lastError,
      stopRequested: client.stopRequested,
    })
  })

  try {
    await client.start(launch)
  } catch (error) {
    if (getNeuronEngineClient() !== client) return
    const message = error instanceof Error ? error.message : String(error)
    useEngineStore.getState().setConnection('exited', message, true)
    showSystemMessage(message)
  }
}

/** The exit code of an engine that quit on its own after two interrupts. */
const FORCED_QUIT_EXIT_CODE = 130

let engineCrashed = false

/**
 * An engine stopped during this UI session without being asked to, after it
 * was ready. The UI then exits non-zero (1), also when it was quit normally.
 */
export function engineCrashedThisSession(): boolean {
  return engineCrashed
}

/** Test seam: forget an earlier crash. */
export function resetEngineCrashForTesting(): void {
  engineCrashed = false
}

/**
 * The engine process ended. A stop the UI did not ask for, after the engine
 * was ready, is said once: on the turn that was running (its message says
 * "exited during the turn"), else as a chat line; the status bar then only
 * says the engine is not running. A failed start is reported by its caller.
 */
export function handleEngineExit(
  exit: EngineExit,
  options: { lastError: string | null; stopRequested: boolean },
): void {
  // No turn runs in an engine that stopped; one that was running failed, and
  // its message says so.
  const turnShowsIt = turnActive && turnMessageId !== null
  dropTurnScopedNotices()
  if (turnActive) markTurnFailed()
  const engine = useEngineStore.getState()
  const wasReady = engine.connection === 'ready'
  const reason =
    options.lastError ?? `The Neuron engine stopped (exit code ${exit.code}).`
  const unexpected =
    wasReady && !options.stopRequested && (exit.code !== 0 || exit.signal !== null)
  if (unexpected && exit.code !== FORCED_QUIT_EXIT_CODE) engineCrashed = true
  const shownInChat = unexpected && !turnShowsIt
  engine.setConnection('exited', reason, unexpected)
  if (shownInChat) showSystemMessage(reason)
}

/**
 * A `/login` that failed because Zero-X could not be used for a sign-in at
 * all, as the engine words it (device_login.rs): unreachable, no device
 * sign-in on that gateway (404), or a server error. Denied, expired and
 * cancelled sign-ins are not this: those reached Zero-X.
 */
export function isUnreachableSignIn(message: string): boolean {
  return /could not reach zero-x|does not support device sign-in|answered the sign-in request with an error \(http 5/i.test(
    message,
  )
}

/**
 * A new chat starts the engine on a fresh conversation as well: `/new`,
 * `/clear`, `/reset` and the history screen's New chat. The engine refuses a
 * bare `clear` (it asks for `--confirm`), so the next prompt used to carry the
 * old conversation along. The old session stays saved and can be resumed.
 *
 * The clear joins the engine's exchange queue at once (or first thing once a
 * start in progress is done), so a prompt sent right after it, such as
 * `/new <prompt>`, runs in the fresh conversation.
 */
export function clearEngineConversation(): Promise<void> {
  const clear = (): Promise<void> => {
    const engine = getNeuronEngineClient()
    if (engine.connection !== 'ready') return Promise.resolve()
    return runExchange(engine, () => engine.slash('clear', '--confirm'), () => {}).then(
      () => undefined,
      () => undefined,
    )
  }
  return pendingStart ? pendingStart.then(clear) : clear()
}

/**
 * A chat picked in /history: the engine resumes the session saved with it
 * (`--resume`), so the model has the conversation the transcript shows. Before,
 * only the transcript came back, and the model went on with the current
 * conversation. A chat saved without a session starts a fresh one instead.
 */
export function resumeEngineSessionForChat(sessionId: string | undefined): Promise<void> {
  if (!sessionId || !lastLaunch) return clearEngineConversation()
  return startEngineSession({ ...lastLaunch, resume: sessionId })
}

export async function stopEngineSession(): Promise<void> {
  await getNeuronEngineClient()
    .shutdown()
    .catch(() => undefined)
}
