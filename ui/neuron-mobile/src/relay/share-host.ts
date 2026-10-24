/**
 * The computer's side of the relay, in TypeScript: the reference that
 * `neuron engine --share` (WP1, Rust) mirrors, and what the dev share host
 * (../../dev/share.ts) and the integration tests run.
 *
 * It offers one-time pairing codes, answers pairings, accepts encrypted
 * channels from paired phones and bridges each one to a local remote-protocol
 * host (one per shared session). It also keeps each session's sealed presence
 * current and sends a contentless `notify` when a session needs the user or
 * finishes; the relay decides whether that becomes a push.
 *
 * The bridge only lets through what a phone may do: `hello` (with the local
 * host's token put in), `send`, `ping`, and the read-only `rpc` methods.
 * Terminals and engine restarts stay with clients on the computer itself.
 */

import { parseClientFrame } from '@neuron/client'

import { ChannelCrypto } from './channel'
import type { NotifyKind, PairSecrets, Presence, RelayToHost } from './contract'
import {
  acceptConfirm,
  b64u,
  bytesOf,
  derivePairKey,
  deriveChannelKeys,
  keyPair,
  newPairingCode,
  pairIdFor,
  pairProof,
  pairSecretsAad,
  presenceAad,
  proofMatches,
  publicKeyOf,
  randomBytes,
  sealBlob,
  subkey,
} from './crypto'
import type { RelayLink } from './link'
import { buildPairingLink, type PairingPayload } from './pairing'

/** The computer's own keys: kept in its Neuron config, never sent anywhere but the public key. */
export interface HostIdentity {
  hostId: string
  name: string
  /** X25519 secret key, base64url. */
  secretKey: string
  /** Seals presence for every paired phone, base64url. */
  presenceKey: string
}

export interface PairedPhone {
  phoneId: string
  name: string
  pairKey: string
  pairedAt: number
}

export function newHostIdentity(name: string): HostIdentity {
  return { hostId: b64u(randomBytes(16)), name, secretKey: b64u(keyPair().secretKey), presenceKey: b64u(randomBytes(32)) }
}

/** A duplex to a local remote-protocol host, as a client of it (a WebSocket to `NeuronHost`, or the engine's own). */
export interface LocalConnection {
  send(text: string): void
  close(): void
  onmessage: ((text: string) => void) | null
  onclose: (() => void) | null
}

/** One shared session. */
export interface ShareSource {
  shareId: string
  /** Connects to the session's local host; resolves once the socket is open. */
  connect(): Promise<LocalConnection>
  /** The local host's token, put into the phone's `hello`. */
  token: string
  presence(): Presence
  /** Calls `listener` whenever the session changed. */
  onChange(listener: () => void): () => void
}

const PHONE_RPC = new Set(['fs.list', 'fs.read', 'fs.search', 'git.status', 'git.diff'])

interface Bridge {
  ch: string
  phoneId: string
  shareId: string
  crypto: ChannelCrypto
  local: LocalConnection | null
  /** Phone frames that arrived before the local socket opened. */
  pending: string[]
  closed: boolean
}

export interface OfferedPairing {
  payload: PairingPayload
  link: string
  pairId: string
  expiresAt: number
}

export interface ShareHostOptions {
  link: RelayLink
  identity: HostIdentity
  phones: Map<string, PairedPhone>
  /** Called after a pairing or an unpairing, to persist `phones`. */
  savePhones?: (phones: Map<string, PairedPhone>) => void
  /** Asked before a phone is paired (a prompt on the computer); allowed when absent. */
  approvePairing?: (phoneName: string) => Promise<boolean> | boolean
}

export class ShareHost {
  private sources = new Map<string, { source: ShareSource; unsubscribe: () => void; last: string; status: Presence['status'] | null }>()
  private bridges = new Map<string, Bridge>()
  private offers = new Map<string, { code: string; resolve: (offer: OfferedPairing) => void; reject: (error: Error) => void; payload: PairingPayload }>()
  private pairedListeners = new Set<(phone: PairedPhone) => void>()
  private presenceTimers = new Map<string, ReturnType<typeof setTimeout>>()

