import { afterEach, beforeEach, describe, expect, test } from 'bun:test'

import { STATUS_BAR_ACCOUNT } from '../../testing/status-bar-fixtures'
import { useQuotaStore } from '../../state/quota-store'
import {
  AUTO_CONTINUE_GRACE_MS,
  AUTO_CONTINUE_MAX_ATTEMPTS,
  IDLE_REFRESH_BASE_MS,
  RESET_GRACE_MS,
  SCHEDULER_TICK_MS,
} from '../../utils/quota-schedule'
import { createQuotaScheduler, recordTurnOutcome, type SchedulerClock } from '../quota-scheduler'

import type { AccountDetails } from '../protocol'

const MIN = 60_000
const HOUR = 60 * MIN
const START = Date.parse('2026-11-04T14:00:00Z')
const RESET = Date.parse('2026-11-04T18:48:59.950Z')

/** A clock the test moves; each interval fires once per elapsed period. */
class FakeClock implements SchedulerClock {
  time = START
  private timers = new Map<number, { callback: () => void; every: number; next: number }>()
  private ids = 0
  now = () => this.time
  setInterval = (callback: () => void, every: number) => {
    const id = ++this.ids
    this.timers.set(id, { callback, every, next: this.time + every })
    return id
  }
  clearInterval = (handle: unknown) => {
    this.timers.delete(handle as number)
  }
  advance(ms: number) {
    const end = this.time + ms
    for (;;) {
      const due = [...this.timers.values()].filter((t) => t.next <= end).sort((a, b) => a.next - b.next)[0]
      if (!due) break
      this.time = due.next
      due.next += due.every
      due.callback()
    }
    this.time = end
  }
  /** A laptop asleep: time jumps with no ticks, then one tick on waking. */
  sleep(ms: number) {
    this.time += ms
    for (const timer of this.timers.values()) timer.next = this.time + timer.every
  }
  get active() {
    return this.timers.size
  }
}

function freeAccount(windowResets: string, tokens: number): AccountDetails {
  return {
    ...STATUS_BAR_ACCOUNT,
    resets_at: '2026-11-05T00:00:00Z',
    usage_window: { ...STATUS_BAR_ACCOUNT.usage_window!, tokens, resets_at: windowResets },
  }
}

function setup(account: AccountDetails | null = freeAccount('2026-11-04T18:48:59.950Z', 925_671)) {
  const clock = new FakeClock()
  const calls = { refresh: [] as number[], continues: [] as number[] }
  const state = { busy: false, signedIn: true, account }
  const scheduler = createQuotaScheduler({
    clock,
    refreshAccount: () => calls.refresh.push(clock.time),
    continueTurn: () => calls.continues.push(clock.time),
    isBusy: () => state.busy,
    isSignedIn: () => state.signedIn,
    getAccount: () => state.account,
  })
  return { clock, calls, state, scheduler }
}

function blockAt(resumeAt: number | null, code = 'insufficient_quota') {
  useQuotaStore.getState().limitHit({ messageId: 'ai-1', code, resumeAt, at: START })
}

beforeEach(() => {
  useQuotaStore.getState().clear()
  useQuotaStore.getState().setAutoContinue(false, false)
})
afterEach(() => {
  useQuotaStore.getState().clear()
})

