/**
 * The relay protocol, `neuron-relay/1` (docs/RELAY_CONTRACT.md §4): JSON text
 * frames over one WebSocket per device. The relay sees these envelopes and
 * routes them; what a session says travels sealed in `data.c` (remote protocol
 * 1 frames from @neuron/client, end-to-end encrypted by ./crypto).
 *
 * The zero-x.live relay (Durable Object, WP5), `neuron engine --share` (WP1)
 * and the dev relay in ../../dev all speak exactly this. Binary fields are
 * base64url without padding.
 */

export const RELAY_PROTOCOL = 1
/** Offered as a WebSocket subprotocol; the relay answers with it. */
export const RELAY_SUBPROTOCOL = 'neuron-relay.v1'
/** A second subprotocol carries the session token where a client cannot set headers (browsers). */
export const AUTH_SUBPROTOCOL_PREFIX = 'neuron-auth.'

export type Role = 'host' | 'phone'

/** WebSocket close codes the relay uses. */
export const CloseCode = {
  /** The first frame was not a valid `join`, or a frame broke the protocol. */
  protocol: 4400,
  /** No session token, or it is expired or revoked: sign in again. */
  unauthorized: 4401,
  /** The credential may not use the relay (`nzx_` API keys), or the account is suspended. */
  forbidden: 4403,
  /** No `join` in time. */
  joinTimeout: 4408,
  /** The same device joined again elsewhere; this socket is replaced. */
  replaced: 4409,
  /** Over a per-account limit (devices, frame rate, frame size). */
  limited: 4429,
} as const

/** Close codes after which reconnecting cannot help until the user acts. */
export const FATAL_CLOSE_CODES: ReadonlySet<number> = new Set([CloseCode.protocol, CloseCode.unauthorized, CloseCode.forbidden, CloseCode.replaced])

export interface DeviceInfo {
  /** Stable per install (phone) or per computer (host): 8–64 of `[A-Za-z0-9_-]`. */
  id: string
  name: string
  platform: string
}

export type NotifyKind = 'needs_me' | 'finished'

export interface ShareListing {
  share_id: string
  /** Presence, sealed with the computer's presence key; `null` until it sends one. */
  sealed: string | null
  /** When the relay last heard about it (ms since the epoch). */
  at: number
}

export interface HostListing {
  host_id: string
  name: string
  online: boolean
  shares: ShareListing[]
}

/** Frames any device sends. */
export type CommonFrame = { t: 'join'; role: Role; device: DeviceInfo } | { t: 'ping' }

/** Frames the computer sends. */
export type HostToRelay =
  | { t: 'share'; share_id: string; sealed: string | null }
  | { t: 'unshare'; share_id: string }
  | { t: 'pair_offer'; pair_id: string; ttl_s: number }
  | { t: 'pair_cancel'; pair_id: string }
  | { t: 'paired'; pair_id: string; phone_id: string; proof: string; sealed: string }
  | { t: 'pair_denied'; pair_id: string; phone_id: string; code: string }
  | { t: 'accept'; ch: string; e: string; confirm: string }
  | { t: 'reject'; ch: string; code: string }
  | { t: 'data'; ch: string; n: number; c: string }
  | { t: 'close'; ch: string; code?: string }
  | { t: 'notify'; share_id: string; kind: NotifyKind }

/** Frames the phone sends. */
export type PhoneToRelay =
  | { t: 'list' }
  | { t: 'pair'; pair_id: string; pub: string; proof: string; name: string }
  | { t: 'open'; ch: string; host_id: string; share_id: string; e: string }
  | { t: 'resume'; ch: string; after: number }
  | { t: 'data'; ch: string; n: number; c: string }
  | { t: 'close'; ch: string; code?: string }
  | { t: 'unpair'; host_id: string }

/** Frames the relay sends to either. */
export type RelayCommon =
  | { t: 'joined'; relay: number; role: Role; device_id: string; account: { tag: string } }
  | { t: 'pong' }
  | { t: 'error'; code: string; message: string; ch?: string; pair_id?: string }

