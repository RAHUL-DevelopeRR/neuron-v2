/**
 * The relay's end-to-end encryption (docs/RELAY_CONTRACT.md §3). The relay
 * routes frames between the phone and the computer and never holds a key:
 *
 * - Pairing: the QR code carries the computer's X25519 public key and a
 *   one-time code. The phone makes a key pair, and both ends derive the pair
 *   key from the X25519 secret with HKDF-SHA256, salted with the code. The
 *   relay only ever sees a hash of the code (the pair id), so it cannot pair
 *   in the phone's place.
 * - Channels: each attach makes fresh X25519 keys on both ends, and the pair
 *   key salts the HKDF, so a relay that swaps the keys derives nothing useful
 *   and the first frame fails to open.
 * - Frames: AES-256-GCM, one key per direction, the nonce is the frame's
 *   counter, and the channel id and direction are the associated data.
 *
 * Pure TypeScript (noble), so the phone (Hermes), the web build, Bun and the
 * tests all run the same code. Random bytes come from `crypto.getRandomValues`;
 * the app installs it from expo-crypto where the runtime has none.
 */

import { gcm } from '@noble/ciphers/aes.js'
import { x25519 } from '@noble/curves/ed25519.js'
import { hkdf } from '@noble/hashes/hkdf.js'
import { hmac } from '@noble/hashes/hmac.js'
import { sha256 } from '@noble/hashes/sha2.js'

export const LABEL = 'neuron-relay/1'

/** Which way a frame goes: phone to computer, or computer to phone. */
export type Direction = 'p2h' | 'h2p'

export class E2EError extends Error {
  constructor(
    readonly code: 'bad_frame' | 'bad_key' | 'bad_proof' | 'bad_confirm',
    message: string,
  ) {
    super(message)
    this.name = 'E2EError'
  }
}

const encoder = new TextEncoder()
const decoder = new TextDecoder('utf-8', { fatal: true })

export const utf8 = (text: string): Uint8Array => encoder.encode(text)

function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((total, part) => total + part.length, 0))
  let offset = 0
  for (const part of parts) {
    out.set(part, offset)
    offset += part.length
  }
  return out
}

// ------------------------------------------------------------------ base64url

const ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_'
const LOOKUP = new Map([...ALPHABET].map((char, index) => [char, index]))

/** base64url without padding (RFC 4648 §5). */
export function b64u(bytes: Uint8Array): string {
  let out = ''
  let index = 0
  for (; index + 2 < bytes.length; index += 3) {
    const n = (bytes[index] << 16) | (bytes[index + 1] << 8) | bytes[index + 2]
    out += ALPHABET[n >> 18] + ALPHABET[(n >> 12) & 63] + ALPHABET[(n >> 6) & 63] + ALPHABET[n & 63]
  }
  const rest = bytes.length - index
  if (rest === 1) {
    const n = bytes[index] << 16
    out += ALPHABET[n >> 18] + ALPHABET[(n >> 12) & 63]
  } else if (rest === 2) {
    const n = (bytes[index] << 16) | (bytes[index + 1] << 8)
    out += ALPHABET[n >> 18] + ALPHABET[(n >> 12) & 63] + ALPHABET[(n >> 6) & 63]
  }
  return out
}

/** Decodes base64url without padding; `null` when it is not that. */
export function fromB64u(text: string): Uint8Array | null {
  if (typeof text !== 'string' || text.length % 4 === 1) return null
  const out = new Uint8Array(Math.floor((text.length * 3) / 4))
  let bits = 0
  let value = 0
  let offset = 0
  for (const char of text) {
    const digit = LOOKUP.get(char)
    if (digit === undefined) return null
    value = (value << 6) | digit
    bits += 6
    if (bits >= 8) {
      bits -= 8
      out[offset++] = (value >> bits) & 255
    }
  }
  // Leftover bits must be zero, so every byte string has one spelling.
  if ((value & ((1 << bits) - 1)) !== 0) return null
  return out
}

