/**
 * Keeps the plan's usage current while the TUI stays open, and sends
 * "continue" once a usage limit that stopped a turn resets (when the user
 * turned auto-continue on). Plans come from utils/quota-schedule.ts; the
 * state the card shows is state/quota-store.ts.
 *
 * It looks at the clock every {@link SCHEDULER_TICK_MS} instead of setting
 * one timer for the reset, so a terminal left idle for hours (or a laptop
 * that slept) still acts on time when it next ticks. Nothing is read or sent
 * while a turn runs: the turn's own replies update the usage.
 */
import { useQuotaStore } from '../state/quota-store'
import {
  isResettingLimit,
  limitResumeAt,
  nextAccountRefreshAt,
  SCHEDULER_TICK_MS,
  usageFingerprint,
} from '../utils/quota-schedule'

import type { TurnOutcome } from './neuron-engine-client'
import type { AccountDetails } from './protocol'

export interface SchedulerClock {
  now(): number
  setInterval(callback: () => void, ms: number): unknown
  clearInterval(handle: unknown): void
}

export const systemSchedulerClock: SchedulerClock = {
  now: () => Date.now(),
  setInterval: (callback, ms) => {
    const handle = setInterval(callback, ms)
    // The tick must not keep a closing TUI alive.
    ;(handle as { unref?: () => void }).unref?.()
    return handle
  },
  clearInterval: (handle) => clearInterval(handle as ReturnType<typeof setInterval>),
}

export interface QuotaSchedulerDeps {
  clock?: SchedulerClock
  /** Asks the engine for the account (`refresh_account`). */
  refreshAccount: () => void
  /** Sends "continue" as a prompt. */
  continueTurn: () => void
  /** A turn (or another exchange) is running. */
  isBusy: () => boolean
  /** There is a Zero-X login whose usage can be read. */
  isSignedIn: () => boolean
  /** The account from the last `account` event. */
  getAccount: () => AccountDetails | null
}

export interface QuotaScheduler {
  /** An `account` event arrived. */
  accountArrived: (account: AccountDetails | null) => void
  /** A turn ran: its replies carried the usage, so the idle wait starts over. */
  activity: () => void
  /** Runs one tick now (tests, and right after waking). */
  tick: () => void
  stop: () => void
}

export function createQuotaScheduler(deps: QuotaSchedulerDeps): QuotaScheduler {
  const clock = deps.clock ?? systemSchedulerClock
  let lastRefreshAt = clock.now()
  let idleStreak = 0
  let lastFingerprint = usageFingerprint(deps.getAccount())
  let pendingIdleRead = false

  const tick = () => {
    if (deps.isBusy()) return
    const now = clock.now()
    const quota = useQuotaStore.getState()

    if (quota.continueAt !== null && now >= quota.continueAt) {
      quota.fired()
      lastRefreshAt = now
      deps.continueTurn()
      return
    }

    if (!deps.isSignedIn()) return
    const resumeAt = quota.block?.resumeAt
    const plan = nextAccountRefreshAt({
      account: deps.getAccount(),
      extraResets: resumeAt != null ? [resumeAt] : [],
      lastRefreshAt,
      idleStreak,
    })
    if (now < plan.at) return
    lastRefreshAt = now
    pendingIdleRead = plan.reason === 'idle'
    deps.refreshAccount()
  }

  const handle = clock.setInterval(tick, SCHEDULER_TICK_MS)

  return {
    accountArrived: (account) => {
      const fingerprint = usageFingerprint(account)
      if (fingerprint !== lastFingerprint) {
        idleStreak = 0
      } else if (pendingIdleRead) {
        idleStreak += 1
      }
      pendingIdleRead = false
      lastFingerprint = fingerprint
    },
    activity: () => {
      lastRefreshAt = clock.now()
      idleStreak = 0
    },
    tick,
    stop: () => clock.clearInterval(handle),
  }
}

/** The TUI's scheduler, while one runs (see chat.tsx). */
let active: QuotaScheduler | null = null

/** Starts the TUI's scheduler; the returned function stops it. */
export function startQuotaScheduler(deps: QuotaSchedulerDeps): () => void {
  active?.stop()
  const scheduler = createQuotaScheduler(deps)
  active = scheduler
  return () => {
    scheduler.stop()
    if (active === scheduler) active = null
  }
}

/** An `account` event arrived (engine-session.ts). */
export function noteAccountArrived(account: AccountDetails | null): void {
  active?.accountArrived(account)
}

/**
 * A prompt's turn ended on `messageId`: a resetting usage limit puts its card
 * up (and arms auto-continue when that is on); anything else takes it down, so
 * no other error ever auto-continues.
 */
export function recordTurnOutcome(
  messageId: string,
  outcome: TurnOutcome,
  now: number = Date.now(),
): void {
  active?.activity()
  const quota = useQuotaStore.getState()
  if (outcome.error && isResettingLimit(outcome.detail)) {
    quota.limitHit({
      messageId,
      code: outcome.detail!.code,
      resumeAt: limitResumeAt(outcome.detail, now),
      at: now,
    })
  } else {
    quota.clear()
  }
}
