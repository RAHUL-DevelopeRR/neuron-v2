/**
 * Gateway failures a person can act on: sign in again, change plan, wait for
 * the quota to reset, try again shortly, or check the connection. UIs show a
 * card for each instead of the raw error.
 *
 * Engines with the `error_details` feature send a `detail` on the `error`
 * event (`code`, `status`, `upgrade_url`, `resets_at`, `retry_after_seconds`).
 * Older ones send only the message, so known phrases are recognised too.
 */

import type { ErrorDetail } from './protocol'

export type GatewayProblemKind = 'sign_in' | 'upgrade' | 'quota' | 'busy' | 'unreachable'

export interface GatewayProblem {
  kind: GatewayProblemKind
  /** The gateway's own message, for a detail line. */
  message: string
  upgradeUrl: string | null
  /** When the quota resets, as an ISO time. */
  resetsAt: string | null
  retryAfterSeconds: number | null
}

/** The fields an `error` event carries. */
export interface ErrorFields {
  message: string
  detail?: ErrorDetail | null
}

const BUSY_CODES = new Set(['rate_limited', 'capacity_exhausted', 'upstream_error', 'upstream_timeout', 'service_unavailable'])
const SIGN_IN_CODES = new Set(['invalid_session', 'unauthorized', 'session_expired', 'sign_in_required'])
/** The gateway could not be reached, or does not offer the call yet (a 404 from an older gateway). */
const UNREACHABLE_CODES = new Set(['unreachable', 'not_available', 'network'])
const QUOTA_CODES = new Set(['insufficient_quota', 'insufficient_credits'])

const text = (value: unknown) => (typeof value === 'string' && value.trim() ? value.trim() : null)
const number = (value: unknown) => (typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null)

function kindOf(code: string | null, status: number | null, message: string): GatewayProblemKind | null {
  if (code) {
    if (SIGN_IN_CODES.has(code)) return 'sign_in'
    if (code === 'upgrade_required') return 'upgrade'
    if (QUOTA_CODES.has(code)) return 'quota'
    if (BUSY_CODES.has(code)) return 'busy'
    if (UNREACHABLE_CODES.has(code)) return 'unreachable'
  }
  if (status === 401) return 'sign_in'
  if (status === 503) return 'busy'
  const lower = message.toLowerCase()
  // The signed-out stage's own reminder is not a gateway failure.
  if (lower.startsWith('sign in to zero-x first')) return null
  if (lower.includes('upgrade_required')) return 'upgrade'
  if (lower.includes('insufficient_quota') || lower.includes('insufficient_credits')) return 'quota'
  if (lower.includes('invalid_session') || /\b401\b/.test(lower) || lower.includes('sign in again')) return 'sign_in'
  if (
    lower.includes('could not reach zero-x') ||
    lower.includes('does not support device sign-in') ||
    lower.includes('error sending request') ||
    lower.includes('connection refused') ||
    lower.includes('dns error') ||
    lower.includes('failed to lookup address')
  ) {
    return 'unreachable'
  }
  if (lower.includes('service_unavailable') || lower.includes('capacity_exhausted') || lower.includes('rate_limited') || /\b503\b/.test(lower)) {
    return 'busy'
  }
  return null
}

/** The problem behind an `error` event, or `null` for an error that is not about the gateway. */
export function gatewayProblem(error: ErrorFields): GatewayProblem | null {
  const detail = error.detail ?? null
  const kind = kindOf(text(detail?.code), number(detail?.status), error.message)
  if (!kind) return null
  const seconds = number(detail?.retry_after_seconds) ?? (kind === 'busy' ? retryHint(error.message) : null)
  return {
    kind,
    message: error.message,
    upgradeUrl: text(detail?.upgrade_url),
    resetsAt: text(detail?.resets_at),
    retryAfterSeconds: seconds,
  }
}

/** "try again in 30s" / "retry after 12 seconds" in an older engine's message. */
function retryHint(message: string): number | null {
  const match = /(?:try again|retry)[^0-9]{0,12}(\d+)\s*(s|sec|second)/i.exec(message)
  return match ? Number(match[1]) : null
}
