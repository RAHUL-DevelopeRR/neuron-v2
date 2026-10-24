/**
 * The relay's routing, without a server around it: one room per Zero-X
 * account, holding its computers ("hosts"), phones, pairings, channels and
 * replay buffers. ./relay.ts serves it over WebSocket for development and the
 * tests; the zero-x.live Durable Object (WP5) implements the same rules
 * (docs/RELAY_CONTRACT.md §4–§6), one object per account.
 *
 * It never sees a key: pairing frames carry public keys and proofs, `data`
 * frames are sealed end to end, and presence is sealed with the computer's
 * presence key.
 */

import { createHash } from 'crypto'

import {
  CloseCode,
  LIMITS,
  RELAY_PROTOCOL,
  parseDeviceFrame,
  type DeviceInfo,
  type HostToRelay,
  type NotifyKind,
  type PhoneToRelay,
  type Role,
} from '../src/relay/contract'

export interface RelayAccount {
  id: string
  email: string
}

/** What a server hands the room for one socket. */
export interface RelayPeer {
  send(text: string): void
  close(code: number, reason: string): void
}

export interface PushRequest {
  accountId: string
  /** Paired phones to notify (the push service looks up their tokens and switches). */
  phoneIds: string[]
  kind: NotifyKind
  hostId: string
  hostName: string
  shareId: string
}

export interface HubOptions {
  needsMeDelayMs?: number
  /** How long the same push (share and kind) is not repeated. */
  pushDedupMs?: number
  channelAwayMs?: number
  replayFrames?: number
  onPush?: (push: PushRequest) => void
  /** Sees every frame in and out (the tests check that no plaintext passes). */
  tap?: (direction: 'in' | 'out', role: Role | null, text: string) => void
}

/** The tag a pairing link carries to show which account a computer is on: not reversible to the account id. */
export const accountTag = (accountId: string) =>
  createHash('sha256').update(`neuron-relay/1 account|${accountId}`).digest('hex').slice(0, 16)

interface Device {
  role: Role
  info: DeviceInfo
  peer: RelayPeer | null
}

interface Channel {
  ch: string
  hostId: string
  phoneId: string
  shareId: string
  /** Computer-to-phone frames kept for `resume`. */
  buffer: Array<{ n: number; text: string }>
  bytes: number
  /** The last phone counter passed to the computer. */
  lastP2H: number
  accepted: boolean
  phoneAttached: boolean
  awayTimer: ReturnType<typeof setTimeout> | null
}

class RelayRoom {
  readonly tag: string
  private devices = new Map<string, Device>()
  private hosts = new Map<string, { name: string; shares: Map<string, { sealed: string | null; at: number }> }>()
  /** `hostId|phoneId`. A Durable Object keeps these in storage; here they live as long as the process. */
  private pairings = new Set<string>()
  private offers = new Map<string, { hostId: string; expiresAt: number }>()
  private pendingPairs = new Map<string, string>()
  private channels = new Map<string, Channel>()
  private pushTimers = new Map<string, ReturnType<typeof setTimeout>>()
  private lastPush = new Map<string, number>()

  constructor(
    readonly account: RelayAccount,
    private readonly options: HubOptions,
  ) {
    this.tag = accountTag(account.id)
  }

  private out(deviceId: string, frame: Record<string, unknown>): boolean {
    const device = this.devices.get(deviceId)
    if (!device?.peer) return false
    const text = JSON.stringify(frame)
    this.options.tap?.('out', device.role, text)
    device.peer.send(text)
    return true
  }

  private error(deviceId: string, code: string, message: string, extra: Record<string, unknown> = {}): void {
    this.out(deviceId, { t: 'error', code, message, ...extra })
  }

  private paired(hostId: string, phoneId: string): boolean {
    return this.pairings.has(`${hostId}|${phoneId}`)
  }

  private phonesOf(hostId: string): string[] {
    return [...this.pairings].filter((pair) => pair.startsWith(`${hostId}|`)).map((pair) => pair.slice(hostId.length + 1))
  }

  private hostsOf(phoneId: string): string[] {
    return [...this.pairings].filter((pair) => pair.endsWith(`|${phoneId}`)).map((pair) => pair.slice(0, pair.length - phoneId.length - 1))
  }

