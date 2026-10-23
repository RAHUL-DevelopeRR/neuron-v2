/**
 * The phone's side of the relay: pairing with a computer, the computers and
 * shared sessions of the account (with their sealed presence opened), and
 * encrypted channels to one shared session each.
 *
 * A channel is a WebSocket-shaped object (`SocketLike` from @neuron/client),
 * so the shared client's `WebSocketTransport` runs remote protocol 1 over it
 * unchanged. When the relay socket drops, open channels stay open: after the
 * reconnect each one asks the relay to `resume` after the last frame it
 * opened, and sends again what the computer did not get. Only when the relay
 * cannot resume does the channel close, and the transport attaches again for
 * a fresh snapshot (./transport.ts).
 */

import type { SocketLike } from '@neuron/client'

import { ChannelCrypto } from './channel'
import type { HostListing, PairSecrets, Presence, RelayToPhone } from './contract'
import {
  E2EError,
  acceptConfirm,
  b64u,
  bytesOf,
  derivePairKey,
  deriveChannelKeys,
  fromB64u,
  keyPair,
  openBlob,
  pairIdFor,
  pairProof,
  pairSecretsAad,
  presenceAad,
  proofMatches,
  randomId,
  subkey,
  type KeyPair,
} from './crypto'
import type { RelayLink } from './link'
import type { PairingPayload } from './pairing'

/** A computer this phone paired with. `pairKey` and `presenceKey` are secrets: secure storage only. */
export interface PairedComputer {
  hostId: string
  relayUrl: string
  name: string
  pairKey: string
  presenceKey: string
  accountTag: string | null
  pairedAt: number
}

export interface ShareView {
  hostId: string
  shareId: string
  /** `null` until the computer sends one, or when it does not open (not paired with this key). */
  presence: Presence | null
  at: number
}

export interface HostView {
  hostId: string
  name: string
  online: boolean
  shares: ShareView[]
}

/** Close codes a channel reports to its transport. */
export const ChannelClose = {
  left: 1000,
  /** The relay could not resume it; attach again for a snapshot. */
  lost: 4001,
  /** The other end did not prove the pair key: not the paired computer. */
  untrusted: 4002,
  /** The computer refused it (not paired, no such session). */
  refused: 4003,
  /** The computer stopped sharing the session, or went offline. */
  ended: 4004,
} as const

const PAIR_ERRORS: Record<string, string> = {
  pair_not_found:
    "No computer on your Zero-X account is waiting for this code. Check that the computer is signed in to the same account, and show a new code if this one expired.",
  bad_proof: 'The computer could not check this pairing. Show a new code on the computer and scan it again.',
  denied: 'The computer refused to pair.',
  limit: 'Your account already has as many phones as it can pair. Forget one first.',
}

export class PairingFailed extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message)
    this.name = 'PairingFailed'
  }
}

interface PendingPair {
  payload: PairingPayload
  keys: KeyPair
  pairKey: Uint8Array
  pairId: string
  resolve: (computer: PairedComputer) => void
  reject: (error: Error) => void
  timer: ReturnType<typeof setTimeout>
}

export class PhoneRelay {
  readonly hosts = new Map<string, HostView>()
  /** The relay answered `list` at least once since the last reconnect. */
  listed = false
  private listeners = new Set<() => void>()
  private channels = new Map<string, ChannelSocket>()
  private pairs = new Map<string, PendingPair>()

  constructor(
    readonly link: RelayLink,
    /** The paired computer with this id, with its keys. */
    private readonly computer: (hostId: string) => PairedComputer | undefined,
  ) {
    link.onReady(() => {
      this.listed = false
      link.send({ t: 'list' })
      for (const channel of [...this.channels.values()]) channel.linkReady()
    })
    link.onFrame((frame) => this.handle(frame as unknown as RelayToPhone))
  }

