/** The pairing link parser: what a camera sees is untrusted, so every field is checked. */
import { describe, expect, test } from 'bun:test'

import { b64u } from '../src/relay/crypto'
import { PairingError, buildPairingLink, buildPairingWebLink, isPrivateHost, parsePairingLink, type PairingPayload } from '../src/relay/pairing'

const NOW = Date.UTC(2026, 9, 23, 12, 0, 0)
const code = b64u(new Uint8Array(16).fill(7))
const hostKey = b64u(new Uint8Array(32).fill(9))

const payload: PairingPayload = {
  relayUrl: 'wss://relay.zero-x.live/v1/relay',
  hostId: 'H0st-Id_12345678',
  code,
  hostKey,
  name: 'Rahul’s laptop',
  accountTag: '0123456789abcdef',
  expiresAt: NOW + 600_000,
}

const fields = (overrides: Record<string, string | null>) => {
  const params = new URLSearchParams(buildPairingLink(payload).slice('neuron://pair?'.length))
  for (const [key, value] of Object.entries(overrides)) {
    if (value === null) params.delete(key)
    else params.set(key, value)
  }
  return `neuron://pair?${params.toString()}`
}

const failure = (input: string, options: Parameters<typeof parsePairingLink>[1] = { now: NOW }) => {
  try {
    parsePairingLink(input, options)
  } catch (error) {
    expect(error).toBeInstanceOf(PairingError)
    return (error as PairingError).code
  }
  throw new Error('parsed')
}

describe('parsePairingLink', () => {
  test('reads what buildPairingLink writes, in both link forms', () => {
    expect(parsePairingLink(buildPairingLink(payload), { now: NOW })).toEqual(payload)
    expect(parsePairingLink(buildPairingWebLink(payload), { now: NOW })).toEqual(payload)
    // Whitespace around a pasted link, and a trailing slash on the relay, do not matter.
    const pasted = `  ${fields({ r: 'wss://relay.zero-x.live/v1/relay/' })}\n`
    expect(parsePairingLink(pasted, { now: NOW }).relayUrl).toBe('wss://relay.zero-x.live/v1/relay')
  })

  test('the optional fields may be absent', () => {
    const parsed = parsePairingLink(fields({ n: null, u: null, e: null }), { now: NOW })
    expect(parsed.name).toBeNull()
    expect(parsed.accountTag).toBeNull()
    expect(parsed.expiresAt).toBeNull()
  })

  test('refuses what is not a pairing link', () => {
    expect(failure('')).toBe('not_a_pairing_link')
    expect(failure('hello')).toBe('not_a_pairing_link')
    expect(failure('https://example.com/other#v=1')).toBe('not_a_pairing_link')
    expect(failure('http://neuron.zero-x.live/pair#v=1')).toBe('not_a_pairing_link')
    expect(failure(`neuron://pair?${'x'.repeat(3000)}`)).toBe('not_a_pairing_link')
  })

  test('refuses other versions, and missing fields', () => {
    expect(failure(fields({ v: '2' }))).toBe('unsupported_version')
    expect(failure(fields({ v: null }))).toBe('unsupported_version')
    for (const key of ['r', 'h', 'c', 'k']) expect(failure(fields({ [key]: null }))).toBe('missing_field')
  })

  test('checks the relay address', () => {
    expect(failure(fields({ r: 'not a url' }))).toBe('bad_relay')
    expect(failure(fields({ r: 'https://relay.zero-x.live/v1/relay' }))).toBe('bad_relay')
    expect(failure(fields({ r: 'wss://user:pw@relay.zero-x.live/v1/relay' }))).toBe('bad_relay')
    expect(failure(fields({ r: 'wss://relay.zero-x.live/v1/relay?token=x' }))).toBe('bad_relay')
    // Plain ws:// only on this network.
    expect(failure(fields({ r: 'ws://relay.zero-x.live/v1/relay' }))).toBe('insecure_relay')
    // A store build pairs through Zero-X's relay only...
    expect(failure(fields({ r: 'wss://evil.example/v1/relay' }))).toBe('untrusted_relay')
    expect(failure(fields({ r: 'wss://zero-x.live.evil.example/v1/relay' }))).toBe('untrusted_relay')
    // ...and a development build takes a relay on the developer's machine.
    const dev = parsePairingLink(fields({ r: 'ws://192.168.1.20:8790/v1/relay' }), { now: NOW, trustedRelayHosts: null })
    expect(dev.relayUrl).toBe('ws://192.168.1.20:8790/v1/relay')
    expect(failure(fields({ r: 'ws://192.168.1.20:8790/v1/relay' }))).toBe('untrusted_relay')
  })

  test('checks ids, the code and the key byte for byte', () => {
    expect(failure(fields({ h: 'short' }))).toBe('bad_host_id')
    expect(failure(fields({ h: 'has spaces in it' }))).toBe('bad_host_id')
    expect(failure(fields({ c: b64u(new Uint8Array(15)) }))).toBe('bad_code')
    expect(failure(fields({ c: 'not*base64url!!!!!!!!!' }))).toBe('bad_code')
    expect(failure(fields({ k: b64u(new Uint8Array(31)) }))).toBe('bad_key')
    expect(failure(fields({ k: `${hostKey.slice(0, -1)}=` }))).toBe('bad_key')
    expect(failure(fields({ u: 'NOT-HEX' }))).toBe('bad_account')
    expect(failure(fields({ n: 'x'.repeat(65) }))).toBe('bad_name')
  })

  test('cleans the name and refuses an expired code', () => {
    expect(parsePairingLink(fields({ n: ' Lab\u0007 box ' }), { now: NOW }).name).toBe('Lab box')
    expect(failure(fields({ e: String(Math.floor(NOW / 1000) - 1) }))).toBe('expired')
    expect(failure(fields({ e: 'soon' }))).toBe('expired')
  })

  test('the messages are for people', () => {
    try {
      parsePairingLink(fields({ e: String(Math.floor(NOW / 1000) - 1) }), { now: NOW })
    } catch (error) {
      expect((error as Error).message).toBe('This pairing code has expired. Show a new one on the computer.')
    }
  })
})

test('isPrivateHost', () => {
  for (const host of ['localhost', '127.0.0.1', '10.1.2.3', '192.168.0.9', '172.20.1.1', 'laptop.local', '[::1]', 'fd12:3456::1']) {
    expect(isPrivateHost(host)).toBe(true)
  }
  for (const host of ['zero-x.live', '8.8.8.8', '172.32.0.1', '192.169.0.1', 'example.com']) expect(isPrivateHost(host)).toBe(false)
})
