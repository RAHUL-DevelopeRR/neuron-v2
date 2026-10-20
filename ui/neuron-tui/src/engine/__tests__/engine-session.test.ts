import { beforeEach, describe, expect, test } from 'bun:test'

import {
  resolvePermissionKey,
  summarizePermissionInput,
} from '../../components/permission-prompt'
import { useChatStore } from '../../state/chat-store'
import { useEngineStore } from '../../state/engine-store'
import { handleEngineEvent, setEngineTurnActive } from '../engine-session'

import type { PermissionView } from '../protocol'

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