  onChange(listener: () => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  private changed(): void {
    for (const listener of [...this.listeners]) listener()
  }

  /** Asks the relay for the account's computers again. */
  refresh(): void {
    this.link.send({ t: 'list' })
  }

  /**
   * Pairs with the computer in `payload`: sends this phone's key and a proof of
   * the code, and checks the computer's proof before keeping anything.
   */
  pair(payload: PairingPayload, phoneName: string, timeoutMs = 30_000): Promise<PairedComputer> {
    if (!this.link.ready) return Promise.reject(new PairingFailed('offline', 'Not connected to the relay yet. Check your connection and try again.'))
    if (payload.accountTag && this.link.accountTag && payload.accountTag !== this.link.accountTag) {
      return Promise.reject(
        new PairingFailed('other_account', 'This computer is signed in to a different Zero-X account. Sign in to the same account on both.'),
      )
    }
    const keys = keyPair()
    const hostPublic = bytesOf(payload.hostKey, 32, "The computer's key")
    const pairKey = derivePairKey({ secretKey: keys.secretKey, peerPublic: hostPublic, code: payload.code, hostPublic, phonePublic: keys.publicKey })
    const pairId = pairIdFor(payload.code)
    return new Promise<PairedComputer>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pairs.delete(pairId)
        reject(new PairingFailed('timeout', 'The computer did not answer. Keep Neuron open on it and try again.'))
      }, timeoutMs)
      this.pairs.set(pairId, { payload, keys, pairKey, pairId, resolve, reject, timer })
      this.link.send({ t: 'pair', pair_id: pairId, pub: b64u(keys.publicKey), proof: pairProof(pairKey, 'phone', pairId), name: phoneName })
    })
  }

  /** Forgets a computer on the relay (the computer hears `unpaired`). The caller deletes the keys. */
  unpair(hostId: string): void {
    this.link.send({ t: 'unpair', host_id: hostId })
    this.hosts.delete(hostId)
    this.changed()
  }

  /** An encrypted channel to one shared session; open once `onopen` fires. */
  openChannel(hostId: string, shareId: string): ChannelSocket {
    const channel = new ChannelSocket(this, hostId, shareId, this.computer(hostId))
    this.channels.set(channel.ch, channel)
    channel.begin()
    return channel
  }

  /** @internal */
  forget(channel: ChannelSocket): void {
    this.channels.delete(channel.ch)
  }

  private openPresence(hostId: string, shareId: string, sealed: string | null | undefined): Presence | null {
    const computer = this.computer(hostId)
    if (!computer || !sealed) return null
    try {
      const key = bytesOf(computer.presenceKey, 32, 'The presence key')
      const value = JSON.parse(openBlob(key, presenceAad(hostId, shareId), sealed)) as Presence
      return value && value.v === 1 && typeof value.project === 'string' ? value : null
    } catch {
      return null
    }
  }

  private setHost(listing: HostListing): void {
    this.hosts.set(listing.host_id, {
      hostId: listing.host_id,
      name: listing.name,
      online: listing.online,
      shares: listing.shares.map((share) => ({
        hostId: listing.host_id,
        shareId: share.share_id,
        presence: this.openPresence(listing.host_id, share.share_id, share.sealed),
        at: share.at,
      })),
    })
  }

  private handle(frame: RelayToPhone): void {
    switch (frame.t) {
      case 'hosts':
        this.hosts.clear()
        for (const listing of Array.isArray(frame.hosts) ? frame.hosts : []) this.setHost(listing)
        this.listed = true
        return this.changed()
      case 'host': {
        const host = this.hosts.get(frame.host_id)
        if (host) {
          host.online = frame.online
          host.name = frame.name || host.name
          if (!frame.online) host.shares = []
        } else {
          this.setHost({ host_id: frame.host_id, name: frame.name, online: frame.online, shares: [] })
        }
        return this.changed()
      }
      case 'presence': {
        let host = this.hosts.get(frame.host_id)
        if (!host) {
          host = { hostId: frame.host_id, name: this.computer(frame.host_id)?.name ?? 'Computer', online: true, shares: [] }
          this.hosts.set(frame.host_id, host)
        }
        host.shares = host.shares.filter((share) => share.shareId !== frame.share_id)
        if (frame.sealed !== null) {
          host.shares.push({ hostId: frame.host_id, shareId: frame.share_id, presence: this.openPresence(frame.host_id, frame.share_id, frame.sealed), at: frame.at })
        }
        return this.changed()
      }
      case 'paired':
        return this.paired(frame)
      case 'pair_denied': {
        const pending = this.pairs.get(frame.pair_id)
        if (!pending) return
        this.settlePair(pending, new PairingFailed(frame.code, PAIR_ERRORS[frame.code] ?? PAIR_ERRORS.denied))
        return
      }
      case 'unpaired':
        this.hosts.delete(frame.host_id)
        return this.changed()
      case 'error': {
        if (frame.pair_id) {
          const pending = this.pairs.get(frame.pair_id)
          if (pending) this.settlePair(pending, new PairingFailed(frame.code, PAIR_ERRORS[frame.code] ?? frame.message))
          return
        }
        if (frame.ch) this.channels.get(frame.ch)?.relayError(frame.code, frame.message)
        return
      }
      case 'accept':
      case 'reject':
      case 'data':
      case 'close':
      case 'resumed':
      case 'resume_failed':
        this.channels.get(frame.ch)?.handle(frame)
        return
    }
  }

  private settlePair(pending: PendingPair, result: PairedComputer | Error): void {
    clearTimeout(pending.timer)
    this.pairs.delete(pending.pairId)
    if (result instanceof Error) pending.reject(result)
    else pending.resolve(result)
  }

  private paired(frame: Extract<RelayToPhone, { t: 'paired' }>): void {
    const pending = this.pairs.get(frame.pair_id)
    if (!pending) return
    if (frame.host_id !== pending.payload.hostId || !proofMatches(pairProof(pending.pairKey, 'host', pending.pairId), frame.proof)) {
      return this.settlePair(pending, new PairingFailed('bad_proof', 'The computer that answered is not the one in the QR code. Nothing was paired.'))
    }
    let secrets: PairSecrets
    try {
      secrets = JSON.parse(openBlob(subkey(pending.pairKey, 'pair-secrets'), pairSecretsAad(pending.pairId), frame.sealed)) as PairSecrets
      if (fromB64u(secrets.presence_key)?.length !== 32) throw new Error('bad presence key')
    } catch {
      return this.settlePair(pending, new PairingFailed('bad_proof', 'The computer sent pairing data that does not open. Nothing was paired.'))
    }
    this.settlePair(pending, {
      hostId: pending.payload.hostId,
      relayUrl: pending.payload.relayUrl,
      name: (secrets.name || frame.name || pending.payload.name || 'Computer').slice(0, 64),
      pairKey: b64u(pending.pairKey),
      presenceKey: secrets.presence_key,
      accountTag: this.link.accountTag,
      pairedAt: Date.now(),
    })
  }
}