/** Decodes base64url of exactly `length` bytes, or throws `bad_key`. */
export function bytesOf(text: string, length: number, what: string): Uint8Array {
  const bytes = fromB64u(text)
  if (!bytes || bytes.length !== length) throw new E2EError('bad_key', `${what} is not ${length} bytes of base64url.`)
  return bytes
}

// --------------------------------------------------------------- randomness

type RandomSource = (length: number) => Uint8Array

let randomSource: RandomSource = (length) => {
  const crypto = (globalThis as { crypto?: { getRandomValues?: (array: Uint8Array) => Uint8Array } }).crypto
  if (typeof crypto?.getRandomValues !== 'function') throw new Error('No secure random source: crypto.getRandomValues is missing.')
  return crypto.getRandomValues(new Uint8Array(length))
}

/** Replaces the random source (tests use a fixed one for test vectors). */
export function setRandomSource(source: RandomSource): RandomSource {
  const previous = randomSource
  randomSource = source
  return previous
}

export const randomBytes = (length: number): Uint8Array => randomSource(length)

/** A random id: 16 bytes of base64url (22 characters). */
export const randomId = (): string => b64u(randomBytes(16))

// ------------------------------------------------------------------- keys

export interface KeyPair {
  secretKey: Uint8Array
  publicKey: Uint8Array
}

export function keyPair(): KeyPair {
  const secretKey = randomBytes(32)
  return { secretKey, publicKey: x25519.getPublicKey(secretKey) }
}

export const publicKeyOf = (secretKey: Uint8Array): Uint8Array => x25519.getPublicKey(secretKey)

function sharedSecret(secretKey: Uint8Array, peerPublic: Uint8Array): Uint8Array {
  let shared: Uint8Array
  try {
    shared = x25519.getSharedSecret(secretKey, peerPublic)
  } catch {
    throw new E2EError('bad_key', 'The other end sent a key that is not an X25519 public key.')
  }
  // A small-order point gives all zeros: refuse it rather than derive from nothing.
  if (shared.every((byte) => byte === 0)) throw new E2EError('bad_key', 'The other end sent a weak key.')
  return shared
}

function constantTimeEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false
  let diff = 0
  for (let index = 0; index < a.length; index++) diff |= a[index] ^ b[index]
  return diff === 0
}

// ---------------------------------------------------------------- pairing

/** The one-time code in the QR: 16 random bytes. */
export const newPairingCode = (): string => b64u(randomBytes(16))

/** What the relay routes a pairing by: a hash of the code, so the relay never learns it. */
export function pairIdFor(code: string): string {
  const bytes = bytesOf(code, 16, 'The pairing code')
  return b64u(sha256(concat(utf8(`${LABEL} pair-id`), bytes)).slice(0, 16))
}

/**
 * The key a phone and a computer share once paired. Each end passes its own
 * secret key and the other's public key; `hostPublic` and `phonePublic` are
 * the same on both ends, so both derive the same 32 bytes.
 */
export function derivePairKey(options: {
  secretKey: Uint8Array
  peerPublic: Uint8Array
  code: string
  hostPublic: Uint8Array
  phonePublic: Uint8Array
}): Uint8Array {
  const shared = sharedSecret(options.secretKey, options.peerPublic)
  const salt = bytesOf(options.code, 16, 'The pairing code')
  return hkdf(sha256, shared, salt, concat(utf8(`${LABEL} pair`), options.hostPublic, options.phonePublic), 32)
}

/** Proves knowing the pair key (so the code): 16 bytes of HMAC, base64url. */
export function pairProof(pairKey: Uint8Array, role: 'phone' | 'host', pairId: string): string {
  return b64u(hmac(sha256, pairKey, utf8(`${LABEL} ${role}-proof ${pairId}`)).slice(0, 16))
}

export function proofMatches(expected: string, given: unknown): boolean {
  if (typeof given !== 'string') return false
  const a = fromB64u(expected)
  const b = fromB64u(given)
  return !!a && !!b && constantTimeEqual(a, b)
}

/** A key for one purpose, from the pair key (`pair-secrets` seals what the computer sends at pairing). */
export const subkey = (pairKey: Uint8Array, purpose: string): Uint8Array =>
  hkdf(sha256, pairKey, undefined, utf8(`${LABEL} ${purpose}`), 32)

