/**
 * The end-to-end handshake and frames. The fixed vectors are also in
 * docs/RELAY_CONTRACT.md §3.6, so `neuron engine --share` (Rust) and the
 * relay's tests can check they derive the same bytes.
 */
import { describe, expect, test } from 'bun:test'

import { ChannelCrypto } from '../src/relay/channel'
import {
  E2EError,
  acceptConfirm,
  b64u,
  bytesOf,
  deriveChannelKeys,
  derivePairKey,
  fromB64u,
  keyPair,
  newPairingCode,
  openBlob,
  openFrame,
  pairIdFor,
  pairProof,
  proofMatches,
  publicKeyOf,
  sealBlob,
  sealFrame,
  subkey,
} from '../src/relay/crypto'

const VECTORS = {
  hostSecret: 'AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA',
  phoneSecret: 'QEFCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFlaW1xdXl8',
  hostEphemeralSecret: 'gIGCg4SFhoeIiYqLjI2Oj5CRkpOUlZaXmJmam5ydnp8',
  phoneEphemeralSecret: 'oKGio6SlpqeoqaqrrK2ur7CxsrO0tba3uLm6u7y9vr8',
  code: 'wMHCw8TFxsfIycrLzM3Ozw',
  hostPublic: 'B6N8vBQgk8i3VdwbEOhstCY3StFqqFPtC9_AsrhtHHw',
  phonePublic: 'eaYx7t4b-cmPEgMs3q3Q56B5OY_HhriMyEbsia-FpRo',
  pairId: 'xMm4z_g3jDfuYMKwidZHBg',
  pairKey: '4D5wivpGleJpH83D0sn-YabpzY7zC8PG-6GXqDdUalk',
  phoneProof: 'cbwHSSkvuXNM06i3rkMJkw',
  hostProof: 'A9pJGL2ru9oPNO9AkqUO9A',
  pairSecretsKey: 'ml4UAMqUKZdHGLgVhmj2Kn-ArGEsmn2CFyoqF-3mJmo',
  ch: 'channel-0000000001',
  p2h: 'N174o0i91fATCDiCxybPvW1IVSik5YrI1VXfwhySPl8',
  h2p: 'CqAUf_LDYB8oW3HicMS7OV03SzZQgPpuGpyutLS3Ii4',
  confirm: 'WmXZ4JoZr6XvkScveQ9llQ',
  frameP2H0: 'FHfS93E8PCTjaHu3pzTqB3pme1Hi1W-Z3o793A',
  frameH2P1: '7-VzfWIbvw7QbuxBgJrSWrbWheJ1f2hUUvjEfA',
}

const key = (name: keyof typeof VECTORS) => bytesOf(VECTORS[name], 32, name)

describe('base64url', () => {
  test('round-trips every length and refuses other spellings', () => {
    for (let length = 0; length < 40; length++) {
      const bytes = Uint8Array.from({ length }, (_, index) => (index * 37 + length) & 255)
      expect(fromB64u(b64u(bytes))).toEqual(bytes)
    }
    expect(b64u(new Uint8Array([0xfb, 0xff]))).toBe('-_8')
    expect(fromB64u('-_8=')).toBeNull()
    expect(fromB64u('ab+/')).toBeNull()
    expect(fromB64u('A')).toBeNull()
    // Non-zero leftover bits would give one byte string two spellings.
    expect(fromB64u('-_9')).toBeNull()
  })
})

