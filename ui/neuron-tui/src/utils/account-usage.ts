// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the reset
// wording follows formatRenewalDate in Freebuff's
// cli/src/components/usage-banner.tsx (a time of day when the reset is today,
// else a short date), with "in 3h 10m" added; the rest reads Zero-X's
// account. See ui/neuron-tui/PORTING.md.
/**
 * The Zero-X account rows of the context popover: the plan's 5-hour token
 * window and today's requests, with when each resets.
 */

import { formatElapsedTime } from './format-elapsed-time'
import { formatTokenCount } from './format-token-count'
import { usageWindowPercent } from './status-bar-parts'

import type { AccountDetails, UsageEvent } from '../engine/protocol'

/** "14:32 (in 3h 10m)" today, "Nov 4" later, `null` without a time. */
export function formatReset(
  iso: string | null | undefined,
  now: number,
  timeZone?: string,
): string | null {
  if (!iso) return null
  const at = Date.parse(iso)
  if (Number.isNaN(at)) return null
  const date = new Date(at)
  const sameDay =
    date.toLocaleDateString('en-US', { timeZone }) === new Date(now).toLocaleDateString('en-US', { timeZone })
  const minutes = Math.max(0, Math.ceil((at - now) / 60_000))
  const inText = minutes > 0 ? ` (in ${formatElapsedTime(minutes * 60)})` : ''
  if (sameDay || at - now < 24 * 3_600_000) {
    const time = date.toLocaleTimeString('en-US', {
      hour: '2-digit',
      minute: '2-digit',
      hour12: false,
      timeZone,
    })
    return `${time}${inText}`
  }
  return date.toLocaleDateString('en-US', { month: 'short', day: 'numeric', timeZone })
}

export interface AccountRow {
  label: string
  value: string
  /** The window's fill (0–100) for a meter, when there is one. */
  percent: number | null
  resets: string | null
}

/** The account's rows; one row saying why when usage could not be read. */
export function accountRows(account: AccountDetails, now: number, timeZone?: string): AccountRow[] {
  if (account.usage_error) {
    return [{ label: 'Usage', value: `unavailable: ${account.usage_error}`, percent: null, resets: null }]
  }
  const rows: AccountRow[] = []
  const window = account.usage_window
  const windowPercent = usageWindowPercent(account)
  if (window && (window.limit_tokens || window.tokens !== null)) {
    const used =
      window.tokens ??
      (window.limit_tokens !== null && window.remaining_tokens !== null
        ? window.limit_tokens - window.remaining_tokens
        : null)
    const hours = window.window_seconds ? Math.round(window.window_seconds / 3600) : 5
    rows.push({
      label: `${hours}-hour window`,
      value: `${used !== null ? formatTokenCount(used) : '?'} / ${
        window.limit_tokens ? formatTokenCount(window.limit_tokens) : '?'
      } tokens`,
      percent: windowPercent,
      resets: formatReset(window.resets_at, now, timeZone),
    })
  }
  const requests = account.usage_today?.requests
  const requestLimit = account.limits?.requests
  const tokens = account.usage_today?.tokens
  const tokenLimit = account.limits?.tokens
  if (requests !== null && requests !== undefined) {
    rows.push({
      label: 'Today',
      value: requestLimit ? `${requests} / ${requestLimit} requests` : `${requests} requests`,
      percent: null,
      resets: formatReset(account.resets_at, now, timeZone),
    })
  } else if (tokens !== null && tokens !== undefined) {
    rows.push({
      label: 'Today',
      value: tokenLimit
        ? `${formatTokenCount(tokens)} / ${formatTokenCount(tokenLimit)} tokens`
        : `${formatTokenCount(tokens)} tokens`,
      percent: null,
      resets: formatReset(account.resets_at, now, timeZone),
    })
  }
  return rows
}

/** "Free (dev@example.com)". */
export function accountTitle(account: AccountDetails): string {
  const plan = account.plan_name ?? account.plan ?? 'Zero-X'
  return account.email ? `${plan} (${account.email})` : plan
}

/** "Updated 2m ago", from `usage_updated_at`. */
export function updatedAgo(account: AccountDetails, now: number): string | null {
  if (!account.usage_updated_at) return null
  const seconds = Math.max(0, Math.floor(now / 1000 - account.usage_updated_at))
  if (seconds < 60) return 'Updated just now'
  if (seconds >= 86_400) return `Updated ${Math.floor(seconds / 86_400)}d ago`
  return `Updated ${formatElapsedTime(Math.floor(seconds / 60) * 60)} ago`
}

/**
 * The session's tokens, "↑12.4K ↓2.1K": the last totals plus what a running
 * turn has used so far, marked `~` while estimated.
 */
export function sessionTokens(
  usage: Omit<UsageEvent, 'type'> | null,
  live: { input: number; output: number } | null,
): string | null {
  if (!usage && !live) return null
  const total = usage?.cumulative
  const input = (total ? total.input + total.cache_read + total.cache_write : 0) + (live?.input ?? 0)
  const output = (total?.output ?? 0) + (live?.output ?? 0)
  const mark = usage?.estimated || live ? '~' : ''
  return `↑${mark}${formatTokenCount(input)} ↓${mark}${formatTokenCount(output)}`
}