  private online(deviceId: string): boolean {
    return !!this.devices.get(deviceId)?.peer
  }

  /** A socket's first frame. Returns the device id it joined as, or closes it. */
  join(peer: RelayPeer, text: string): string | null {
    this.options.tap?.('in', null, text)
    const frame = parseDeviceFrame(text, null)
    if (!frame || frame.t !== 'join') {
      peer.close(CloseCode.protocol, 'The first frame must be join.')
      return null
    }
    const { role, device } = frame
    const known = this.devices.get(device.id)
    if (known && known.role !== role) {
      peer.close(CloseCode.protocol, 'This device id belongs to another role.')
      return null
    }
    const count = [...this.devices.values()].filter((other) => other.role === role && other.info.id !== device.id).length
    if (!known && count >= (role === 'host' ? LIMITS.maxHostsPerAccount : LIMITS.maxPhonesPerAccount)) {
      peer.close(CloseCode.limited, role === 'host' ? 'Too many computers on this account.' : 'Too many phones on this account.')
      return null
    }
    // The same device again: the newer socket wins.
    known?.peer?.close(CloseCode.replaced, 'This device connected again.')
    this.devices.set(device.id, { role, info: device, peer })
    this.out(device.id, { t: 'joined', relay: RELAY_PROTOCOL, role, device_id: device.id, account: { tag: this.tag } })
    if (role === 'host') {
      this.hosts.set(device.id, { name: device.name, shares: new Map() })
      for (const phoneId of this.phonesOf(device.id)) this.out(phoneId, { t: 'host', host_id: device.id, name: device.name, online: true })
    }
    return device.id
  }

  message(deviceId: string, text: string): void {
    const device = this.devices.get(deviceId)
    if (!device) return
    this.options.tap?.('in', device.role, text)
    const frame = parseDeviceFrame(text, device.role)
    if (!frame) return this.error(deviceId, 'bad_frame', 'The relay did not understand that frame.')
    if (frame.t === 'ping') return void this.out(deviceId, { t: 'pong' })
    if (frame.t === 'join') return this.error(deviceId, 'bad_frame', 'Already joined.')
    if (device.role === 'host') this.fromHost(deviceId, frame as HostToRelay)
    else this.fromPhone(deviceId, frame as PhoneToRelay)
  }

  left(deviceId: string, peer: RelayPeer): void {
    const device = this.devices.get(deviceId)
    if (!device || device.peer !== peer) return
    device.peer = null
    if (device.role === 'host') {
      for (const channel of [...this.channels.values()]) {
        if (channel.hostId !== deviceId) continue
        this.out(channel.phoneId, { t: 'close', ch: channel.ch, code: 'host_gone' })
        this.dropChannel(channel)
      }
      this.hosts.get(deviceId)?.shares.clear()
      for (const phoneId of this.phonesOf(deviceId)) this.out(phoneId, { t: 'host', host_id: deviceId, name: device.info.name, online: false })
      for (const [pairId, offer] of this.offers) if (offer.hostId === deviceId) this.offers.delete(pairId)
    } else {
      // The phone's socket dropped: its channels wait for it, buffering what the computer sends.
      for (const channel of this.channels.values()) {
        if (channel.phoneId !== deviceId) continue
        channel.phoneAttached = false
        this.out(channel.hostId, { t: 'peer', ch: channel.ch, state: 'away' })
        channel.awayTimer = setTimeout(() => {
          this.out(channel.hostId, { t: 'close', ch: channel.ch, code: 'phone_gone' })
          this.dropChannel(channel)
        }, this.options.channelAwayMs ?? LIMITS.channelAwayMs)
      }
    }
  }

  private dropChannel(channel: Channel): void {
    if (channel.awayTimer) clearTimeout(channel.awayTimer)
    this.channels.delete(channel.ch)
  }

  // ------------------------------------------------------------ computer