/** Frames the relay sends to a computer. */
export type RelayToHost =
  | RelayCommon
  | { t: 'pair_offer_ok'; pair_id: string; expires_at: number }
  | { t: 'pair'; pair_id: string; phone_id: string; pub: string; proof: string; name: string }
  | { t: 'open'; ch: string; phone_id: string; share_id: string; e: string }
  | { t: 'data'; ch: string; n: number; c: string }
  | { t: 'close'; ch: string; code?: string }
  | { t: 'peer'; ch: string; state: 'away' | 'back' }
  | { t: 'unpaired'; phone_id: string }

/** Frames the relay sends to a phone. */
export type RelayToPhone =
  | RelayCommon
  | { t: 'hosts'; hosts: HostListing[] }
  | { t: 'host'; host_id: string; name: string; online: boolean }
  | { t: 'presence'; host_id: string; share_id: string; sealed: string | null; at: number }
  | { t: 'paired'; pair_id: string; host_id: string; name: string; proof: string; sealed: string }
  | { t: 'pair_denied'; pair_id: string; code: string }
  | { t: 'accept'; ch: string; e: string; confirm: string }
  | { t: 'reject'; ch: string; code: string }
  | { t: 'data'; ch: string; n: number; c: string }
  | { t: 'close'; ch: string; code?: string }
  /** Answer to `resume`: replayed `data` follows. `received` is the last phone counter the computer got. */
  | { t: 'resumed'; ch: string; received: number }
  | { t: 'resume_failed'; ch: string; code: string }
  | { t: 'unpaired'; host_id: string }

/** Inside `paired.sealed` (sealed with the pair key's `pair-secrets` subkey). */
export interface PairSecrets {
  /** Opens the computer's presence blobs: 32 bytes, base64url. */
  presence_key: string
  name: string
}

export type ShareStatus = 'running' | 'needs_me' | 'idle' | 'failed'

/** Inside a presence blob: what the sessions list shows for one shared session. */
export interface Presence {
  v: 1
  /** The workspace folder's name. */
  project: string
  title: string | null
  branch: string | null
  model: string | null
  status: ShareStatus
  /** The remote protocol `seq` of the session's latest action: the phone compares it for "unread". */
  last_seq: number
  updated_at_ms: number
}

/** Size and rate limits a relay enforces (the dev relay uses these too). */
export const LIMITS = {
  maxFrameBytes: 1_000_000,
  maxHostsPerAccount: 3,
  maxPhonesPerAccount: 5,
  maxSharesPerHost: 16,
  /** Frames a relay keeps per channel for `resume`. */
  replayFrames: 512,
  replayBytes: 4_000_000,
  /** How long a channel waits for its phone to come back. */
  channelAwayMs: 10 * 60_000,
  pairTtlMaxS: 600,
  joinTimeoutMs: 10_000,
  /** A pushed approval waits this long for a phone to attach first. */
  needsMeDelayMs: 30_000,
} as const

const ID = /^[A-Za-z0-9_-]{8,64}$/
const isId = (value: unknown): value is string => typeof value === 'string' && ID.test(value)
const isB64 = (value: unknown, max = 2_000_000): value is string => typeof value === 'string' && value.length <= max && /^[A-Za-z0-9_-]*$/.test(value)
const isCounter = (value: unknown): value is number => Number.isSafeInteger(value) && (value as number) >= 0
const text = (value: unknown, max: number): string | null =>
  typeof value === 'string' ? value.replace(/[\u0000-\u001f\u007f-\u009f]/g, '').trim().slice(0, max) : null

function parseObject(data: unknown): Record<string, unknown> | null {
  if (typeof data !== 'string' || data.length > LIMITS.maxFrameBytes) return null
  try {
    const value = JSON.parse(data)
    return value && typeof value === 'object' && !Array.isArray(value) && typeof value.t === 'string' ? value : null
  } catch {
    return null
  }
}

/**
 * Checks a frame a device sent to the relay; `null` when it is not one the
 * role may send. Used by relays (and by the dev relay), never trusted blindly.
 */
