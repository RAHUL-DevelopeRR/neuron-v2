/**
 * When the plan's usage should be read again, and when a turn a usage limit
 * stopped may go on. Pure: every function takes the time it reasons about, so
 * the scheduler (engine/quota-scheduler.ts) can run on a fake clock in tests.
 *
 * A session left open used to keep showing the usage of the moment it was last
 * read (a Free window at 93%, a quota error card) long after the window reset.
 * Now the account is read again shortly after each reset time it carries, and
 * every so often while the terminal sits idle, backing off while nothing moves.
 */
import type { AccountDetails, ErrorDetail } from '../engine/protocol'

/** Read the account this long after a reset, so the gateway has moved on. */
export const RESET_GRACE_MS = 5_000
/** The first idle re-read; it doubles while the usage stays the same. */
export const IDLE_REFRESH_BASE_MS = 10 * 60_000
/** The longest wait between idle re-reads. */
export const IDLE_REFRESH_MAX_MS = 60 * 60_000
/**
 * How often the scheduler looks at the clock. A tick instead of one long
 * timer, so a laptop that slept through a reset still acts when it wakes.
 */
export const SCHEDULER_TICK_MS = 15_000
/** Auto-continue waits this long after the reset before it sends "continue". */
export const AUTO_CONTINUE_GRACE_MS = 10_000
/** Auto-continues in a row a fresh limit may stop before the toggle gives up. */
export const AUTO_CONTINUE_MAX_ATTEMPTS = 3
/** What auto-continue sends. */
export const AUTO_CONTINUE_PROMPT = 'continue'

/** Errors of a usage limit that resets by itself; only these auto-continue. */
const RESETTING_LIMIT_CODES = new Set(['insufficient_quota', 'rate_limited'])

/** Whether `detail` is a usage limit that frees up by itself. */
export function isResettingLimit(detail: ErrorDetail | null | undefined): boolean {
  return !!detail && RESETTING_LIMIT_CODES.has(detail.code)
}

/**
 * When the limit in `detail` frees up (ms since the epoch): its reset time, or
 * `receivedAt` plus its retry delay. `null` for any other error, and for a
 * limit that says neither.
 */
export function limitResumeAt(
  detail: ErrorDetail | null | undefined,
  receivedAt: number,
): number | null {
  if (!detail || !isResettingLimit(detail)) return null
  const at = parseTime(detail.resets_at)
  if (at !== null) return at
  const seconds = detail.retry_after_seconds
  if (typeof seconds === 'number' && Number.isFinite(seconds) && seconds >= 0) {
    return receivedAt + seconds * 1000
  }
  return null
}

/** The reset times an account carries: its usage window's and the day's. */
export function accountResetTimes(account: AccountDetails | null | undefined): number[] {
  if (!account) return []
  return [parseTime(account.usage_window?.resets_at), parseTime(account.resets_at)].filter(
    (at): at is number => at !== null,
  )
}

export interface RefreshPlanInput {
  /** The last account read, when there is one. */
  account: AccountDetails | null
  /** Reset times known from elsewhere, such as a quota error. */
  extraResets?: number[]
  /** When the account was last asked for (or arrived), ms. */
  lastRefreshAt: number
  /** Idle re-reads in a row that found the usage unchanged. */
  idleStreak: number
}

/** The idle re-read's wait after `idleStreak` unchanged reads. */
export function idleRefreshDelay(idleStreak: number): number {
  const factor = 2 ** Math.max(0, Math.min(idleStreak, 10))
  return Math.min(IDLE_REFRESH_BASE_MS * factor, IDLE_REFRESH_MAX_MS)
}

/**
 * When the account should next be read (ms): just after the first reset that
 * falls after the last read, else the idle re-read.
 */
export function nextAccountRefreshAt({
  account,
  extraResets = [],
  lastRefreshAt,
  idleStreak,
}: RefreshPlanInput): { at: number; reason: 'reset' | 'idle' } {
  const idleAt = lastRefreshAt + idleRefreshDelay(idleStreak)
  const resets = [...accountResetTimes(account), ...extraResets]
    .filter((at) => at >= lastRefreshAt)
    .map((at) => at + RESET_GRACE_MS)
  const resetAt = resets.length > 0 ? Math.min(...resets) : Number.POSITIVE_INFINITY
  return resetAt <= idleAt ? { at: resetAt, reason: 'reset' } : { at: idleAt, reason: 'idle' }
}

/** What an account read says about usage, to tell whether anything moved. */
export function usageFingerprint(account: AccountDetails | null | undefined): string {
  if (!account) return ''
  const window = account.usage_window
  return JSON.stringify([
    account.usage_today?.requests ?? null,
    account.usage_today?.tokens ?? null,
    window?.tokens ?? null,
    window?.remaining_tokens ?? null,
    window?.resets_at ?? null,
    account.resets_at,
    account.usage_error,
  ])
}

/**
 * A wait as people read it: "2h 14m", "4m 12s", "12s". Hours drop the
 * seconds; a day or more reads "1d 3h".
 */
export function formatCountdown(ms: number): string {
  const total = Math.max(0, Math.ceil(ms / 1000))
  const days = Math.floor(total / 86_400)
  const hours = Math.floor((total % 86_400) / 3600)
  const minutes = Math.floor((total % 3600) / 60)
  const seconds = total % 60
  if (days > 0) return `${days}d ${hours}h`
  if (hours > 0) return `${hours}h ${minutes}m`
  if (minutes > 0) return `${minutes}m ${seconds}s`
  return `${seconds}s`
}

function parseTime(iso: string | null | undefined): number | null {
  if (!iso) return null
  const at = Date.parse(iso)
  return Number.isNaN(at) ? null : at
}
