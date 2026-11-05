/**
 * What a usage-limit error card says, from the limit and the clock:
 * components/quota-error-card.tsx draws it. Before the reset it counts down
 * ("Resets in 2h 14m (18:49)") and offers auto-continue; after, the card no
 * longer reads as a live error. Any older error banner whose text names a
 * reset time that has passed gets the same relabel (see {@link staleLimitNote}).
 */
import { formatCountdown } from './quota-schedule'

import type { QuotaBlock } from '../state/quota-store'

export interface QuotaCardInput {
  block: QuotaBlock
  autoContinue: boolean
  continueAt: number | null
  stoppedReason: string | null
  now: number
  timeZone?: string
}

export interface QuotaCardView {
  title: string
  /** `true` once the limit has reset: drawn muted instead of red. */
  stale: boolean
  /** "Resets in 2h 14m (18:49)" or "Reset at 18:49 · send a message to go on". */
  resetLine: string | null
  /** The toggle, as "[x] Auto-continue when limits reset". `null` once reset. */
  toggle: string | null
  /** "Continuing in 4m 12s · Esc to cancel", or why it will not. */
  countdown: string | null
}

export function quotaCardView({
  block,
  autoContinue,
  continueAt,
  stoppedReason,
  now,
  timeZone,
}: QuotaCardInput): QuotaCardView {
  const label = block.code === 'rate_limited' ? 'Rate limit' : 'Usage limit'
  const resumeAt = block.resumeAt
  const stale = resumeAt !== null && resumeAt <= now
  const clock = resumeAt !== null ? clockTime(resumeAt, now, timeZone) : null

  if (stale && continueAt === null) {
    return {
      title: `${label} reset`,
      stale: true,
      resetLine: `Reset at ${clock} · send a message to go on`,
      toggle: null,
      countdown: stoppedReason ? `Auto-continue ${stoppedReason}` : null,
    }
  }
  return {
    title: `${label} reached`,
    stale: false,
    resetLine:
      resumeAt === null
        ? null
        : stale
          ? `Reset at ${clock}`
          : `Resets in ${formatCountdown(resumeAt - now)} (${clock})`,
    toggle:
      resumeAt === null ? null : `[${autoContinue ? 'x' : ' '}] Auto-continue when limits reset`,
    countdown:
      continueAt !== null
        ? `Continuing in ${formatCountdown(continueAt - now)} · Esc to cancel`
        : stoppedReason
          ? `Auto-continue ${stoppedReason}`
          : null,
  }
}

const RESETS_AT = /resets at (\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?Z)/

/**
 * For an error banner whose text says when its limit resets, once that time
 * has passed: a note that it has. `null` otherwise.
 */
export function staleLimitNote(error: string, now: number, timeZone?: string): string | null {
  const match = RESETS_AT.exec(error)
  if (!match) return null
  const at = Date.parse(match[1]!)
  if (Number.isNaN(at) || at > now) return null
  return `This limit reset at ${clockTime(at, now, timeZone)}; send a message to go on.`
}

/** "18:49" within a day of `now`, else "Nov 4, 18:49". */
function clockTime(at: number, now: number, timeZone?: string): string {
  const date = new Date(at)
  const time = date.toLocaleTimeString('en-US', {
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
    timeZone,
  })
  if (Math.abs(at - now) < 24 * 3_600_000) return time
  const day = date.toLocaleDateString('en-US', { month: 'short', day: 'numeric', timeZone })
  return `${day}, ${time}`
}
