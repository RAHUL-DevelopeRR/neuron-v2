/**
 * When to (re)connect the relay socket, as a pure state machine: events in,
 * a new state and effects out. The link (./link.ts) runs the effects; tests
 * drive the machine directly.
 *
 * - Backoff is exponential with full jitter, capped, and starts over once a
 *   connection stayed up for `stableMs`. A connection that drops right away
 *   keeps counting, so a flapping network does not hammer the relay.
 * - Coming online or back to the foreground retries at once.
 * - Close codes that reconnecting cannot fix (a revoked session, a protocol
 *   error, this device joining elsewhere) stop it: the user has to act.
 * - Once open, the link resumes its channels from the last `seq` it got.
 */

import { CloseCode, FATAL_CLOSE_CODES } from './contract'

export type LinkStatus =
  /** Not started, or stopped by the user. */
  | 'idle'
  | 'connecting'
  | 'open'
  /** Waiting out a backoff delay before the next attempt. */
  | 'waiting'
  /** The device is offline; waiting for it to come back. */
  | 'offline'
  /** Stopped for good until the user acts (signed out, replaced). */
  | 'failed'

export interface ReconnectState {
  status: LinkStatus
  /** Failed attempts since the last stable connection. */
  attempt: number
  /** The delay being waited out, while `waiting`. */
  delayMs: number | null
  lastCloseCode: number | null
  lastError: string | null
  /** When the current connection opened (ms), while `open`. */
  openedAt: number | null
  online: boolean
}

export type ReconnectEvent =
  | { type: 'start' }
  | { type: 'opened'; at: number }
  | { type: 'closed'; code: number | null; reason?: string; at: number }
  | { type: 'timer' }
  | { type: 'online' }
  | { type: 'offline' }
  | { type: 'foreground' }
  | { type: 'stop' }

export type ReconnectEffect =
  | { type: 'connect' }
  | { type: 'schedule'; ms: number }
  | { type: 'cancel_timer' }
  /** Resume the link's channels (and list hosts) on the fresh socket. */
  | { type: 'resume' }
  | { type: 'disconnect' }

export interface BackoffOptions {
  baseMs: number
  maxMs: number
  factor: number
  /** A connection up this long resets the attempt count. */
  stableMs: number
  /** Uniform in [0, 1); injectable for tests. */
  random: () => number
}

export const DEFAULT_BACKOFF: BackoffOptions = {
  baseMs: 500,
  maxMs: 30_000,
  factor: 2,
  stableMs: 10_000,
  random: Math.random,
}

export const initialReconnect = (online = true): ReconnectState => ({
  status: 'idle',
  attempt: 0,
  delayMs: null,
  lastCloseCode: null,
  lastError: null,
  openedAt: null,
  online,
})

/** The delay before attempt `attempt` (0-based): full jitter over an exponential, capped window. */
export function backoffDelay(attempt: number, options: BackoffOptions): number {
  const window = Math.min(options.maxMs, options.baseMs * options.factor ** Math.max(0, attempt))
  // Never zero: at least a tenth of the base, so a tight loop is impossible.
  return Math.max(Math.round(options.baseMs / 10), Math.round(window * options.random()))
}

const FATAL_MESSAGES: Record<number, string> = {
  [CloseCode.unauthorized]: 'Your Zero-X sign-in expired. Sign in again.',
  [CloseCode.forbidden]: 'This account cannot use the relay.',
  [CloseCode.protocol]: 'The relay and this app disagree on the protocol. Update the app.',
  [CloseCode.replaced]: 'This phone connected to the relay from somewhere else.',
}

export function stepReconnect(
  state: ReconnectState,
  event: ReconnectEvent,
  options: BackoffOptions = DEFAULT_BACKOFF,
): { state: ReconnectState; effects: ReconnectEffect[] } {
  const same = { state, effects: [] as ReconnectEffect[] }
  switch (event.type) {
    case 'start':
      if (state.status === 'connecting' || state.status === 'open') return same
      if (!state.online) return { state: { ...state, status: 'offline', delayMs: null }, effects: [{ type: 'cancel_timer' }] }
      return {
        state: { ...state, status: 'connecting', delayMs: null, lastError: null, attempt: state.status === 'failed' ? 0 : state.attempt },
        effects: [{ type: 'cancel_timer' }, { type: 'connect' }],
      }

    case 'opened':
      if (state.status !== 'connecting') return same
      return { state: { ...state, status: 'open', openedAt: event.at, delayMs: null, lastCloseCode: null, lastError: null }, effects: [{ type: 'resume' }] }

    case 'closed': {
      if (state.status === 'idle' || state.status === 'failed') return same
      const code = event.code
      if (code !== null && FATAL_CLOSE_CODES.has(code)) {
        return {
          state: { ...state, status: 'failed', delayMs: null, openedAt: null, lastCloseCode: code, lastError: FATAL_MESSAGES[code] ?? event.reason ?? null },
          effects: [{ type: 'cancel_timer' }],
        }
      }
      const stable = state.status === 'open' && state.openedAt !== null && event.at - state.openedAt >= options.stableMs
      const attempt = stable ? 0 : state.attempt
      const base = { ...state, openedAt: null, lastCloseCode: code, lastError: event.reason || null }
      if (!state.online) return { state: { ...base, status: 'offline', attempt, delayMs: null }, effects: [] }
      // Over a limit: wait the longest delay.
      const delayMs = code === CloseCode.limited ? options.maxMs : backoffDelay(attempt, options)
      return { state: { ...base, status: 'waiting', attempt: attempt + 1, delayMs }, effects: [{ type: 'schedule', ms: delayMs }] }
    }

    case 'timer':
      if (state.status !== 'waiting') return same
      return { state: { ...state, status: 'connecting', delayMs: null }, effects: [{ type: 'connect' }] }

    case 'online':
      if (state.online && state.status !== 'offline') return same
      if (state.status === 'offline' || state.status === 'waiting') {
        return { state: { ...state, online: true, status: 'connecting', delayMs: null }, effects: [{ type: 'cancel_timer' }, { type: 'connect' }] }
      }
      return { state: { ...state, online: true }, effects: [] }

    case 'offline':
      if (state.status === 'waiting') return { state: { ...state, online: false, status: 'offline', delayMs: null }, effects: [{ type: 'cancel_timer' }] }
      return { state: { ...state, online: false }, effects: [] }

    case 'foreground':
      // Back from the background: try now instead of waiting out the backoff.
      if (state.status !== 'waiting') return same
      return { state: { ...state, status: 'connecting', delayMs: null }, effects: [{ type: 'cancel_timer' }, { type: 'connect' }] }

    case 'stop':
      if (state.status === 'idle') return same
      return { state: { ...initialReconnect(state.online) }, effects: [{ type: 'cancel_timer' }, { type: 'disconnect' }] }
  }
}