describe('pairing', () => {
  test('matches the published vectors', () => {
    const hostPublic = publicKeyOf(key('hostSecret'))
    const phonePublic = publicKeyOf(key('phoneSecret'))
    expect(b64u(hostPublic)).toBe(VECTORS.hostPublic)
    expect(b64u(phonePublic)).toBe(VECTORS.phonePublic)
    expect(pairIdFor(VECTORS.code)).toBe(VECTORS.pairId)
    const pairKey = derivePairKey({ secretKey: key('phoneSecret'), peerPublic: hostPublic, code: VECTORS.code, hostPublic, phonePublic })
    expect(b64u(pairKey)).toBe(VECTORS.pairKey)
    expect(pairProof(pairKey, 'phone', VECTORS.pairId)).toBe(VECTORS.phoneProof)
    expect(pairProof(pairKey, 'host', VECTORS.pairId)).toBe(VECTORS.hostProof)
    expect(b64u(subkey(pairKey, 'pair-secrets'))).toBe(VECTORS.pairSecretsKey)
  })

  test('both ends derive the same pair key, and only with the code', () => {
    const host = keyPair()
    const phone = keyPair()
    const code = newPairingCode()
    const common = { code, hostPublic: host.publicKey, phonePublic: phone.publicKey }
    const onPhone = derivePairKey({ ...common, secretKey: phone.secretKey, peerPublic: host.publicKey })
    const onHost = derivePairKey({ ...common, secretKey: host.secretKey, peerPublic: phone.publicKey })
    expect(onHost).toEqual(onPhone)
    // A relay that knows only the pair id (a hash of the code) cannot make the phone's proof.
    const relayGuess = derivePairKey({ ...common, code: newPairingCode(), secretKey: host.secretKey, peerPublic: phone.publicKey })
    const pairId = pairIdFor(code)
    expect(proofMatches(pairProof(onPhone, 'phone', pairId), pairProof(relayGuess, 'phone', pairId))).toBe(false)
    expect(proofMatches(pairProof(onPhone, 'phone', pairId), pairProof(onHost, 'phone', pairId))).toBe(true)
    // The two roles' proofs differ, so one cannot be replayed as the other.
    expect(pairProof(onHost, 'host', pairId)).not.toBe(pairProof(onHost, 'phone', pairId))
  })

  test('refuses weak and malformed keys', () => {
    const phone = keyPair()
    const zero = new Uint8Array(32)
    const code = newPairingCode()
    expect(() => derivePairKey({ secretKey: phone.secretKey, peerPublic: zero, code, hostPublic: zero, phonePublic: phone.publicKey })).toThrow(E2EError)
    expect(() => bytesOf('short', 32, 'A key')).toThrow('A key is not 32 bytes')
    expect(() => pairIdFor('AAAA')).toThrow(E2EError)
    expect(proofMatches(VECTORS.hostProof, 42)).toBe(false)
  })
})