  constructor(private readonly options: ShareHostOptions) {
    const { link } = options
    link.onReady(() => {
      for (const entry of this.sources.values()) {
        entry.last = ''
        this.sendPresence(entry.source.shareId)
      }
    })
    link.onFrame((frame) => void this.handle(frame as unknown as RelayToHost))
    link.onStatus((state) => {
      // Channels do not survive this end's reconnect: phones attach again.
      if (state.status !== 'open') for (const bridge of [...this.bridges.values()]) this.endBridge(bridge, false)
    })
  }

  get publicKey(): string {
    return b64u(publicKeyOf(bytesOf(this.options.identity.secretKey, 32, 'The host key')))
  }

  get phones(): Map<string, PairedPhone> {
    return this.options.phones
  }

  /** Channels attached right now. */
  get attached(): number {
    return this.bridges.size
  }

  onPaired(listener: (phone: PairedPhone) => void): () => void {
    this.pairedListeners.add(listener)
    return () => this.pairedListeners.delete(listener)
  }

  /** Starts sharing a session: its presence goes to the relay, and paired phones can attach. */
  share(source: ShareSource): void {
    const unsubscribe = source.onChange(() => this.schedulePresence(source.shareId))
    this.sources.set(source.shareId, { source, unsubscribe, last: '', status: null })
    this.sendPresence(source.shareId)
  }

  unshare(shareId: string): void {
    const entry = this.sources.get(shareId)
    if (!entry) return
    entry.unsubscribe()
    this.sources.delete(shareId)
    for (const bridge of [...this.bridges.values()]) if (bridge.shareId === shareId) this.endBridge(bridge, true)
    this.options.link.send({ t: 'unshare', share_id: shareId })
  }

  /** A one-time pairing code for the QR, valid for `ttlS` seconds. */
  offerPairing(relayUrl: string, ttlS = 600): Promise<OfferedPairing> {
    const code = newPairingCode()
    const pairId = pairIdFor(code)
    const payload: PairingPayload = {
      relayUrl,
      hostId: this.options.identity.hostId,
      code,
      hostKey: this.publicKey,
      name: this.options.identity.name,
      accountTag: this.options.link.accountTag,
      expiresAt: null,
    }
    return new Promise((resolve, reject) => {
      this.offers.set(pairId, { code, resolve, reject, payload })
      if (!this.options.link.send({ t: 'pair_offer', pair_id: pairId, ttl_s: ttlS })) {
        this.offers.delete(pairId)
        reject(new Error('Not connected to the relay.'))
      }
    })
  }

  /** Forgets a phone: it can no longer attach. */
  forgetPhone(phoneId: string): void {
    this.options.phones.delete(phoneId)
    this.options.savePhones?.(this.options.phones)
    for (const bridge of [...this.bridges.values()]) if (bridge.phoneId === phoneId) this.endBridge(bridge, true)
  }

  stop(): void {
    for (const bridge of [...this.bridges.values()]) this.endBridge(bridge, true)
    for (const entry of this.sources.values()) entry.unsubscribe()
    for (const timer of this.presenceTimers.values()) clearTimeout(timer)
    this.presenceTimers.clear()
  }

  // ------------------------------------------------------------- presence

  private schedulePresence(shareId: string): void {
    if (this.presenceTimers.has(shareId)) return
    // At most a few updates a second, however busy the session.
    this.presenceTimers.set(
      shareId,
      setTimeout(() => {
        this.presenceTimers.delete(shareId)
        this.sendPresence(shareId)
      }, 150),
    )
  }