  private fromHost(hostId: string, frame: HostToRelay): void {
    const host = this.hosts.get(hostId)!
    switch (frame.t) {
      case 'share': {
        if (!host.shares.has(frame.share_id) && host.shares.size >= LIMITS.maxSharesPerHost) {
          return this.error(hostId, 'limit', 'This computer shares too many sessions.')
        }
        const at = Date.now()
        host.shares.set(frame.share_id, { sealed: frame.sealed, at })
        for (const phoneId of this.phonesOf(hostId)) this.out(phoneId, { t: 'presence', host_id: hostId, share_id: frame.share_id, sealed: frame.sealed, at })
        return
      }
      case 'unshare':
        host.shares.delete(frame.share_id)
        for (const phoneId of this.phonesOf(hostId)) this.out(phoneId, { t: 'presence', host_id: hostId, share_id: frame.share_id, sealed: null, at: Date.now() })
        return
      case 'pair_offer': {
        const live = [...this.offers.values()].filter((offer) => offer.hostId === hostId && offer.expiresAt > Date.now()).length
        if (live >= 5) return this.error(hostId, 'limit', 'Too many pairing codes are waiting.', { pair_id: frame.pair_id })
        const expiresAt = Date.now() + frame.ttl_s * 1000
        this.offers.set(frame.pair_id, { hostId, expiresAt })
        return void this.out(hostId, { t: 'pair_offer_ok', pair_id: frame.pair_id, expires_at: expiresAt })
      }
      case 'pair_cancel':
        if (this.offers.get(frame.pair_id)?.hostId === hostId) this.offers.delete(frame.pair_id)
        return
      case 'paired': {
        if (this.pendingPairs.get(frame.pair_id) !== frame.phone_id) return
        this.pendingPairs.delete(frame.pair_id)
        this.offers.delete(frame.pair_id)
        this.pairings.add(`${hostId}|${frame.phone_id}`)
        this.out(frame.phone_id, { t: 'paired', pair_id: frame.pair_id, host_id: hostId, name: host.name, proof: frame.proof, sealed: frame.sealed })
        this.out(frame.phone_id, { t: 'host', host_id: hostId, name: host.name, online: true })
        for (const [shareId, share] of host.shares) {
          this.out(frame.phone_id, { t: 'presence', host_id: hostId, share_id: shareId, sealed: share.sealed, at: share.at })
        }
        return
      }
      case 'pair_denied':
        if (this.pendingPairs.get(frame.pair_id) !== frame.phone_id) return
        this.pendingPairs.delete(frame.pair_id)
        return void this.out(frame.phone_id, { t: 'pair_denied', pair_id: frame.pair_id, code: frame.code })
      case 'accept':
      case 'reject': {
        const channel = this.channels.get(frame.ch)
        if (!channel || channel.hostId !== hostId) return
        if (frame.t === 'reject') this.dropChannel(channel)
        else channel.accepted = true
        this.out(channel.phoneId, frame)
        if (frame.t === 'accept') this.cancelPush(hostId, channel.shareId)
        return
      }
      case 'data': {
        const channel = this.channels.get(frame.ch)
        if (!channel || channel.hostId !== hostId) return this.error(hostId, 'unknown_channel', 'No such channel.', { ch: frame.ch })
        const text = JSON.stringify(frame)
        channel.buffer.push({ n: frame.n, text })
        channel.bytes += text.length
        const max = this.options.replayFrames ?? LIMITS.replayFrames
        while (channel.buffer.length > max || channel.bytes > LIMITS.replayBytes) channel.bytes -= channel.buffer.shift()!.text.length
        if (channel.phoneAttached) this.out(channel.phoneId, frame)
        return
      }
      case 'close': {
        const channel = this.channels.get(frame.ch)
        if (!channel || channel.hostId !== hostId) return
        this.dropChannel(channel)
        return void this.out(channel.phoneId, { t: 'close', ch: frame.ch, ...(frame.code ? { code: frame.code } : {}) })
      }
      case 'notify':
        return this.notify(hostId, frame.share_id, frame.kind)
    }
  }

  // --------------------------------------------------------------- phone

