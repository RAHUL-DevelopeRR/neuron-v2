/**
 * Who is signed in, and how much of today's plan they used, as a UI shows it:
 * the account row and menu on the desktop, the account screen on the phone.
 *
 * Engines with the `account` feature send the whole account (name, plan,
 * limits, usage) as `account` events. Older ones send only the email and plan
 * in `status.account`; the rest then stays empty.
 */

import type { AccountDetails, AccountInfo } from './protocol'
import type { SessionState } from './session'

export interface UsageMeter {
  label: string
  used: number
  limit: number
  /** 0–100. */
  percent: number
}

export interface AccountView {
  signedIn: boolean
  email: string | null
  /** A name to show: the account's, or the part of the email before the @. */
  name: string | null
  /** One or two letters for the avatar. */
  initials: string
  /** `Free`, `Pro`, `Max`, or the gateway's own name for the plan. */
  plan: string | null
  meters: UsageMeter[]
  /** The most used of today's limits, 0–100; `null` when the limits are unknown. */
  todayPercent: number | null
  /** When today's usage resets, as an ISO time (UTC). */
  resetsAt: string | null
  creditsUsd: number | null
  suspended: boolean
  /** Why the plan and usage could not be read, when they could not. */
  usageError: string | null
  /** The gateway's own pages for this account, when it named them. */
  dashboardUrl: string | null
  pricingUrl: string | null
}

const PLAN_NAMES: Record<string, string> = { free: 'Free', pro: 'Pro', max: 'Max', team: 'Team' }

export function planLabel(plan: string | null | undefined): string | null {
  if (!plan?.trim()) return null
  const key = plan.trim().toLowerCase()
  return PLAN_NAMES[key] ?? plan.trim().replace(/^./, (first) => first.toUpperCase())
}

/** "Rahul S" → "RS", "rahul@zero-x.live" → "R", nothing → "?". */
export function initials(name: string | null | undefined, email?: string | null): string {
  const words = (name ?? '').trim().split(/\s+/).filter(Boolean)
  if (words.length >= 2) return (words[0][0] + words[words.length - 1][0]).toUpperCase()
  if (words.length === 1) return words[0].slice(0, 2).toUpperCase()
  const local = (email ?? '').split('@')[0]?.trim()
  return local ? local[0].toUpperCase() : '?'
}

function meter(label: string, used: number | null | undefined, limit: number | null | undefined): UsageMeter | null {
  if (used === null || used === undefined || !limit || limit <= 0) return null
  return { label, used, limit, percent: Math.min(100, Math.round((used / limit) * 100)) }
}

/**
 * The account to show. `signedIn` and `info` come from the engine's status;
 * `details`, from its `account` event, add the name and today's usage.
 */
export function accountView(signedIn: boolean | undefined, info: AccountInfo | null | undefined, details?: AccountDetails | null): AccountView {
  const email = details?.email ?? info?.email ?? null
  const name = details?.name?.trim() || (email ? email.split('@')[0] : null)
  const meters = [
    meter('Requests today', details?.usage_today?.requests, details?.limits?.requests),
    meter('Tokens today', details?.usage_today?.tokens, details?.limits?.tokens),
  ].filter((entry): entry is UsageMeter => entry !== null)
  return {
    signedIn: signedIn === true || Boolean(details || info?.email),
    email,
    name,
    initials: initials(details?.name, email),
    plan: details?.plan_name?.trim() || planLabel(details?.plan ?? info?.plan),
    meters,
    todayPercent: meters.length ? Math.max(...meters.map((entry) => entry.percent)) : null,
    resetsAt: details?.resets_at ?? null,
    creditsUsd: typeof details?.credit_balance_cents === 'number' ? details.credit_balance_cents / 100 : null,
    suspended: details?.suspended === true,
    usageError: details?.usage_error ?? null,
    dashboardUrl: details?.dashboard_url || null,
    pricingUrl: details?.pricing_url || null,
  }
}

/** The account a session's engine reports: its `account` events, else its status. */
export function sessionAccount(state: SessionState): AccountView {
  return accountView(state.status?.signed_in, state.status?.account, state.account)
}

/** "Pro · 37% of today used", or just the plan when usage is unknown. */
export function planLine(view: AccountView): string | null {
  if (!view.plan) return view.todayPercent === null ? null : `${view.todayPercent}% of today used`
  return view.todayPercent === null ? view.plan : `${view.plan} · ${view.todayPercent}% of today used`
}