const CONNECTING = 0
const OPEN = 1
const CLOSED = 3

/** One encrypted channel to a shared session, shaped like a WebSocket for `WebSocketTransport`. */
export class ChannelSocket implements SocketLike {
  readonly ch = randomId()
  readyState = CONNECTING
  onopen: ((event: unknown) => void) | null = null
  onmessage: ((event: { data: unknown }) => void) | null = null
  onclose: ((event: { code: number; reason: string }) => void) | null = null
  onerror: ((event: unknown) => void) | null = null
  /** Times this channel resumed after the relay socket dropped. */
  resumes = 0
  private ephemeral = keyPair()
  private crypto: ChannelCrypto | null = null
  /** Waiting for `resumed`: sends wait in the outbox until then. */
  private resuming = false
  private sentOpen = false

  constructor(
    private readonly relay: PhoneRelay,
    readonly hostId: string,
    readonly shareId: string,
    private readonly computer: PairedComputer | undefined,
  ) {}

  /** @internal Sends `open` (now, or once the relay link is up). */
  begin(): void {
    // Handlers are set by the caller right after this returns.
    queueMicrotask(() => {
      if (!this.computer) return this.end(ChannelClose.refused, 'This computer is not paired with this phone.')
      if (this.relay.link.ready) this.sendOpen()
    })
  }

  private sendOpen(): void {
    this.sentOpen = true
    this.relay.link.send({ t: 'open', ch: this.ch, host_id: this.hostId, share_id: this.shareId, e: b64u(this.ephemeral.publicKey) })
  }