// --------------------------------------------------------------- channels

export interface ChannelKeys {
  /** Seals what the phone sends. */
  p2h: Uint8Array
  /** Seals what the computer sends. */
  h2p: Uint8Array
}

/**
 * The keys of one channel (one attach of a phone to a shared session). Both
 * ends pass their own ephemeral secret and the other's ephemeral public key.
 */
export function deriveChannelKeys(options: {
  secretKey: Uint8Array
  peerPublic: Uint8Array
  pairKey: Uint8Array
  phoneEphemeral: Uint8Array
  hostEphemeral: Uint8Array
  ch: string
}): ChannelKeys {
  const shared = sharedSecret(options.secretKey, options.peerPublic)
  const info = concat(utf8(`${LABEL} channel ${options.ch}`), options.phoneEphemeral, options.hostEphemeral)
  const okm = hkdf(sha256, shared, options.pairKey, info, 64)
  return { p2h: okm.slice(0, 32), h2p: okm.slice(32, 64) }
}

/** The computer's proof, in `accept`, that it holds the pair key. */
export const acceptConfirm = (keys: ChannelKeys, ch: string): string =>
  b64u(hmac(sha256, keys.h2p, utf8(`${LABEL} accept ${ch}`)).slice(0, 16))

const frameNonce = (n: number): Uint8Array => {
  if (!Number.isSafeInteger(n) || n < 0) throw new E2EError('bad_frame', 'A frame counter must be a whole number.')
  const nonce = new Uint8Array(12)
  // 4 zero bytes, then n as 64-bit big-endian (safe integers fit in 53 bits).
  const view = new DataView(nonce.buffer)
  view.setUint32(4, Math.floor(n / 2 ** 32))
  view.setUint32(8, n >>> 0)
  return nonce
}

const frameAad = (ch: string, dir: Direction) => utf8(`${LABEL}|${ch}|${dir}`)

/** Seals one inner frame (JSON text) as the `c` of a `data` frame. */
export function sealFrame(key: Uint8Array, ch: string, dir: Direction, n: number, plaintext: string): string {
  return b64u(gcm(key, frameNonce(n), frameAad(ch, dir)).encrypt(utf8(plaintext)))
}

/** Opens the `c` of a `data` frame; throws `bad_frame` when it was not sealed with this key, channel, direction and counter. */
export function openFrame(key: Uint8Array, ch: string, dir: Direction, n: number, c: unknown): string {
  const sealed = typeof c === 'string' ? fromB64u(c) : null
  if (!sealed || sealed.length < 16) throw new E2EError('bad_frame', 'A frame is not sealed data.')
  try {
    return decoder.decode(gcm(key, frameNonce(n), frameAad(ch, dir)).decrypt(sealed))
  } catch {
    throw new E2EError('bad_frame', 'A frame did not open: it was changed, replayed or sealed with another key.')
  }
}

/** Seals a small value with a random nonce (presence and the pairing secrets): base64url of nonce ‖ ciphertext. */
export function sealBlob(key: Uint8Array, aad: string, plaintext: string): string {
  const nonce = randomBytes(12)
  return b64u(concat(nonce, gcm(key, nonce, utf8(aad)).encrypt(utf8(plaintext))))
}

export function openBlob(key: Uint8Array, aad: string, sealed: unknown): string {
  const bytes = typeof sealed === 'string' ? fromB64u(sealed) : null
  if (!bytes || bytes.length < 28) throw new E2EError('bad_frame', 'A sealed value is not sealed data.')
  try {
    return decoder.decode(gcm(key, bytes.slice(0, 12), utf8(aad)).decrypt(bytes.slice(12)))
  } catch {
    throw new E2EError('bad_frame', 'A sealed value did not open.')
  }
}

export const presenceAad = (hostId: string, shareId: string) => `${LABEL} presence|${hostId}|${shareId}`
export const pairSecretsAad = (pairId: string) => `${LABEL} pair-secrets|${pairId}`
