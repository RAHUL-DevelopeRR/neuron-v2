/**
 * The pairing link in the QR code that *Continue on phone* (desktop) and
 * `/phone` (terminal) show (docs/RELAY_CONTRACT.md §2):
 *
 *   neuron://pair?v=1&r=<relay URL>&h=<host id>&c=<one-time code>&k=<host key>&n=<name>&u=<account tag>&e=<expiry>
 *
 * The https form (`https://neuron.zero-x.live/pair#v=1&...`) carries the same
 * fields in the fragment, which browsers never send to a server. The parser
 * reads untrusted input (anything a camera sees), so it checks every field
 * and says what is wrong in words the pairing screen can show.
 */

import { b64u, fromB64u } from './crypto'

export const PAIRING_VERSION = '1'

export interface PairingPayload {
  /** The relay's WebSocket URL: `wss://relay.zero-x.live/v1/relay`. */
  relayUrl: string
  /** The computer's id on the relay. */
  hostId: string
  /** The one-time code: 16 bytes, base64url. Never sent to the relay. */
  code: string
  /** The computer's X25519 public key: 32 bytes, base64url. */
  hostKey: string
  /** The computer's name, as it reported it. */
  name: string | null
  /** A short tag of the Zero-X account the computer is signed in to. */
  accountTag: string | null
  /** When the code stops working (ms since the epoch). */
  expiresAt: number | null
}

export type PairingErrorCode =
  | 'not_a_pairing_link'
  | 'unsupported_version'
  | 'missing_field'
  | 'bad_relay'
  | 'insecure_relay'
  | 'untrusted_relay'
  | 'bad_host_id'
  | 'bad_code'
  | 'bad_key'
  | 'bad_name'
  | 'bad_account'
  | 'expired'

export class PairingError extends Error {
  constructor(
    readonly code: PairingErrorCode,
    message: string,
  ) {
    super(message)
    this.name = 'PairingError'
  }
}

export interface ParseOptions {
  /** Now, in ms (tests pass a fixed clock). */
  now?: number
  /**
   * Relay hosts a store build accepts. Development builds pass `null` to accept
   * any relay, so a phone can pair with a relay on the developer's machine.
   */
  trustedRelayHosts?: readonly string[] | null
}

/** Relays a store build talks to: Zero-X's own. */
export const TRUSTED_RELAY_HOSTS: readonly string[] = ['relay.zero-x.live', 'zero-x.live']

const HOST_ID = /^[A-Za-z0-9_-]{8,64}$/
const ACCOUNT_TAG = /^[0-9a-f]{8,32}$/
const MAX_LINK_LENGTH = 2048

/** Loopback and private (RFC 1918, link-local, `.local`) hosts: plain `ws://` is allowed there for development. */
export function isPrivateHost(hostname: string): boolean {
  const host = hostname.replace(/^\[|\]$/g, '').toLowerCase()
  if (host === 'localhost' || host.endsWith('.localhost') || host.endsWith('.local') || host === '::1') return true
  const v4 = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(host)
  if (!v4) return /^f[cd][0-9a-f]{2}:/.test(host) || host.startsWith('fe80:')
  const [a, b] = [Number(v4[1]), Number(v4[2])]
  return a === 127 || a === 10 || (a === 192 && b === 168) || (a === 172 && b >= 16 && b <= 31) || (a === 169 && b === 254)
}

function hostTrusted(hostname: string, trusted: readonly string[]): boolean {
  const host = hostname.toLowerCase()
  return trusted.some((allowed) => host === allowed || host.endsWith(`.${allowed}`))
}