describe('account refresh', () => {
  test('reads the account just after the 5-hour window resets', () => {
    const { clock, calls } = setup()
    clock.advance(RESET - START - MIN)
    // Only idle re-reads so far: 10, 30 and 70 minutes in, the usage unchanged.
    const beforeReset = calls.refresh.length
    clock.advance(MIN + RESET_GRACE_MS + SCHEDULER_TICK_MS)
    expect(calls.refresh.length).toBe(beforeReset + 1)
    const last = calls.refresh.at(-1)!
    expect(last).toBeGreaterThanOrEqual(RESET + RESET_GRACE_MS)
    expect(last).toBeLessThan(RESET + RESET_GRACE_MS + SCHEDULER_TICK_MS)
  })

  test('idle re-reads back off while nothing changes, and start over when usage moves', () => {
    const { clock, calls, state, scheduler } = setup(freeAccount('2026-11-05T09:00:00Z', 10))
    const unchanged = () => scheduler.accountArrived(state.account)
    clock.advance(IDLE_REFRESH_BASE_MS + SCHEDULER_TICK_MS)
    expect(calls.refresh.length).toBe(1)
    unchanged()
    clock.advance(20 * MIN + SCHEDULER_TICK_MS)
    expect(calls.refresh.length).toBe(2)
    unchanged()
    clock.advance(30 * MIN)
    expect(calls.refresh.length).toBe(2) // 40 minutes now
    clock.advance(10 * MIN + SCHEDULER_TICK_MS)
    expect(calls.refresh.length).toBe(3)
    state.account = freeAccount('2026-11-05T09:00:00Z', 50_000)
    scheduler.accountArrived(state.account)
    clock.advance(IDLE_REFRESH_BASE_MS + SCHEDULER_TICK_MS)
    expect(calls.refresh.length).toBe(4)
  })

  test('reads nothing while a turn runs or while signed out', () => {
    const { clock, calls, state } = setup()
    state.busy = true
    clock.advance(6 * HOUR)
    expect(calls.refresh).toEqual([])
    state.busy = false
    state.signedIn = false
    clock.advance(HOUR)
    expect(calls.refresh).toEqual([])
  })

  test('a terminal that slept past the reset reads the account on its first tick', () => {
    const { clock, calls, scheduler } = setup()
    scheduler.activity()
    clock.sleep(16 * HOUR)
    clock.advance(SCHEDULER_TICK_MS)
    expect(calls.refresh).toEqual([START + 16 * HOUR + SCHEDULER_TICK_MS])
  })

  test('stop clears the tick', () => {
    const { clock, scheduler } = setup()
    expect(clock.active).toBe(1)
    scheduler.stop()
    expect(clock.active).toBe(0)
  })
})