  private fromPhone(phoneId: string, frame: PhoneToRelay): void {
    switch (frame.t) {
      case 'list': {
        const hosts = this.hostsOf(phoneId).map((hostId) => {
          const host = this.hosts.get(hostId)
          const online = this.online(hostId)
          return {
            host_id: hostId,
            name: host?.name ?? this.devices.get(hostId)?.info.name ?? 'Computer',
            online,
            shares: online && host ? [...host.shares].map(([shareId, share]) => ({ share_id: shareId, sealed: share.sealed, at: share.at })) : [],
          }
        })
        return void this.out(phoneId, { t: 'hosts', hosts })
      }
      case 'pair': {
        const offer = this.offers.get(frame.pair_id)
        if (!offer || offer.expiresAt <= Date.now()) {
          return this.error(phoneId, 'pair_not_found', 'No computer on this account is waiting for that code.', { pair_id: frame.pair_id })
        }
        if (!this.online(offer.hostId)) return this.error(phoneId, 'host_offline', 'The computer went offline.', { pair_id: frame.pair_id })
        if (this.pendingPairs.has(frame.pair_id)) return this.error(phoneId, 'pair_not_found', 'That code is already being used.', { pair_id: frame.pair_id })
        this.pendingPairs.set(frame.pair_id, phoneId)
        return void this.out(offer.hostId, { t: 'pair', pair_id: frame.pair_id, phone_id: phoneId, pub: frame.pub, proof: frame.proof, name: frame.name })
      }
      case 'open': {
        if (!this.paired(frame.host_id, phoneId)) return this.error(phoneId, 'not_paired', 'This phone is not paired with that computer.', { ch: frame.ch })
        if (!this.online(frame.host_id)) return this.error(phoneId, 'host_offline', 'The computer is offline.', { ch: frame.ch })
        if (this.channels.has(frame.ch)) return this.error(phoneId, 'bad_frame', 'That channel id is taken.', { ch: frame.ch })
        this.channels.set(frame.ch, {
          ch: frame.ch,
          hostId: frame.host_id,
          phoneId,
          shareId: frame.share_id,
          buffer: [],
          bytes: 0,
          lastP2H: -1,
          accepted: false,
          phoneAttached: true,
          awayTimer: null,
        })
        return void this.out(frame.host_id, { t: 'open', ch: frame.ch, phone_id: phoneId, share_id: frame.share_id, e: frame.e })
      }
      case 'resume': {
        const channel = this.channels.get(frame.ch)
        if (!channel || channel.phoneId !== phoneId || !channel.accepted) {
          return void this.out(phoneId, { t: 'resume_failed', ch: frame.ch, code: 'unknown_channel' })
        }
        const missed = channel.buffer.filter((entry) => entry.n > frame.after)
        const nextWanted = frame.after + 1
        const oldest = channel.buffer[0]?.n
        // A gap: frames the phone needs are no longer buffered. The channel is over; the phone attaches again.
        if (oldest !== undefined && oldest > nextWanted && missed.length > 0) {
          this.dropChannel(channel)
          this.out(channel.hostId, { t: 'close', ch: frame.ch, code: 'lost' })
          return void this.out(phoneId, { t: 'resume_failed', ch: frame.ch, code: 'gap' })
        }
        if (channel.awayTimer) clearTimeout(channel.awayTimer)
        channel.awayTimer = null
        const wasAway = !channel.phoneAttached
        channel.phoneAttached = true
        this.out(phoneId, { t: 'resumed', ch: frame.ch, received: channel.lastP2H })
        for (const entry of missed) {
          this.options.tap?.('out', 'phone', entry.text)
          this.devices.get(phoneId)?.peer?.send(entry.text)
        }
        if (wasAway) this.out(channel.hostId, { t: 'peer', ch: frame.ch, state: 'back' })
        this.cancelPush(channel.hostId, channel.shareId)
        return
      }
      case 'data': {
        const channel = this.channels.get(frame.ch)
        if (!channel || channel.phoneId !== phoneId) return this.error(phoneId, 'unknown_channel', 'No such channel.', { ch: frame.ch })
        if (!this.online(channel.hostId)) return this.error(phoneId, 'host_offline', 'The computer is offline.', { ch: frame.ch })
        channel.lastP2H = Math.max(channel.lastP2H, frame.n)
        return void this.out(channel.hostId, frame)
      }
      case 'close': {
        const channel = this.channels.get(frame.ch)
        if (!channel || channel.phoneId !== phoneId) return
        this.dropChannel(channel)
        return void this.out(channel.hostId, { t: 'close', ch: frame.ch })
      }
      case 'unpair': {
        if (!this.paired(frame.host_id, phoneId)) return
        this.pairings.delete(`${frame.host_id}|${phoneId}`)
        for (const channel of [...this.channels.values()]) {
          if (channel.hostId === frame.host_id && channel.phoneId === phoneId) {
            this.out(channel.hostId, { t: 'close', ch: channel.ch })
            this.dropChannel(channel)
          }
        }
        this.out(frame.host_id, { t: 'unpaired', phone_id: phoneId })
        return
      }
    }
  }

