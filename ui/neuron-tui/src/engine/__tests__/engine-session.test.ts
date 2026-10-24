import { beforeEach, describe, expect, test } from 'bun:test'

import {
  interruptAtPermission,
  resolvePermissionKey,
  resolvePermissionNoteKey,
  summarizePermissionInput,
} from '../../components/permission-prompt'
import {
  setNeuronEngineClientForTesting,
  type NeuronEngineClient,
} from '../neuron-engine-client'
import { useChatStore } from '../../state/chat-store'
import { needsSignIn, useEngineStore } from '../../state/engine-store'
import {
  engineNotice,
  handleEngineEvent,
  inUiWording,
  isRetryNotice,
  setEngineTurnActive,
} from '../engine-session'

import type { PermissionView, SignInCodeEvent, StatusEvent } from '../protocol'

const STATUS = {
  type: 'status' as const,
  model: 'claude-sonnet-4-6',
  permission_mode: 'workspace-write',
  session_id: 'session-1',
  cwd: '/work',
  workspace_trusted: false,
  plan_mode: false,
  context_percent: 4,
  git: { branch: 'main', changed: 2, staged: 1, unstaged: 1, untracked: 0, conflicted: 0 },
}

// The engine's own permission request for the approved-bash scenario.
const REQUEST: PermissionView = {
  tool: 'bash',
  current_mode: 'workspace-write',
  required_mode: 'danger-full-access',
  reason: "tool 'bash' requires approval to escalate: `tee` is not a known read-only command",
  input: JSON.stringify({ command: "printf 'approved via prompt' | tee approved.txt" }),
  session_prefixes: [],
}

const systemLines = () =>
  useChatStore
    .getState()
    .messages.filter((m) => m.variant !== 'user')
    .map((m) => m.content)

beforeEach(() => {
  useEngineStore.getState().reset()
  useChatStore.getState().reset()
  setEngineTurnActive(false)
})

describe('handleEngineEvent', () => {
  test('ready, status, usage and models land in the engine store', () => {
    handleEngineEvent({ type: 'ready', protocol: 1, version: '6.3.0' })
    handleEngineEvent(STATUS)
    handleEngineEvent({
      type: 'usage',
      turn: { input: 11, output: 8, cache_read: 0, cache_write: 0 },
      cumulative: { input: 11, output: 8, cache_read: 0, cache_write: 0 },
      estimated: false,
    })
    const engine = useEngineStore.getState()
    expect(engine.connection).toBe('ready')
    expect(engine.version).toBe('6.3.0')
    expect(engine.status?.git?.branch).toBe('main')
    expect(engine.usage?.cumulative.output).toBe(8)
  })

  test('permission requests queue until answered, and a turn end drops them', () => {
    handleEngineEvent({ type: 'permission_requested', id: 1, request: REQUEST })
    handleEngineEvent({ type: 'permission_requested', id: 2, request: REQUEST })
    handleEngineEvent({ type: 'permission_requested', id: 1, request: REQUEST })
    expect(useEngineStore.getState().permissionQueue.map((r) => r.id)).toEqual([1, 2])
    useEngineStore.getState().resolvePermission(1)
    expect(useEngineStore.getState().permissionQueue.map((r) => r.id)).toEqual([2])
    handleEngineEvent({ type: 'turn_finished' })
    expect(useEngineStore.getState().permissionQueue).toEqual([])
  })

  test('notices become system lines; a question waits for the next submit', () => {
    handleEngineEvent({ type: 'notice', message: 'Compacted 12 messages.' })
    handleEngineEvent({ type: 'notice', message: 'Question: Which branch?' })
    expect(systemLines()).toEqual(['Compacted 12 messages.', 'Question: Which branch?'])
    expect(useEngineStore.getState().pendingQuestion).toBe('Question: Which branch?')
    handleEngineEvent({ type: 'turn_finished' })
    expect(useEngineStore.getState().pendingQuestion).toBeNull()
  })

  test('an error outside a turn is shown; inside a turn the turn shows it', () => {
    handleEngineEvent({ type: 'ready', protocol: 1, version: 'x' })
    handleEngineEvent({ type: 'error', message: 'unknown model opus-9' })
    setEngineTurnActive(true)
    handleEngineEvent({ type: 'error', message: 'rate limited' })
    expect(systemLines()).toEqual(['Error: unknown model opus-9'])
  })
})

