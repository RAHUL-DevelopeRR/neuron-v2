/**
 * The relay socket of one device: it connects, joins, keeps the socket alive
 * with pings and reconnects with backoff (./reconnect.ts). Both ends use it:
 * the phone (./phone.ts) and the computer side (./share-host.ts, which
 * `neuron engine --share` mirrors in Rust).
 *
 * The session token goes in a WebSocket subprotocol (`neuron-auth.<token>`),
 * never in the URL: browsers cannot set an Authorization header on a
 * WebSocket, and the relay needs the account before it picks the room.
 */

import {
  AUTH_SUBPROTOCOL_PREFIX,
  LIMITS,
  RELAY_SUBPROTOCOL,
  parseRelayFrame,
  type DeviceInfo,
  type Role,
} from './contract'
import { DEFAULT_BACKOFF, initialReconnect, stepReconnect, type BackoffOptions, type ReconnectEffect, type ReconnectEvent, type ReconnectState } from './reconnect'

/** The part of a WebSocket the link uses (browser, React Native and Bun sockets fit). */
export interface RelaySocket {
  readyState: number
  send(data: string): void
  close(code?: number, reason?: string): void
  onopen: ((event: unknown) => void) | null
  onmessage: ((event: { data: unknown }) => void) | null
  onclose: ((event: { code: number; reason: string }) => void) | null
  onerror: ((event: unknown) => void) | null
}

export type SocketFactory = (url: string, protocols: string[]) => RelaySocket

export interface Timers {
  setTimeout(callback: () => void, ms: number): unknown
  clearTimeout(handle: unknown): void
  now(): number
}

const realTimers: Timers = {
  setTimeout: (callback, ms) => setTimeout(callback, ms),
  clearTimeout: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
  now: () => Date.now(),
}

export interface RelayLinkOptions {
  url: string
  role: Role
  device: DeviceInfo
  /** The Zero-X session token (`ses_…`), read at each connect. Never logged. */
  token: () => string | null
  connect?: SocketFactory
  backoff?: Partial<BackoffOptions>
  /** How often to ping an idle socket. */
  heartbeatMs?: number
  /** A socket that answers no ping this long is dead. */
  pongTimeoutMs?: number
  timers?: Timers
}

const OPEN = 1

const defaultConnect: SocketFactory = (url, protocols) =>
  new (globalThis as unknown as { WebSocket: new (url: string, protocols: string[]) => RelaySocket }).WebSocket(url, protocols)

export interface RelayFrame {
  t: string
  [key: string]: unknown
}

export class RelayLink {
  state: ReconnectState = initialReconnect()
  /** The account's tag, from `joined`. */
  accountTag: string | null = null
  private socket: RelaySocket | null = null
  private joined = false
  private timer: unknown = null
  private heartbeat: unknown = null
  private pongDeadline: unknown = null
  private joinDeadline: unknown = null
  private frameListeners = new Set<(frame: RelayFrame) => void>()
  private statusListeners = new Set<(state: ReconnectState) => void>()
  private readyListeners = new Set<() => void>()
  private readonly backoff: BackoffOptions
  private readonly timers: Timers

  constructor(readonly options: RelayLinkOptions) {
    this.backoff = { ...DEFAULT_BACKOFF, ...options.backoff }
    this.timers = options.timers ?? realTimers
  }

  /** Joined and ready for frames. */
  get ready(): boolean {
    return this.joined && this.socket?.readyState === OPEN
  }

  onFrame(listener: (frame: RelayFrame) => void): () => void {
    this.frameListeners.add(listener)
    return () => this.frameListeners.delete(listener)
  }

  onStatus(listener: (state: ReconnectState) => void): () => void {
    this.statusListeners.add(listener)
    return () => this.statusListeners.delete(listener)
  }

  /** After each `joined` (the first connect and every reconnect). */
  onReady(listener: () => void): () => void {
    this.readyListeners.add(listener)
    return () => this.readyListeners.delete(listener)
  }

  start(): void {
    this.dispatch({ type: 'start' })
  }

  stop(): void {
    this.dispatch({ type: 'stop' })
  }

  /** Tell the link the device went online or offline (NetInfo, `online`/`offline` events). */
  setOnline(online: boolean): void {
    this.dispatch({ type: online ? 'online' : 'offline' })
  }

  /** The app came back to the foreground. */
  foreground(): void {
    this.dispatch({ type: 'foreground' })
  }

