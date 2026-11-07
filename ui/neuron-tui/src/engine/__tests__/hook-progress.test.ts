import { describe, expect, test } from 'bun:test'

import { INITIAL_LIVE_TURN } from '../../state/live-turn-store'
import { formatHookDuration, hookFinishedLine } from '../hook-progress'
import { describeLiveActivity, reduceLiveTurn } from '../live-turn'
import { UI_EVENT_TYPES } from '../protocol'

import type { HookFinishedEvent, HookStartedEvent } from '../protocol'

const started: HookStartedEvent = {
  type: 'hook_started',
  event: 'PreToolUse',
  matcher: 'bash',
  command: 'guard.sh --token ***',
  tool_use_id: 'call-1',
}

const finished = (patch: Partial<HookFinishedEvent> = {}): HookFinishedEvent => ({
  type: 'hook_finished',
  event: 'PreToolUse',
  matcher: 'bash',
  command: 'guard.sh --token ***',
  tool_use_id: 'call-1',
  outcome: 'ok',
  duration_ms: 12,
  ...patch,
})

describe('hook progress', () => {
  test('the protocol knows both hook events', () => {
    expect(UI_EVENT_TYPES).toContain('hook_started')
    expect(UI_EVENT_TYPES).toContain('hook_finished')
  })

  test('the status line names the running hook until it finishes', () => {
    const running = { ...INITIAL_LIVE_TURN, active: true, model: 'idle' as const }
    const during = reduceLiveTurn(running, started, 0)
    expect(during.hook).toEqual({
      event: 'PreToolUse',
      command: 'guard.sh --token ***',
      toolUseId: 'call-1',
    })
    expect(describeLiveActivity(during, { now: 0, awaitingApproval: false })).toEqual({
      verb: 'Running PreToolUse hook',
      target: 'guard.sh --token ***',
      detail: null,
      stalled: false,
    })
    const after = reduceLiveTurn(during, finished(), 5)
    expect(after.hook).toBeNull()
    expect(describeLiveActivity(after, { now: 5, awaitingApproval: false }).verb).not.toContain(
      'hook',
    )
  })

  test('approval waits are still shown first', () => {
    const during = reduceLiveTurn({ ...INITIAL_LIVE_TURN, active: true }, started, 0)
    expect(describeLiveActivity(during, { now: 0, awaitingApproval: true }).verb).toBe(
      'Waiting for approval',
    )
  })

  test('hooks that pass stay quiet; others get one line', () => {
    expect(hookFinishedLine(finished())).toBeNull()
    expect(hookFinishedLine(finished({ outcome: 'blocked', message: 'no rm -rf' }))).toBe(
      'PreToolUse hook (bash) blocked: no rm -rf · guard.sh --token *** · 12 ms',
    )
    expect(
      hookFinishedLine(
        finished({ event: 'Stop', matcher: null, outcome: 'error', duration_ms: 1500 }),
      ),
    ).toBe('Stop hook failed · guard.sh --token *** · 1.5 s')
    expect(hookFinishedLine(finished({ outcome: 'cancelled' }))).toContain('was cancelled')
  })

  test('durations read in ms below a second', () => {
    expect(formatHookDuration(840)).toBe('840 ms')
    expect(formatHookDuration(1234)).toBe('1.2 s')
  })
})