  /** @internal The relay link (re)joined. */
  linkReady(): void {
    if (this.readyState === CONNECTING) {
      if (!this.sentOpen) return this.sendOpen()
      // The `open` may or may not have reached the computer: start over with a fresh channel.
      return this.end(ChannelClose.lost, 'The relay connection dropped while attaching.')
    }
    if (this.readyState !== OPEN || !this.crypto) return
    this.resuming = true
    this.relay.link.send({ t: 'resume', ch: this.ch, after: this.crypto.lastReceived })
  }

  /** @internal */
  handle(frame: Extract<RelayToPhone, { ch: string }>): void {
    switch (frame.t) {
      case 'accept': {
        if (this.readyState !== CONNECTING || !this.computer) return
        try {
          const hostEphemeral = bytesOf(frame.e, 32, "The computer's channel key")
          const keys = deriveChannelKeys({
            secretKey: this.ephemeral.secretKey,
            peerPublic: hostEphemeral,
            pairKey: bytesOf(this.computer.pairKey, 32, 'The pair key'),
            phoneEphemeral: this.ephemeral.publicKey,
            hostEphemeral,
            ch: this.ch,
          })
          if (!proofMatches(acceptConfirm(keys, this.ch), frame.confirm)) throw new E2EError('bad_confirm', 'bad confirm')
          this.crypto = new ChannelCrypto(this.ch, 'phone', keys)
        } catch {
          this.relay.link.send({ t: 'close', ch: this.ch, code: 'untrusted' })
          return this.end(ChannelClose.untrusted, 'The computer could not prove it is the one this phone paired with.')
        }
        this.readyState = OPEN
        this.onopen?.({})
        return
      }
      case 'reject':
        return this.end(
          ChannelClose.refused,
          frame.code === 'not_paired' ? 'This computer no longer knows this phone. Pair again.' : 'The computer is not sharing this session any more.',
        )
      case 'data': {
        if (!this.crypto || this.readyState !== OPEN) return
        let opened
        try {
          opened = this.crypto.open(frame.n, frame.c)
        } catch {
          this.relay.link.send({ t: 'close', ch: this.ch, code: 'bad_frame' })
          return this.end(ChannelClose.untrusted, 'A frame from the relay did not open; the channel was closed to be safe.')
        }
        if (opened.kind === 'gap') {
          // Frames went missing between the relay and here: ask for them again.
          this.resuming = true
          this.relay.link.send({ t: 'resume', ch: this.ch, after: this.crypto.lastReceived })
          return
        }
        if (opened.kind === 'frame') this.onmessage?.({ data: opened.text })
        return
      }
      case 'resumed': {
        if (!this.crypto) return
        const resend = this.crypto.acknowledge(frame.received)
        if (resend === null) return this.end(ChannelClose.lost, 'Messages to the computer were lost while offline.')
        this.resuming = false
        this.resumes++
        for (const sealed of resend) this.relay.link.send({ t: 'data', ch: this.ch, n: sealed.n, c: sealed.c })
        return
      }
      case 'resume_failed':
        return this.end(ChannelClose.lost, 'The relay could not resume the session.')
      case 'close':
        return this.end(ChannelClose.ended, frame.code === 'host_gone' ? 'The computer went offline.' : 'The computer stopped sharing this session.')
    }
  }

  /** @internal An `error` for this channel (the computer is offline, the session unknown). */
  relayError(code: string, message: string): void {
    this.end(code === 'host_offline' ? ChannelClose.ended : ChannelClose.refused, message)
  }

  send(data: string): void {
    if (this.readyState !== OPEN || !this.crypto) throw new Error('The channel is not open.')
    const sealed = this.crypto.seal(data)
    // While the relay reconnects, the frame waits in the outbox and goes after `resumed`.
    if (!this.resuming) this.relay.link.send({ t: 'data', ch: this.ch, n: sealed.n, c: sealed.c })
  }

  close(code: number = ChannelClose.left, reason = 'left'): void {
    if (this.readyState === CLOSED) return
    this.relay.link.send({ t: 'close', ch: this.ch })
    this.end(code, reason)
  }

  private end(code: number, reason: string): void {
    if (this.readyState === CLOSED) return
    this.readyState = CLOSED
    this.relay.forget(this)
    this.onclose?.({ code, reason })
  }
}