  private sendPresence(shareId: string): void {
    const entry = this.sources.get(shareId)
    if (!entry || !this.options.link.ready) return
    const presence = entry.source.presence()
    const { updated_at_ms: _at, ...content } = presence
    const fingerprint = JSON.stringify(content)
    if (fingerprint !== entry.last) {
      entry.last = fingerprint
      const key = bytesOf(this.options.identity.presenceKey, 32, 'The presence key')
      const sealed = sealBlob(key, presenceAad(this.options.identity.hostId, shareId), JSON.stringify(presence))
      this.options.link.send({ t: 'share', share_id: shareId, sealed })
    }
    // A turn that needs the user, or one that ended: the relay may push.
    const previous = entry.status
    entry.status = presence.status
    let kind: NotifyKind | null = null
    if (presence.status === 'needs_me' && previous !== 'needs_me') kind = 'needs_me'
    else if (previous === 'running' && (presence.status === 'idle' || presence.status === 'failed')) kind = 'finished'
    if (kind) this.options.link.send({ t: 'notify', share_id: shareId, kind })
  }

  // -------------------------------------------------------------- frames

  private async handle(frame: RelayToHost): Promise<void> {
    switch (frame.t) {
      case 'pair_offer_ok': {
        const offer = this.offers.get(frame.pair_id)
        if (!offer) return
        offer.payload.expiresAt = frame.expires_at
        offer.resolve({ payload: offer.payload, link: buildPairingLink(offer.payload), pairId: frame.pair_id, expiresAt: frame.expires_at })
        return
      }
      case 'error': {
        if (frame.pair_id) {
          const offer = this.offers.get(frame.pair_id)
          this.offers.delete(frame.pair_id)
          offer?.reject(new Error(frame.message))
        }
        if (frame.ch) {
          const bridge = this.bridges.get(frame.ch)
          if (bridge) this.endBridge(bridge, false)
        }
        return
      }
      case 'pair':
        return this.pair(frame)
      case 'open':
        return this.open(frame)
      case 'data':
        return this.data(frame)
      case 'close': {
        const bridge = this.bridges.get(frame.ch)
        if (bridge) this.endBridge(bridge, false)
        return
      }
      case 'unpaired':
        this.forgetPhone(frame.phone_id)
        return
      case 'peer':
        // The phone is away (its socket dropped) or back: the relay keeps the frames meanwhile.
        return
    }
  }

  private async pair(frame: Extract<RelayToHost, { t: 'pair' }>): Promise<void> {
    const { link, identity } = this.options
    const offer = this.offers.get(frame.pair_id)
    const deny = (code: string) => link.send({ t: 'pair_denied', pair_id: frame.pair_id, phone_id: frame.phone_id, code })
    if (!offer) return void deny('pair_not_found')
    let pairKey: Uint8Array
    try {
      const secretKey = bytesOf(identity.secretKey, 32, 'The host key')
      const phonePublic = bytesOf(frame.pub, 32, "The phone's key")
      pairKey = derivePairKey({ secretKey, peerPublic: phonePublic, code: offer.code, hostPublic: publicKeyOf(secretKey), phonePublic })
    } catch {
      return void deny('bad_proof')
    }
    if (!proofMatches(pairProof(pairKey, 'phone', frame.pair_id), frame.proof)) return void deny('bad_proof')
    // One phone per code: the code is spent whatever the answer.
    this.offers.delete(frame.pair_id)
    const allowed = this.options.approvePairing ? await this.options.approvePairing(frame.name) : true
    if (!allowed) return void deny('denied')
    const secrets: PairSecrets = { presence_key: identity.presenceKey, name: identity.name }
    const phone: PairedPhone = { phoneId: frame.phone_id, name: frame.name, pairKey: b64u(pairKey), pairedAt: Date.now() }
    this.options.phones.set(phone.phoneId, phone)
    this.options.savePhones?.(this.options.phones)
    link.send({
      t: 'paired',
      pair_id: frame.pair_id,
      phone_id: frame.phone_id,
      proof: pairProof(pairKey, 'host', frame.pair_id),
      sealed: sealBlob(subkey(pairKey, 'pair-secrets'), pairSecretsAad(frame.pair_id), JSON.stringify(secrets)),
    })
    for (const listener of [...this.pairedListeners]) listener(phone)
    // The new phone lists this computer's sessions next: make sure the relay has them.
    for (const entry of this.sources.values()) {
      entry.last = ''
      this.sendPresence(entry.source.shareId)
    }
  }