export function parseDeviceFrame(data: unknown, role: Role | null): CommonFrame | HostToRelay | PhoneToRelay | null {
  const f = parseObject(data)
  if (!f) return null
  if (f.t === 'ping') return { t: 'ping' }
  if (f.t === 'join') {
    const device = (f.device ?? {}) as Record<string, unknown>
    if ((f.role !== 'host' && f.role !== 'phone') || !isId(device.id)) return null
    return { t: 'join', role: f.role, device: { id: device.id, name: text(device.name, 64) || 'device', platform: text(device.platform, 32) || 'unknown' } }
  }
  if (f.t === 'data') return isId(f.ch) && isCounter(f.n) && isB64(f.c) ? { t: 'data', ch: f.ch, n: f.n, c: f.c } : null
  if (f.t === 'close') return isId(f.ch) ? { t: 'close', ch: f.ch, ...(typeof f.code === 'string' ? { code: f.code.slice(0, 32) } : {}) } : null
  if (role === 'host') {
    switch (f.t) {
      case 'share':
        return isId(f.share_id) && (f.sealed === null || isB64(f.sealed, 8_192)) ? { t: 'share', share_id: f.share_id, sealed: (f.sealed as string | null) ?? null } : null
      case 'unshare':
        return isId(f.share_id) ? { t: 'unshare', share_id: f.share_id } : null
      case 'pair_offer':
        return isB64(f.pair_id, 64) && Number.isSafeInteger(f.ttl_s) && (f.ttl_s as number) > 0
          ? { t: 'pair_offer', pair_id: f.pair_id, ttl_s: Math.min(f.ttl_s as number, LIMITS.pairTtlMaxS) }
          : null
      case 'pair_cancel':
        return isB64(f.pair_id, 64) ? { t: 'pair_cancel', pair_id: f.pair_id } : null
      case 'paired':
        return isB64(f.pair_id, 64) && isId(f.phone_id) && isB64(f.proof, 64) && isB64(f.sealed, 4_096)
          ? { t: 'paired', pair_id: f.pair_id, phone_id: f.phone_id, proof: f.proof, sealed: f.sealed }
          : null
      case 'pair_denied':
        return isB64(f.pair_id, 64) && isId(f.phone_id) && typeof f.code === 'string'
          ? { t: 'pair_denied', pair_id: f.pair_id, phone_id: f.phone_id, code: f.code.slice(0, 32) }
          : null
      case 'accept':
        return isId(f.ch) && isB64(f.e, 64) && isB64(f.confirm, 64) ? { t: 'accept', ch: f.ch, e: f.e, confirm: f.confirm } : null
      case 'reject':
        return isId(f.ch) && typeof f.code === 'string' ? { t: 'reject', ch: f.ch, code: f.code.slice(0, 32) } : null
      case 'notify':
        return isId(f.share_id) && (f.kind === 'needs_me' || f.kind === 'finished') ? { t: 'notify', share_id: f.share_id, kind: f.kind } : null
      default:
        return null
    }
  }
  if (role === 'phone') {
    switch (f.t) {
      case 'list':
        return { t: 'list' }
      case 'pair':
        return isB64(f.pair_id, 64) && isB64(f.pub, 64) && isB64(f.proof, 64)
          ? { t: 'pair', pair_id: f.pair_id, pub: f.pub, proof: f.proof, name: text(f.name, 64) || 'phone' }
          : null
      case 'open':
        return isId(f.ch) && isId(f.host_id) && isId(f.share_id) && isB64(f.e, 64)
          ? { t: 'open', ch: f.ch, host_id: f.host_id, share_id: f.share_id, e: f.e }
          : null
      case 'resume':
        return isId(f.ch) && Number.isSafeInteger(f.after) && (f.after as number) >= -1 ? { t: 'resume', ch: f.ch, after: f.after as number } : null
      case 'unpair':
        return isId(f.host_id) ? { t: 'unpair', host_id: f.host_id } : null
      default:
        return null
    }
  }
  return null
}

/** Reads a frame from the relay. Shapes are checked where the phone acts on them. */
export function parseRelayFrame<T extends { t: string }>(data: unknown): T | null {
  return parseObject(data) as T | null
}

export const isRelayId = isId