describe('channels', () => {
  test('match the published vectors', () => {
    const hostEphemeral = publicKeyOf(key('hostEphemeralSecret'))
    const phoneEphemeral = publicKeyOf(key('phoneEphemeralSecret'))
    const keys = deriveChannelKeys({ secretKey: key('hostEphemeralSecret'), peerPublic: phoneEphemeral, pairKey: key('pairKey'), phoneEphemeral, hostEphemeral, ch: VECTORS.ch })
    expect(b64u(keys.p2h)).toBe(VECTORS.p2h)
    expect(b64u(keys.h2p)).toBe(VECTORS.h2p)
    expect(acceptConfirm(keys, VECTORS.ch)).toBe(VECTORS.confirm)
    expect(sealFrame(keys.p2h, VECTORS.ch, 'p2h', 0, '{"t":"ping"}')).toBe(VECTORS.frameP2H0)
    expect(openFrame(keys.h2p, VECTORS.ch, 'h2p', 1, VECTORS.frameH2P1)).toBe('{"t":"pong"}')
  })

  function pair() {
    const pairKey = keyPair().secretKey
    const phone = keyPair()
    const host = keyPair()
    const ch = 'ch-test-0001'
    const base = { pairKey, phoneEphemeral: phone.publicKey, hostEphemeral: host.publicKey, ch }
    return {
      phoneKeys: deriveChannelKeys({ ...base, secretKey: phone.secretKey, peerPublic: host.publicKey }),
      hostKeys: deriveChannelKeys({ ...base, secretKey: host.secretKey, peerPublic: phone.publicKey }),
      phone,
      host,
      pairKey,
      ch,
    }
  }

  test('a relay that swaps the ephemeral keys learns nothing and is caught', () => {
    const { phoneKeys, hostKeys, phone, pairKey, ch } = pair()
    expect(hostKeys).toEqual(phoneKeys)
    // The relay answers the phone with its own key: without the pair key it cannot confirm.
    const relay = keyPair()
    const relayKeys = deriveChannelKeys({ secretKey: relay.secretKey, peerPublic: phone.publicKey, pairKey: keyPair().secretKey, phoneEphemeral: phone.publicKey, hostEphemeral: relay.publicKey, ch })
    const phoneView = deriveChannelKeys({ secretKey: phone.secretKey, peerPublic: relay.publicKey, pairKey, phoneEphemeral: phone.publicKey, hostEphemeral: relay.publicKey, ch })
    expect(proofMatches(acceptConfirm(phoneView, ch), acceptConfirm(relayKeys, ch))).toBe(false)
  })

  test('frames open only with their key, channel, direction and counter', () => {
    const { phoneKeys: keys, ch } = pair()
    const sealed = sealFrame(keys.p2h, ch, 'p2h', 5, '{"t":"send"}')
    expect(openFrame(keys.p2h, ch, 'p2h', 5, sealed)).toBe('{"t":"send"}')
    expect(() => openFrame(keys.h2p, ch, 'p2h', 5, sealed)).toThrow(E2EError)
    expect(() => openFrame(keys.p2h, 'other-channel', 'p2h', 5, sealed)).toThrow(E2EError)
    expect(() => openFrame(keys.p2h, ch, 'h2p', 5, sealed)).toThrow(E2EError)
    expect(() => openFrame(keys.p2h, ch, 'p2h', 6, sealed)).toThrow(E2EError)
    // One flipped bit anywhere fails the tag.
    const bytes = fromB64u(sealed)!
    bytes[3] ^= 1
    expect(() => openFrame(keys.p2h, ch, 'p2h', 5, b64u(bytes))).toThrow('did not open')
    expect(() => openFrame(keys.p2h, ch, 'p2h', 5, 'short')).toThrow(E2EError)
    expect(() => sealFrame(keys.p2h, ch, 'p2h', -1, 'x')).toThrow(E2EError)
  })

  test('counters: in order, no replays, gaps noticed, unacknowledged frames resent', () => {
    const { phoneKeys, hostKeys, ch } = pair()
    const phone = new ChannelCrypto(ch, 'phone', phoneKeys)
    const host = new ChannelCrypto(ch, 'host', hostKeys)
    const first = phone.seal('one')
    const second = phone.seal('two')
    expect(host.open(first.n, first.c)).toEqual({ kind: 'frame', text: 'one' })
    expect(host.open(first.n, first.c)).toEqual({ kind: 'duplicate' })
    const third = phone.seal('three')
    expect(host.open(third.n, third.c)).toEqual({ kind: 'gap', expected: 1 })
    expect(host.open(second.n, second.c)).toEqual({ kind: 'frame', text: 'two' })
    // The relay passed on frame 0 only: 1 and 2 go again.
    expect(phone.acknowledge(0)!.map((frame) => frame.n)).toEqual([1, 2])
    expect(phone.acknowledge(2)).toEqual([])
    // The computer's frames open on the phone with the other key.
    const reply = host.seal('{"t":"pong"}')
    expect(phone.open(reply.n, reply.c)).toEqual({ kind: 'frame', text: '{"t":"pong"}' })
  })

  test('a resume cannot skip frames the outbox no longer has', () => {
    const { phoneKeys, ch } = pair()
    const phone = new ChannelCrypto(ch, 'phone', phoneKeys)
    for (let index = 0; index < 600; index++) phone.seal(`frame ${index}`)
    expect(phone.acknowledge(10)).toBeNull()
    expect(phone.acknowledge(590)!.length).toBe(9)
  })
})

test('blobs (presence) open only with their key and label', () => {
  const blobKey = keyPair().secretKey
  const sealed = sealBlob(blobKey, 'presence|a|b', '{"v":1}')
  expect(openBlob(blobKey, 'presence|a|b', sealed)).toBe('{"v":1}')
  expect(() => openBlob(blobKey, 'presence|a|c', sealed)).toThrow(E2EError)
  expect(() => openBlob(keyPair().secretKey, 'presence|a|b', sealed)).toThrow(E2EError)
  // A fresh nonce each time: sealing twice never repeats.
  expect(sealBlob(blobKey, 'x', 'same')).not.toBe(sealBlob(blobKey, 'x', 'same'))
})