  /** Sends a frame; `false` when the link is not joined (the caller keeps it for later). */
  send(frame: RelayFrame): boolean {
    if (!this.ready) return false
    this.socket!.send(JSON.stringify(frame))
    return true
  }

  /** Drops the socket as if the network did (tests, and a socket that stopped answering). */
  drop(): void {
    this.socket?.close(4000, 'dropped')
  }

  private dispatch(event: ReconnectEvent): void {
    const { state, effects } = stepReconnect(this.state, event, this.backoff)
    const changed = state !== this.state
    this.state = state
    for (const effect of effects) this.run(effect)
    // An effect may have stepped the machine again (a connect that failed at once): report where it ended.
    if (changed) for (const listener of [...this.statusListeners]) listener(this.state)
  }

  private run(effect: ReconnectEffect): void {
    switch (effect.type) {
      case 'connect':
        return this.connect()
      case 'schedule':
        this.cancelTimer()
        this.timer = this.timers.setTimeout(() => {
          this.timer = null
          this.dispatch({ type: 'timer' })
        }, effect.ms)
        return
      case 'cancel_timer':
        return this.cancelTimer()
      case 'resume':
        for (const listener of [...this.readyListeners]) listener()
        return
      case 'disconnect':
        return this.disconnect()
    }
  }

  private cancelTimer(): void {
    if (this.timer !== null) this.timers.clearTimeout(this.timer)
    this.timer = null
  }

  private clearLiveness(): void {
    for (const handle of [this.heartbeat, this.pongDeadline, this.joinDeadline]) if (handle !== null) this.timers.clearTimeout(handle)
    this.heartbeat = this.pongDeadline = this.joinDeadline = null
  }

  private disconnect(): void {
    const socket = this.socket
    this.socket = null
    this.joined = false
    this.clearLiveness()
    if (socket) {
      socket.onclose = socket.onmessage = socket.onerror = socket.onopen = null
      socket.close(1000, 'left')
    }
  }

  private connect(): void {
    this.disconnect()
    const token = this.options.token()
    if (!token) {
      this.dispatch({ type: 'closed', code: 4401, reason: 'Not signed in to Zero-X.', at: this.timers.now() })
      return
    }
    let socket: RelaySocket
    try {
      socket = (this.options.connect ?? defaultConnect)(this.options.url, [RELAY_SUBPROTOCOL, `${AUTH_SUBPROTOCOL_PREFIX}${token}`])
    } catch (error) {
      this.dispatch({ type: 'closed', code: null, reason: error instanceof Error ? error.message : String(error), at: this.timers.now() })
      return
    }
    this.socket = socket
    socket.onopen = () => {
      socket.send(JSON.stringify({ t: 'join', role: this.options.role, device: this.options.device }))
      this.joinDeadline = this.timers.setTimeout(() => socket.close(4408, 'no joined'), LIMITS.joinTimeoutMs)
    }
    socket.onmessage = (message) => {
      const frame = parseRelayFrame<RelayFrame>(message.data)
      if (!frame) return
      this.alive()
      if (frame.t === 'pong') return
      if (frame.t === 'joined') {
        if (this.joinDeadline !== null) this.timers.clearTimeout(this.joinDeadline)
        this.joinDeadline = null
        this.joined = true
        this.accountTag = typeof (frame.account as { tag?: unknown })?.tag === 'string' ? (frame.account as { tag: string }).tag : null
        this.dispatch({ type: 'opened', at: this.timers.now() })
      }
      for (const listener of [...this.frameListeners]) listener(frame)
    }
    socket.onerror = () => {}
    socket.onclose = (event) => {
      if (this.socket !== socket) return
      this.socket = null
      this.joined = false
      this.clearLiveness()
      this.dispatch({ type: 'closed', code: event.code ?? null, reason: event.reason, at: this.timers.now() })
    }
  }

  /** Any frame proves the socket is alive: push the next ping out. */
  private alive(): void {
    if (this.pongDeadline !== null) this.timers.clearTimeout(this.pongDeadline)
    this.pongDeadline = null
    if (this.heartbeat !== null) this.timers.clearTimeout(this.heartbeat)
    const socket = this.socket
    this.heartbeat = this.timers.setTimeout(() => {
      if (!socket || socket !== this.socket || socket.readyState !== OPEN) return
      socket.send('{"t":"ping"}')
      this.pongDeadline = this.timers.setTimeout(() => socket.close(4000, 'no pong'), this.options.pongTimeoutMs ?? 10_000)
    }, this.options.heartbeatMs ?? 25_000)
  }
}