describe('interrupt notices', () => {
  // The engine's own lines for an interrupted turn (repl_loop.rs), without the
  // line REPL's "press Ctrl-C again" hints.
  const INTERRUPTING = '■ Interrupting…'

  test('"Interrupting…" shows only while the turn stops', () => {
    handleEngineEvent({ type: 'notice', message: 'Compacted 12 messages.' })
    handleEngineEvent({ type: 'notice', message: INTERRUPTING })
    expect(systemLines()).toEqual(['Compacted 12 messages.', '■ Interrupting…'])

    handleEngineEvent({ type: 'notice', message: 'Interrupted.' })
    // The turn's message already says "[response interrupted]".
    expect(systemLines()).toEqual(['Compacted 12 messages.', '■ Interrupting…'])

    handleEngineEvent({ type: 'turn_finished' })
    expect(systemLines()).toEqual(['Compacted 12 messages.'])
  })

  test('the tool calls an interrupt skipped are still reported', () => {
    handleEngineEvent({ type: 'notice', message: INTERRUPTING })
    handleEngineEvent({
      type: 'notice',
      message: 'Interrupted. 2 pending tool call(s) were not run.',
    })
    handleEngineEvent({ type: 'turn_finished' })
    expect(systemLines()).toEqual(['Interrupted. 2 pending tool call(s) were not run.'])
  })

  test('the classification itself', () => {
    expect(engineNotice(INTERRUPTING)).toEqual({
      kind: 'show',
      text: '■ Interrupting…',
      untilTurnEnd: true,
    })
    expect(engineNotice('Interrupted.')).toEqual({ kind: 'drop' })
    expect(engineNotice('Interrupted. 1 pending tool call(s) were not run.')).toEqual({
      kind: 'show',
      text: 'Interrupted. 1 pending tool call(s) were not run.',
      untilTurnEnd: false,
    })
    expect(engineNotice('Quitting: saving the session first…')).toEqual({
      kind: 'show',
      text: 'Quitting: saving the session first…',
      untilTurnEnd: false,
    })
  })

  test('notices are shown as the engine words them, Ctrl-C included', () => {
    // The UI no longer rewrites the engine's Ctrl-C wording.
    expect(engineNotice('Press Ctrl-C again to quit.')).toEqual({
      kind: 'show',
      text: 'Press Ctrl-C again to quit.',
      untilTurnEnd: false,
    })
  })
})