  private async open(frame: Extract<RelayToHost, { t: 'open' }>): Promise<void> {
    const { link } = this.options
    const phone = this.options.phones.get(frame.phone_id)
    if (!phone) return void link.send({ t: 'reject', ch: frame.ch, code: 'not_paired' })
    const entry = this.sources.get(frame.share_id)
    if (!entry) return void link.send({ t: 'reject', ch: frame.ch, code: 'unknown_share' })
    let bridge: Bridge
    try {
      const ephemeral = keyPair()
      const phoneEphemeral = bytesOf(frame.e, 32, "The phone's channel key")
      const keys = deriveChannelKeys({
        secretKey: ephemeral.secretKey,
        peerPublic: phoneEphemeral,
        pairKey: bytesOf(phone.pairKey, 32, 'The pair key'),
        phoneEphemeral,
        hostEphemeral: ephemeral.publicKey,
        ch: frame.ch,
      })
      bridge = { ch: frame.ch, phoneId: phone.phoneId, shareId: frame.share_id, crypto: new ChannelCrypto(frame.ch, 'host', keys), local: null, pending: [], closed: false }
      this.bridges.set(frame.ch, bridge)
      link.send({ t: 'accept', ch: frame.ch, e: b64u(ephemeral.publicKey), confirm: acceptConfirm(keys, frame.ch) })
    } catch {
      return void link.send({ t: 'reject', ch: frame.ch, code: 'bad_key' })
    }
    try {
      const local = await entry.source.connect()
      if (bridge.closed) return local.close()
      bridge.local = local
      local.onmessage = (text) => {
        if (bridge.closed) return
        const sealed = bridge.crypto.seal(text)
        link.send({ t: 'data', ch: bridge.ch, n: sealed.n, c: sealed.c })
      }
      local.onclose = () => this.endBridge(bridge, true)
      for (const text of bridge.pending.splice(0)) this.toLocal(bridge, text)
    } catch {
      this.endBridge(bridge, true)
    }
  }

  private data(frame: Extract<RelayToHost, { t: 'data' }>): void {
    const bridge = this.bridges.get(frame.ch)
    if (!bridge) return
    let opened
    try {
      opened = bridge.crypto.open(frame.n, frame.c)
    } catch {
      // Not from the paired phone: end the channel rather than guess.
      return this.endBridge(bridge, true)
    }
    if (opened.kind !== 'frame') return
    if (!bridge.local) bridge.pending.push(opened.text)
    else this.toLocal(bridge, opened.text)
  }

  /** Passes one phone frame to the local host, if a phone may send it. */
  private toLocal(bridge: Bridge, text: string): void {
    const frame = parseClientFrame(text)
    if (!frame || !bridge.local) return
    const source = this.sources.get(bridge.shareId)?.source
    switch (frame.t) {
      case 'hello':
        bridge.local.send(JSON.stringify({ ...frame, token: source?.token ?? '' }))
        return
      case 'send':
      case 'ping':
        bridge.local.send(JSON.stringify(frame))
        return
      case 'rpc':
        if (PHONE_RPC.has(frame.method)) bridge.local.send(JSON.stringify(frame))
        else this.replyToPhone(bridge, { t: 'rpc_result', id: frame.id, error: 'Not available from the phone.' })
        return
      default:
        // Terminal frames: only for clients on this computer.
        return
    }
  }

  private replyToPhone(bridge: Bridge, frame: unknown): void {
    const sealed = bridge.crypto.seal(JSON.stringify(frame))
    this.options.link.send({ t: 'data', ch: bridge.ch, n: sealed.n, c: sealed.c })
  }

  private endBridge(bridge: Bridge, tellRelay: boolean): void {
    if (bridge.closed) return
    bridge.closed = true
    this.bridges.delete(bridge.ch)
    if (tellRelay) this.options.link.send({ t: 'close', ch: bridge.ch })
    const local = bridge.local
    bridge.local = null
    if (local) {
      local.onclose = null
      local.close()
    }
  }
}