describe('auto-continue', () => {
  test('is off by default: a quota error only waits', () => {
    const { clock, calls } = setup()
    blockAt(RESET)
    clock.advance(6 * HOUR)
    expect(calls.continues).toEqual([])
    expect(useQuotaStore.getState().continueAt).toBeNull()
  })

  test('sends "continue" once, just after the reset, when turned on', () => {
    const { clock, calls } = setup()
    useQuotaStore.getState().setAutoContinue(true, false)
    blockAt(RESET)
    expect(useQuotaStore.getState().continueAt).toBe(RESET + AUTO_CONTINUE_GRACE_MS)
    clock.advance(RESET - START)
    expect(calls.continues).toEqual([])
    clock.advance(AUTO_CONTINUE_GRACE_MS + SCHEDULER_TICK_MS)
    expect(calls.continues).toHaveLength(1)
    expect(calls.continues[0]).toBeGreaterThanOrEqual(RESET + AUTO_CONTINUE_GRACE_MS)
    clock.advance(HOUR)
    expect(calls.continues).toHaveLength(1)
    expect(useQuotaStore.getState().attempts).toBe(1)
  })

  test('turning it on from the card arms the error already shown', () => {
    const { clock, calls } = setup()
    blockAt(RESET)
    clock.advance(HOUR)
    useQuotaStore.getState().setAutoContinue(true, false)
    clock.advance(RESET - START)
    expect(calls.continues).toHaveLength(1)
  })

  test('Esc cancels it', () => {
    const { clock, calls } = setup()
    useQuotaStore.getState().setAutoContinue(true, false)
    blockAt(RESET)
    useQuotaStore.getState().cancel()
    expect(useQuotaStore.getState().stoppedReason).toBe('cancelled')
    clock.advance(6 * HOUR)
    expect(calls.continues).toEqual([])
  })

  test('waits for a running turn to end', () => {
    const { clock, calls, state } = setup()
    useQuotaStore.getState().setAutoContinue(true, false)
    blockAt(RESET)
    state.busy = true
    clock.advance(RESET - START + HOUR)
    expect(calls.continues).toEqual([])
    state.busy = false
    clock.advance(SCHEDULER_TICK_MS)
    expect(calls.continues).toHaveLength(1)
  })

  test('survives a sleep through the reset', () => {
    const { clock, calls } = setup()
    useQuotaStore.getState().setAutoContinue(true, false)
    blockAt(RESET)
    clock.sleep(16 * HOUR)
    clock.advance(SCHEDULER_TICK_MS)
    expect(calls.continues).toHaveLength(1)
  })

  test('a limit without a reset time is never armed', () => {
    const { clock, calls } = setup()
    useQuotaStore.getState().setAutoContinue(true, false)
    blockAt(null)
    clock.advance(24 * HOUR)
    expect(calls.continues).toEqual([])
  })

  test(`gives up after ${AUTO_CONTINUE_MAX_ATTEMPTS} limits in a row`, () => {
    const { clock, calls } = setup()
    useQuotaStore.getState().setAutoContinue(true, false)
    let resumeAt = START + MIN
    for (let attempt = 0; attempt < AUTO_CONTINUE_MAX_ATTEMPTS; attempt += 1) {
      blockAt(resumeAt)
      clock.advance(resumeAt - clock.time + AUTO_CONTINUE_GRACE_MS + SCHEDULER_TICK_MS)
      resumeAt = clock.time + MIN
    }
    expect(calls.continues).toHaveLength(AUTO_CONTINUE_MAX_ATTEMPTS)
    blockAt(resumeAt)
    expect(useQuotaStore.getState().continueAt).toBeNull()
    expect(useQuotaStore.getState().stoppedReason).toContain('stopped after')
    clock.advance(HOUR)
    expect(calls.continues).toHaveLength(AUTO_CONTINUE_MAX_ATTEMPTS)
    // A turn that ends well starts the count over.
    useQuotaStore.getState().clear()
    expect(useQuotaStore.getState().attempts).toBe(0)
  })
})

describe('turn outcomes', () => {
  const quotaDetail = {
    code: 'insufficient_quota',
    status: 429,
    upgrade_url: null,
    resets_at: '2026-11-04T18:48:59.950Z',
    retry_after_seconds: null,
  }

  test('a quota error puts the card up and arms auto-continue when on', () => {
    useQuotaStore.getState().setAutoContinue(true, false)
    recordTurnOutcome('ai-7', { error: 'quota', detail: quotaDetail }, START)
    const state = useQuotaStore.getState()
    expect(state.block).toEqual({ messageId: 'ai-7', code: 'insufficient_quota', resumeAt: RESET, at: START })
    expect(state.continueAt).toBe(RESET + AUTO_CONTINUE_GRACE_MS)
  })

  test('never arms after any other error, and a good turn clears the card', () => {
    useQuotaStore.getState().setAutoContinue(true, false)
    recordTurnOutcome('ai-7', { error: 'quota', detail: quotaDetail }, START)
    for (const code of ['insufficient_credits', 'service_unavailable', 'unreachable', 'invalid_session']) {
      recordTurnOutcome('ai-8', { error: code, detail: { ...quotaDetail, code } }, START)
      expect(useQuotaStore.getState().block).toBeNull()
      expect(useQuotaStore.getState().continueAt).toBeNull()
    }
    recordTurnOutcome('ai-9', { error: 'boom', detail: null }, START)
    expect(useQuotaStore.getState().continueAt).toBeNull()
    recordTurnOutcome('ai-7', { error: 'quota', detail: quotaDetail }, START)
    recordTurnOutcome('ai-10', { error: null }, START)
    expect(useQuotaStore.getState().block).toBeNull()
  })
})