describe('retry notices', () => {
  // The engine's own lines (api/src/retry.rs, rusty-claude-cli/src/api_client.rs).
  const RETRY = 'Zero-X upstream_timeout; retrying in 1.3s (retry 1 of 2)'
  const STALL_RETRY = '⚠ The model did not start responding within 90s — retrying once…'
  const FINAL_ERROR =
    'api failed after 3 attempts: The model did not respond in time. Retry your request. (upstream_timeout)'

  beforeEach(() => {
    handleEngineEvent({ type: 'ready', protocol: 1, version: 'x' })
  })

  test('are recognised by their wording', () => {
    expect(isRetryNotice(RETRY)).toBe(true)
    expect(isRetryNotice('Zero-X capacity_exhausted; retrying now (retry 2 of 2)')).toBe(true)
    expect(isRetryNotice(STALL_RETRY)).toBe(true)
    expect(isRetryNotice('Compacted 12 messages.')).toBe(false)
    expect(isRetryNotice('Retry your request.')).toBe(false)
  })

  test('show while the turn retries, and stay when it then succeeds', () => {
    setEngineTurnActive(true)
    handleEngineEvent({ type: 'notice', message: RETRY })
    expect(systemLines()).toEqual([RETRY])
    handleEngineEvent({ type: 'turn_finished' })
    setEngineTurnActive(false)
    expect(systemLines()).toEqual([RETRY])
  })

  test('a turn that fails for good takes its retries back; its error says how many', () => {
    setEngineTurnActive(true)
    handleEngineEvent({ type: 'notice', message: 'Compacted 12 messages.' })
    handleEngineEvent({ type: 'notice', message: RETRY })
    handleEngineEvent({ type: 'notice', message: STALL_RETRY })
    expect(systemLines()).toEqual(['Compacted 12 messages.', RETRY, STALL_RETRY])

    // The turn's message shows the error; nothing "retrying" is drawn after it.
    handleEngineEvent({ type: 'error', message: FINAL_ERROR })
    expect(systemLines()).toEqual(['Compacted 12 messages.'])
    // Nor a retry notice that comes after the error.
    handleEngineEvent({ type: 'notice', message: RETRY })
    handleEngineEvent({ type: 'turn_finished' })
    setEngineTurnActive(false)
    expect(systemLines()).toEqual(['Compacted 12 messages.'])

    // The next turn's retries show again.
    setEngineTurnActive(true)
    handleEngineEvent({ type: 'notice', message: RETRY })
    expect(systemLines()).toEqual(['Compacted 12 messages.', RETRY])
  })

  test('a retry outside a turn (a slash command) is shown before its error', () => {
    handleEngineEvent({ type: 'notice', message: RETRY })
    handleEngineEvent({ type: 'error', message: FINAL_ERROR })
    expect(systemLines()).toEqual([RETRY, `Error: ${FINAL_ERROR}`])
  })

  test("go above the turn's answer, where they are saved too", () => {
    useChatStore.getState().setMessages(() => [
      { id: 'user-1', variant: 'user', content: 'what is 17 times 23', timestamp: 'now' },
      { id: 'ai-1', variant: 'ai', content: '', blocks: [], timestamp: 'now' },
    ])
    setEngineTurnActive(true, 'ai-1')
    handleEngineEvent({ type: 'notice', message: STALL_RETRY })
    handleEngineEvent({ type: 'notice', message: RETRY })
    // Other notices of the turn still come after it.
    handleEngineEvent({ type: 'notice', message: 'Compacted 12 messages.' })
    expect(useChatStore.getState().messages.map((m) => m.content || m.id)).toEqual([
      'what is 17 times 23',
      STALL_RETRY,
      RETRY,
      'ai-1',
      'Compacted 12 messages.',
    ])
  })
})

describe('questions', () => {
  const QUESTION = 'Question: Which branch?\n  1. main\n  2. dev\n  Answer with 1-2 or in your own words.'

  test("a turn's question is drawn in the turn's message, not as a chat line", () => {
    setEngineTurnActive(true, 'ai-1')
    handleEngineEvent({ type: 'notice', message: QUESTION })
    expect(systemLines()).toEqual([])
    expect(useEngineStore.getState().pendingQuestion).toBe(QUESTION)
  })

  test('outside a turn it is a chat line, as before', () => {
    handleEngineEvent({ type: 'notice', message: QUESTION })
    expect(systemLines()).toEqual([QUESTION])
  })
})