/** The fields of a pairing link: from the query (`neuron://pair?...`) or the fragment (`https://.../pair#...`). */
function fieldsOf(input: string): URLSearchParams {
  const text = input.trim()
  if (!text) throw new PairingError('not_a_pairing_link', 'Scan the QR code your computer shows, or paste its pairing link.')
  if (text.length > MAX_LINK_LENGTH) throw new PairingError('not_a_pairing_link', 'That is too long to be a Neuron pairing link.')
  const scheme = /^neuron:\/\/pair\/?([?#].*)?$/i.exec(text)
  if (scheme) return new URLSearchParams((scheme[1] ?? '').slice(1))
  let url: URL
  try {
    url = new URL(text)
  } catch {
    throw new PairingError('not_a_pairing_link', 'That is not a Neuron pairing link.')
  }
  if (url.protocol !== 'https:' || !/\/pair\/?$/.test(url.pathname)) {
    throw new PairingError('not_a_pairing_link', 'That is not a Neuron pairing link.')
  }
  return new URLSearchParams(url.hash.slice(1) || url.search.slice(1))
}

function required(fields: URLSearchParams, key: string, what: string): string {
  const value = fields.get(key)
  if (value === null || value === '') throw new PairingError('missing_field', `The pairing link has no ${what}. Show a new code on the computer.`)
  return value
}

/** Reads a pairing link, or throws a {@link PairingError} saying what is wrong with it. */
export function parsePairingLink(input: string, options: ParseOptions = {}): PairingPayload {
  const fields = fieldsOf(input)
  const version = fields.get('v')
  if (version !== PAIRING_VERSION) {
    throw new PairingError(
      'unsupported_version',
      version ? 'This pairing code is from a newer Neuron. Update the app, then scan it again.' : 'That is not a Neuron pairing link.',
    )
  }

  const relayText = required(fields, 'r', 'relay address')
  let relay: URL
  try {
    relay = new URL(relayText)
  } catch {
    throw new PairingError('bad_relay', 'The relay address in the pairing link is not an address.')
  }
  if (relay.protocol !== 'wss:' && relay.protocol !== 'ws:') throw new PairingError('bad_relay', 'The relay address must be a WebSocket address.')
  if (relay.username || relay.password || relay.search || relay.hash) {
    throw new PairingError('bad_relay', 'The relay address must not carry credentials, a query or a fragment.')
  }
  if (relay.protocol === 'ws:' && !isPrivateHost(relay.hostname)) {
    throw new PairingError('insecure_relay', 'The relay address is not encrypted (ws://). Only wss:// relays are used outside your own network.')
  }
  const trusted = options.trustedRelayHosts === undefined ? TRUSTED_RELAY_HOSTS : options.trustedRelayHosts
  if (trusted && !hostTrusted(relay.hostname, trusted)) {
    throw new PairingError('untrusted_relay', `This app only pairs through Zero-X's relay, not ${relay.hostname}.`)
  }

  const hostId = required(fields, 'h', 'computer id')
  if (!HOST_ID.test(hostId)) throw new PairingError('bad_host_id', 'The computer id in the pairing link is not valid.')

  const code = required(fields, 'c', 'pairing code')
  const codeBytes = fromB64u(code)
  if (!codeBytes || codeBytes.length !== 16) throw new PairingError('bad_code', 'The pairing code is damaged. Show a new code on the computer.')

  const hostKey = required(fields, 'k', 'computer key')
  const keyBytes = fromB64u(hostKey)
  if (!keyBytes || keyBytes.length !== 32) throw new PairingError('bad_key', "The computer's key in the pairing link is damaged.")

  const rawName = fields.get('n')
  let name: string | null = null
  if (rawName !== null) {
    name = rawName.replace(/[\u0000-\u001f\u007f-\u009f]/g, '').trim()
    if (name.length > 64) throw new PairingError('bad_name', 'The computer name in the pairing link is too long.')
    if (!name) name = null
  }

  const accountTag = fields.get('u')
  if (accountTag !== null && !ACCOUNT_TAG.test(accountTag)) throw new PairingError('bad_account', 'The account tag in the pairing link is not valid.')

  const expiry = fields.get('e')
  let expiresAt: number | null = null
  if (expiry !== null) {
    if (!/^\d{1,12}$/.test(expiry)) throw new PairingError('expired', 'The pairing code has a bad expiry. Show a new code on the computer.')
    expiresAt = Number(expiry) * 1000
    if (expiresAt <= (options.now ?? Date.now())) throw new PairingError('expired', 'This pairing code has expired. Show a new one on the computer.')
  }

  // Normalised, so the same relay is the same string (no trailing slash variants).
  const relayUrl = `${relay.protocol}//${relay.host}${relay.pathname === '/' ? '' : relay.pathname.replace(/\/+$/, '')}`
  // Re-encode both keys so equal bytes always spell the same.
  return { relayUrl, hostId, code: b64u(codeBytes), hostKey: b64u(keyBytes), name, accountTag, expiresAt }
}

/** Builds the `neuron://pair` link (what the computer puts in the QR code). */
export function buildPairingLink(payload: PairingPayload): string {
  const fields = new URLSearchParams({ v: PAIRING_VERSION, r: payload.relayUrl, h: payload.hostId, c: payload.code, k: payload.hostKey })
  if (payload.name) fields.set('n', payload.name)
  if (payload.accountTag) fields.set('u', payload.accountTag)
  if (payload.expiresAt !== null) fields.set('e', String(Math.floor(payload.expiresAt / 1000)))
  return `neuron://pair?${fields.toString()}`
}

/** The same link in its https form, for phones that open it from the camera app. */
export function buildPairingWebLink(payload: PairingPayload, base = 'https://neuron.zero-x.live/pair'): string {
  return `${base}#${buildPairingLink(payload).slice('neuron://pair?'.length)}`
}
