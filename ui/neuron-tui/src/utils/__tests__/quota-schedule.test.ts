import { describe, expect, test } from 'bun:test'

import { STATUS_BAR_ACCOUNT } from '../../testing/status-bar-fixtures'
import {
  accountResetTimes,
  formatCountdown,
  IDLE_REFRESH_BASE_MS,
  IDLE_REFRESH_MAX_MS,
  idleRefreshDelay,
  isResettingLimit,
  limitResumeAt,
  nextAccountRefreshAt,
  RESET_GRACE_MS,
  usageFingerprint,
} from '../quota-schedule'

import type { AccountDetails, ErrorDetail } from '../../engine/protocol'

const MIN = 60_000
const HOUR = 60 * MIN

/** The 429 Rahul hit: the Free 5-hour window, resetting at 18:48:59Z. */
const QUOTA: ErrorDetail = {
  code: 'insufficient_quota',
  status: 429,
  upgrade_url: 'https://zero-x.live/pricing',
  resets_at: '2026-11-04T18:48:59.950Z',
  retry_after_seconds: null,
}
const RESET = Date.parse('2026-11-04T18:48:59.950Z')

function accountWithWindow(resetsAt: string, tokens = 925_671): AccountDetails {
  return {
    ...STATUS_BAR_ACCOUNT,
    resets_at: '2026-11-05T00:00:00Z',
    usage_window: { ...STATUS_BAR_ACCOUNT.usage_window!, tokens, resets_at: resetsAt },
  }
}

describe('limits that reset by themselves', () => {
  test('only quota and rate limits count', () => {
    expect(isResettingLimit(QUOTA)).toBe(true)
    expect(isResettingLimit({ ...QUOTA, code: 'rate_limited' })).toBe(true)
    for (const code of ['insufficient_credits', 'upgrade_required', 'invalid_session', 'unreachable', 'capacity_exhausted']) {
      expect(isResettingLimit({ ...QUOTA, code })).toBe(false)
    }
    expect(isResettingLimit(null)).toBe(false)
  })

  test('the reset time comes from resets_at, else the retry delay', () => {
    expect(limitResumeAt(QUOTA, 0)).toBe(RESET)
    expect(
      limitResumeAt({ ...QUOTA, code: 'rate_limited', resets_at: null, retry_after_seconds: 30 }, 1_000),
    ).toBe(31_000)
    expect(limitResumeAt({ ...QUOTA, resets_at: null }, 1_000)).toBeNull()
    expect(limitResumeAt({ ...QUOTA, code: 'service_unavailable', retry_after_seconds: 5 }, 0)).toBeNull()
  })
})

describe('when the account is read again', () => {
  test('just after the window resets, when that comes before the idle re-read', () => {
    const lastRefreshAt = RESET - 2 * MIN
    expect(
      nextAccountRefreshAt({
        account: accountWithWindow('2026-11-04T18:48:59.950Z'),
        lastRefreshAt,
        idleStreak: 0,
      }),
    ).toEqual({ at: RESET + RESET_GRACE_MS, reason: 'reset' })
  })

  test('a reset that passed before the last read is not read again', () => {
    const lastRefreshAt = RESET + MIN
    const plan = nextAccountRefreshAt({
      account: accountWithWindow('2026-11-04T18:48:59.950Z'),
      lastRefreshAt,
      idleStreak: 0,
    })
    expect(plan).toEqual({ at: lastRefreshAt + IDLE_REFRESH_BASE_MS, reason: 'idle' })
  })

  test('a reset that passed after the last read is due at once', () => {
    // Backed off to hourly reads; the last one was 30 minutes before the reset,
    // and the terminal is looked at again 16 hours later.
    const plan = nextAccountRefreshAt({
      account: accountWithWindow('2026-11-04T18:48:59.950Z'),
      lastRefreshAt: RESET - 30 * MIN,
      idleStreak: 6,
    })
    expect(plan).toEqual({ at: RESET + RESET_GRACE_MS, reason: 'reset' })
    expect(plan.at).toBeLessThan(RESET + 16 * HOUR)
  })

  test('a quota error\'s reset counts when the account has none', () => {
    const plan = nextAccountRefreshAt({
      account: null,
      extraResets: [RESET],
      lastRefreshAt: RESET - HOUR * 4,
      idleStreak: 0,
    })
    expect(plan).toEqual({ at: RESET - HOUR * 4 + IDLE_REFRESH_BASE_MS, reason: 'idle' })
    const closer = nextAccountRefreshAt({
      account: null,
      extraResets: [RESET],
      lastRefreshAt: RESET - MIN,
      idleStreak: 0,
    })
    expect(closer.reason).toBe('reset')
  })

  test('the idle re-read backs off to an hour', () => {
    expect(idleRefreshDelay(0)).toBe(10 * MIN)
    expect(idleRefreshDelay(1)).toBe(20 * MIN)
    expect(idleRefreshDelay(2)).toBe(40 * MIN)
    expect(idleRefreshDelay(3)).toBe(IDLE_REFRESH_MAX_MS)
    expect(idleRefreshDelay(50)).toBe(IDLE_REFRESH_MAX_MS)
  })

  test('both reset times are read from the account', () => {
    expect(accountResetTimes(accountWithWindow('2026-11-04T18:48:59.950Z'))).toEqual([
      RESET,
      Date.parse('2026-11-05T00:00:00Z'),
    ])
    expect(accountResetTimes({ ...STATUS_BAR_ACCOUNT, resets_at: null, usage_window: null })).toEqual([])
  })

  test('the fingerprint moves with usage, not with the read time', () => {
    const before = accountWithWindow('2026-11-04T18:48:59.950Z')
    expect(usageFingerprint({ ...before, usage_updated_at: 1 })).toBe(usageFingerprint(before))
    expect(usageFingerprint(accountWithWindow('2026-11-04T23:48:00Z', 0))).not.toBe(usageFingerprint(before))
  })
})

describe('countdowns', () => {
  test('read in days, hours, minutes or seconds', () => {
    expect(formatCountdown(2 * HOUR + 14 * MIN + 30_000)).toBe('2h 14m')
    expect(formatCountdown(4 * MIN + 12_000)).toBe('4m 12s')
    expect(formatCountdown(11_200)).toBe('12s')
    expect(formatCountdown(-5)).toBe('0s')
    expect(formatCountdown(27 * HOUR)).toBe('1d 3h')
  })
})