describe('sign-in wording', () => {
  test('the signed-out reason is left to the sign-in panel, which says /login', () => {
    handleEngineEvent({
      type: 'notice',
      message:
        'Sign in with `neuron auth login` (or `neuron auth login --with-token` for CI), or set NEURON_TOKEN',
    })
    expect(systemLines()).toEqual([])
  })

  test('messages of a running engine say /login, not the shell command', () => {
    expect(inUiWording('Your Zero-X session expired or was revoked — run neuron auth login')).toBe(
      'Your Zero-X session expired or was revoked — type /login',
    )
    expect(
      inUiWording(
        'Not signed in to Zero-X. Run `neuron auth login` (or set NEURON_TOKEN) to see usage.',
      ),
    ).toBe('Not signed in to Zero-X. Type /login to see usage.')
    // Where the token login is the only way, the message keeps it.
    const tokenOnly =
      'This Zero-X gateway does not support device sign-in yet. Use `neuron auth login --with-token`.'
    expect(inUiWording(tokenOnly)).toBe(tokenOnly)
    expect(inUiWording('unknown model opus-9')).toBe('unknown model opus-9')

    handleEngineEvent({ type: 'ready', protocol: 1, version: 'x' })
    handleEngineEvent({
      type: 'error',
      message: 'Your Zero-X session expired or was revoked — run neuron auth login',
    })
    expect(systemLines()).toEqual([
      'Error: Your Zero-X session expired or was revoked — type /login',
    ])
  })
})

describe('sign-in', () => {
  // The engine's signed-out stage: no session until /login.
  const SIGNED_OUT: StatusEvent = {
    ...STATUS,
    session_id: '',
    git: null,
    signed_in: false,
    account: null,
  }
  const CODE: SignInCodeEvent = {
    type: 'sign_in_code',
    url: 'https://zero-x.live/device',
    url_complete: 'https://zero-x.live/device?code=BCDF-GHJK',
    user_code: 'BCDF-GHJK',
    expires_in_secs: 600,
    browser_opened: true,
  }

  test('sign_in_required decides when the engine sends it', () => {
    expect(needsSignIn({ ...STATUS, signed_in: false, sign_in_required: true })).toBe(true)
    // A Zero-X session after /logout still has its session id.
    expect(needsSignIn({ ...STATUS, session_id: 'session-1', sign_in_required: true })).toBe(true)
    // Even the old signed-out shape follows the explicit field.
    expect(needsSignIn({ ...SIGNED_OUT, sign_in_required: false })).toBe(false)
  })

  test('session_starting marks the sign-in approved', () => {
    handleEngineEvent(CODE)
    handleEngineEvent({ type: 'session_starting' })
    expect(useEngineStore.getState().signIn?.approved).toBe(true)
    handleEngineEvent({ type: 'turn_finished' })
    expect(useEngineStore.getState().signIn).toBeNull()
  })

  test('only the signed-out stage needs a sign-in', () => {
    expect(needsSignIn(SIGNED_OUT)).toBe(true)
    // An API-key provider is not signed in to Zero-X, but has a session.
    expect(needsSignIn({ ...SIGNED_OUT, session_id: 'session-1' })).toBe(false)
    // Engines before sign-in support sent neither field.
    expect(needsSignIn(STATUS)).toBe(false)
    expect(needsSignIn(null)).toBe(false)
  })

  test('a device code shows until its sign-in is approved and the session is up', () => {
    handleEngineEvent({ type: 'ready', protocol: 1, version: 'x' })
    handleEngineEvent(SIGNED_OUT)
    handleEngineEvent(CODE)
    expect(useEngineStore.getState().signIn).toMatchObject({
      user_code: 'BCDF-GHJK',
      approved: false,
    })
    handleEngineEvent({ type: 'notice', message: 'Signed in to Zero-X (plan pro).' })
    expect(useEngineStore.getState().signIn?.approved).toBe(true)
    handleEngineEvent({ type: 'turn_finished' })
    expect(useEngineStore.getState().signIn).toBeNull()
  })

  test('a signed-in status ends the sign-in too; a cancelled one ends at turn_finished', () => {
    handleEngineEvent(CODE)
    handleEngineEvent({
      ...STATUS,
      signed_in: true,
      account: { email: null, plan: 'pro', source: 'stored login', expires_at: null },
    })
    expect(useEngineStore.getState().signIn).toBeNull()

    handleEngineEvent(CODE)
    handleEngineEvent({ type: 'error', message: 'Sign-in cancelled.' })
    handleEngineEvent({ type: 'turn_finished' })
    expect(useEngineStore.getState().signIn).toBeNull()
  })
})