  // ---------------------------------------------------------------- push

  private attachedTo(hostId: string, shareId: string): boolean {
    return [...this.channels.values()].some((channel) => channel.hostId === hostId && channel.shareId === shareId && channel.phoneAttached)
  }

  private cancelPush(hostId: string, shareId: string): void {
    const key = `${hostId}|${shareId}|needs_me`
    const timer = this.pushTimers.get(key)
    if (timer) clearTimeout(timer)
    this.pushTimers.delete(key)
  }

  private notify(hostId: string, shareId: string, kind: NotifyKind): void {
    // Someone is looking at it: no push.
    if (this.attachedTo(hostId, shareId)) return
    const key = `${hostId}|${shareId}|${kind}`
    if (this.pushTimers.has(key)) return
    const fire = () => {
      this.pushTimers.delete(key)
      if (this.attachedTo(hostId, shareId)) return
      const last = this.lastPush.get(key) ?? 0
      if (Date.now() - last < (this.options.pushDedupMs ?? 60_000)) return
      this.lastPush.set(key, Date.now())
      const phoneIds = this.phonesOf(hostId)
      if (phoneIds.length === 0) return
      this.options.onPush?.({ accountId: this.account.id, phoneIds, kind, hostId, hostName: this.hosts.get(hostId)?.name ?? 'your computer', shareId })
    }
    if (kind === 'finished') return fire()
    this.pushTimers.set(key, setTimeout(fire, this.options.needsMeDelayMs ?? LIMITS.needsMeDelayMs))
  }

  /** Dev and tests: forget every replay buffer, so the next resume falls back to a snapshot. */
  forgetReplay(): void {
    for (const channel of this.channels.values()) {
      channel.buffer = [{ n: Number.MAX_SAFE_INTEGER, text: '' }]
      channel.bytes = 0
    }
  }

  dropDevice(deviceId: string): void {
    this.devices.get(deviceId)?.peer?.close(4000, 'dropped by the relay')
  }

  close(): void {
    for (const timer of this.pushTimers.values()) clearTimeout(timer)
    for (const channel of this.channels.values()) if (channel.awayTimer) clearTimeout(channel.awayTimer)
  }
}

/** All rooms: the server routes each authenticated socket to its account's room. */
export class RelayHub {
  private rooms = new Map<string, RelayRoom>()

  constructor(readonly options: HubOptions = {}) {}

  room(account: RelayAccount): RelayRoom {
    let room = this.rooms.get(account.id)
    if (!room) {
      room = new RelayRoom(account, this.options)
      this.rooms.set(account.id, room)
    }
    return room
  }

  /** A new authenticated socket: feed it frames, and tell it when the socket closes. */
  connect(account: RelayAccount, peer: RelayPeer): { message(text: string): void; closed(): void } {
    const room = this.room(account)
    let deviceId: string | null = null
    const timer = setTimeout(() => {
      if (deviceId === null) peer.close(CloseCode.joinTimeout, 'No join in time.')
    }, LIMITS.joinTimeoutMs)
    return {
      message(text) {
        if (deviceId === null) {
          deviceId = room.join(peer, text)
          if (deviceId) clearTimeout(timer)
          return
        }
        room.message(deviceId, text)
      },
      closed() {
        clearTimeout(timer)
        if (deviceId) room.left(deviceId, peer)
      },
    }
  }

  forgetReplay(): void {
    for (const room of this.rooms.values()) room.forgetReplay()
  }

  dropDevice(deviceId: string): void {
    for (const room of this.rooms.values()) room.dropDevice(deviceId)
  }

  close(): void {
    for (const room of this.rooms.values()) room.close()
  }
}
