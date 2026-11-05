import { describe, expect, test } from 'bun:test'

import { quotaCardView, staleLimitNote } from '../quota-card'

import type { QuotaBlock } from '../../state/quota-store'

const RESET = Date.parse('2026-11-04T18:48:59.950Z')
const MIN = 60_000
const TZ = 'UTC'
const BLOCK: QuotaBlock = { messageId: 'ai-1', code: 'insufficient_quota', resumeAt: RESET, at: RESET - 3 * 60 * MIN }

/** The screenshot's error text, as the engine words it. */
const SCREENSHOT_ERROR =
  'This request needs 84,937 tokens but only 74,329 of your 1,000,000 tokens for this 5-hour window are left; enough frees up at 18:49 UTC. (insufficient_quota); resets at 2026-11-04T18:48:59.950Z; upgrade: https://zero-x.live/pricing'

describe('the quota error card', () => {
  test('counts down to the reset and offers auto-continue', () => {
    const view = quotaCardView({
      block: BLOCK,
      autoContinue: false,
      continueAt: null,
      stoppedReason: null,
      now: RESET - (2 * 60 + 14) * MIN - 30_000,
      timeZone: TZ,
    })
    expect(view).toEqual({
      title: 'Usage limit reached',
      stale: false,
      resetLine: 'Resets in 2h 14m (18:48)',
      toggle: '[ ] Auto-continue when limits reset',
      countdown: null,
    })
  })

  test('shows the countdown while armed', () => {
    const view = quotaCardView({
      block: BLOCK,
      autoContinue: true,
      continueAt: RESET + 10_000,
      stoppedReason: null,
      now: RESET - 4 * MIN,
      timeZone: TZ,
    })
    expect(view.toggle).toBe('[x] Auto-continue when limits reset')
    expect(view.countdown).toBe('Continuing in 4m 10s · Esc to cancel')
  })

  test('relabels itself once the limit has reset', () => {
    const view = quotaCardView({
      block: BLOCK,
      autoContinue: false,
      continueAt: null,
      stoppedReason: null,
      now: RESET + 16 * 60 * MIN,
      timeZone: TZ,
    })
    expect(view.title).toBe('Usage limit reset')
    expect(view.stale).toBe(true)
    expect(view.resetLine).toBe('Reset at 18:48 · send a message to go on')
    expect(view.toggle).toBeNull()
  })

  test('says why auto-continue stopped', () => {
    const view = quotaCardView({
      block: BLOCK,
      autoContinue: true,
      continueAt: null,
      stoppedReason: 'cancelled',
      now: RESET - MIN,
      timeZone: TZ,
    })
    expect(view.countdown).toBe('Auto-continue cancelled')
  })

  test('a limit with no reset time offers no toggle', () => {
    const view = quotaCardView({
      block: { ...BLOCK, code: 'rate_limited', resumeAt: null },
      autoContinue: true,
      continueAt: null,
      stoppedReason: null,
      now: RESET,
      timeZone: TZ,
    })
    expect(view.title).toBe('Rate limit reached')
    expect(view.resetLine).toBeNull()
    expect(view.toggle).toBeNull()
  })
})

describe('older error banners', () => {
  test('a reset time that has passed adds a note', () => {
    expect(staleLimitNote(SCREENSHOT_ERROR, RESET - MIN, TZ)).toBeNull()
    // Rahul's terminal, 16 hours later.
    expect(staleLimitNote(SCREENSHOT_ERROR, RESET + 16 * 60 * MIN, TZ)).toBe(
      'This limit reset at 18:48; send a message to go on.',
    )
    expect(staleLimitNote(SCREENSHOT_ERROR, RESET + 30 * 60 * MIN, TZ)).toBe(
      'This limit reset at Nov 4, 18:48; send a message to go on.',
    )
    expect(staleLimitNote('Connection refused', RESET, TZ)).toBeNull()
  })
})