describe('permission prompt keys', () => {
  const armed = { armed: true, canAllowForSession: true }

  test('y, a, n and e answer; other keys do nothing', () => {
    expect(resolvePermissionKey({ name: 'y', ctrl: false, meta: false }, armed)).toEqual({
      type: 'decide',
      decision: { kind: 'allow_once' },
    })
    expect(resolvePermissionKey({ name: 'a', ctrl: false, meta: false }, armed)).toEqual({
      type: 'decide',
      decision: { kind: 'allow_for_session' },
    })
    expect(resolvePermissionKey({ name: 'n', ctrl: false, meta: false }, armed)).toEqual({
      type: 'decide',
      decision: { kind: 'deny', note: null },
    })
    expect(resolvePermissionKey({ name: 'e', ctrl: false, meta: false }, armed)).toEqual({
      type: 'write-note',
    })
    expect(resolvePermissionKey({ name: 'x', ctrl: false, meta: false }, armed).type).toBe('none')
    expect(resolvePermissionKey({ name: 'y', ctrl: true, meta: false }, armed).type).toBe('none')
  })

  test('Esc denies; Ctrl-C interrupts the turn, even before the guard passes', () => {
    expect(resolvePermissionKey({ name: 'escape', ctrl: false, meta: false }, armed)).toEqual({
      type: 'decide',
      decision: { kind: 'deny', note: null },
    })
    const unarmed = { armed: false, canAllowForSession: true }
    expect(resolvePermissionKey({ name: 'escape', ctrl: false, meta: false }, unarmed).type).toBe(
      'none',
    )
    expect(resolvePermissionKey({ name: 'c', ctrl: true, meta: false }, armed)).toEqual({
      type: 'interrupt',
    })
    expect(resolvePermissionKey({ name: 'c', ctrl: true, meta: false }, unarmed)).toEqual({
      type: 'interrupt',
    })
    // While the deny note is written: Esc goes back, Ctrl-C still stops.
    expect(resolvePermissionNoteKey({ name: 'escape', ctrl: false, meta: false })).toBe('back')
    expect(resolvePermissionNoteKey({ name: 'c', ctrl: true, meta: false })).toBe('interrupt')
    expect(resolvePermissionNoteKey({ name: 'c', ctrl: false, meta: false })).toBe('none')
  })

  test('Ctrl-C stops the turn and closes the prompt', () => {
    const sent: string[] = []
    setNeuronEngineClientForTesting({
      connection: 'ready',
      interrupt: () => sent.push('interrupt'),
      replyToPermission: (reply: { decision: { kind: string } }) =>
        sent.push(`reply:${reply.decision.kind}`),
    } as unknown as NeuronEngineClient)
    try {
      handleEngineEvent({ type: 'permission_requested', id: 4, request: REQUEST })
      // No UI run is registered here, so the engine is interrupted directly.
      interruptAtPermission(4)
      expect(sent).toEqual(['interrupt', 'reply:deny'])
      expect(useEngineStore.getState().permissionQueue).toEqual([])
    } finally {
      setNeuronEngineClientForTesting(null)
    }
  })

  test('keys are ignored until the guard passes, and a only when offered', () => {
    expect(
      resolvePermissionKey({ name: 'y', ctrl: false, meta: false }, { armed: false, canAllowForSession: true }).type,
    ).toBe('none')
    expect(
      resolvePermissionKey({ name: 'a', ctrl: false, meta: false }, { armed: true, canAllowForSession: false }).type,
    ).toBe('none')
  })

  test('the request shows its command, path or raw input', () => {
    expect(summarizePermissionInput(REQUEST)).toBe("printf 'approved via prompt' | tee approved.txt")
    expect(
      summarizePermissionInput({ ...REQUEST, input: JSON.stringify({ path: '\\\\?\\C:\\a.txt' }) }),
    ).toBe('C:\\a.txt')
    expect(summarizePermissionInput({ ...REQUEST, input: 'raw' })).toBe('raw')
  })
})
