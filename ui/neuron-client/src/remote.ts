/**
 * The remote protocol: one engine, many clients, over WebSocket text frames
 * (JSON). A host owns the engine (`neuron engine --stdio`) and relays it: the
 * desktop app in a browser during development, the mobile app on the LAN, and
 * later the zero-x.live relay all speak it.
 *
 * - The client's first frame is `hello` with the host's pairing token (never in
 *   the URL). The host answers `welcome` and a `snapshot` of the session as it
 *   stands, then every action in order, numbered by `seq`.
 * - Actions are the engine's events plus the clients' own (a prompt someone
 *   typed, a permission someone answered), so every client shows the same
 *   transcript. The host applies the first answer to a permission request and
 *   drops the rest.
 * - A client that reconnects gets a fresh snapshot: the reduced session is
 *   small next to the events behind it, and always right.
 * - Terminals (`term.open`) are for clients on the host's own machine only: a
 *   shell would bypass the engine's permission prompts.
 * - A frame from a peer meets `parseClientMessage` (via {@link parseClientFrame}) first, which passes only
 *   the commands in `PEER_PASSES` (test/protocol.test.ts). Passing is not permission: the host then
 *   refuses more (`HOST_POLICY`, `REMOTE_SLASH` and the rest in node/host.ts). Whoever builds the
 *   zero-x.live relay must apply the host's policy, not only the parser.
 */

import type { TransportClose, EngineTransport } from './controller'
import { parseClientMessage, type ClientMessage } from './protocol'
import type { SessionAction, SessionState } from './session'

export const REMOTE_PROTOCOL = 1

export type ClientKind = 'desktop' | 'mobile' | 'web' | 'cli'

export type ClientFrame =
  | { t: 'hello'; protocol: number; token: string; client: { name: string; kind: ClientKind } }
  | { t: 'send'; message: ClientMessage }
  | { t: 'rpc'; id: number; method: string; params?: Record<string, unknown> }
  /** Keystrokes for a terminal this client opened with `term.open`. */
  | { t: 'term_input'; id: number; data: string }
  | { t: 'term_resize'; id: number; cols: number; rows: number }
  | { t: 'ping' }

export interface HostInfo {
  name: string
  /** The engine's Neuron version, once it is ready. */
  version: string | null
  cwd: string
}

export type HostFrame =
  | { t: 'welcome'; protocol: number; host: HostInfo; clients: number }
  | { t: 'snapshot'; seq: number; state: SessionState }
  | { t: 'action'; seq: number; action: SessionAction }
  | { t: 'rpc_result'; id: number; result?: unknown; error?: string }
  | { t: 'term_output'; id: number; data: string }
  | { t: 'term_exit'; id: number; code: number | null }
  | { t: 'clients'; count: number }
  | { t: 'error'; message: string; fatal: boolean }
  | { t: 'pong' }

/** Parses a client frame from an untrusted peer; `null` when it is not one. */
export function parseClientFrame(text: string): ClientFrame | null {
  let value: unknown
  try {
    value = JSON.parse(text)
  } catch {
    return null
  }
  if (typeof value !== 'object' || value === null) return null
  const frame = value as Record<string, unknown>
  switch (frame.t) {
    case 'hello': {
      const client = (frame.client ?? {}) as Record<string, unknown>
      const kinds: ClientKind[] = ['desktop', 'mobile', 'web', 'cli']
      if (typeof frame.token !== 'string' || typeof frame.protocol !== 'number') return null
      return {
        t: 'hello',
        protocol: frame.protocol,
        token: frame.token,
        client: {
          name: typeof client.name === 'string' ? client.name.slice(0, 80) : 'client',
          kind: kinds.includes(client.kind as ClientKind) ? (client.kind as ClientKind) : 'web',
        },
      }
    }
    case 'send': {
      const message = parseClientMessage(frame.message)
      return message ? { t: 'send', message } : null
    }
    case 'rpc':
      return Number.isSafeInteger(frame.id) && typeof frame.method === 'string'
        ? {
            t: 'rpc',
            id: frame.id as number,
            method: frame.method,
            params: typeof frame.params === 'object' && frame.params !== null ? (frame.params as Record<string, unknown>) : {},
          }
        : null
    case 'term_input':
      return Number.isSafeInteger(frame.id) && typeof frame.data === 'string' ? { t: 'term_input', id: frame.id as number, data: frame.data } : null
    case 'term_resize': {
      const size = (n: unknown) => Number.isSafeInteger(n) && (n as number) > 0 && (n as number) <= 1000
      return Number.isSafeInteger(frame.id) && size(frame.cols) && size(frame.rows)
        ? { t: 'term_resize', id: frame.id as number, cols: frame.cols as number, rows: frame.rows as number }
        : null
    }
    case 'ping':
      return { t: 'ping' }
    default:
      return null
  }
}

/** The part of a WebSocket this transport uses: the browser's, React Native's and Bun's all fit. */
export interface SocketLike {
  readyState: number
  send(data: string): void
  close(code?: number, reason?: string): void
  onopen: ((event: unknown) => void) | null
  onmessage: ((event: { data: unknown }) => void) | null
  onclose: ((event: { code: number; reason: string }) => void) | null
  onerror: ((event: unknown) => void) | null
}

/** The timers and clock the transport uses; tests pass a fake one. */
export interface TransportClock {
  now(): number
  setTimeout(fn: () => void, ms: number): unknown
  clearTimeout(id: unknown): void
  setInterval(fn: () => void, ms: number): unknown
  clearInterval(id: unknown): void
}

export interface ReconnectOptions {
  /** Failed attempts in a row before the transport gives up and is `offline` (default 10). */
  maxAttempts?: number
  /** The first wait (default {@link RECONNECT_BASE_MS}); each failed attempt doubles it. */
  baseMs?: number
  /** The longest wait before jitter (default {@link RECONNECT_CAP_MS}). */
  capMs?: number
}

export interface WebSocketTransportOptions {
  url: string
  token: string
  client: { name: string; kind: ClientKind }
  /** Makes the socket; defaults to the global `WebSocket`. */
  connect?: (url: string) => SocketLike
  /** How long `start` (and each reconnect attempt) waits for the welcome and the engine's `ready`. */
  timeoutMs?: number
  /** How often a connected client sends `ping` (default {@link KEEPALIVE_MS}; 0 turns it off). */
  keepaliveMs?: number
  /** Silence after which the connection counts as dead (default {@link SILENCE_MS}; 0 turns the watchdog off). */
  silenceMs?: number
  /**
   * Reconnects by itself after a connection that once worked drops or goes silent, with jittered
   * exponential backoff. Off by default: without it a drop closes the session, as it always did.
   */
  reconnect?: boolean | ReconnectOptions
  clock?: TransportClock
  /** For the backoff's jitter; `Math.random` by default. */
  random?: () => number
}

/** A connected host answers each keepalive with `pong`, so a healthy connection is never quiet this long. */
export const KEEPALIVE_MS = 15_000
/** Twice the keepalive: silence this long means something between the client and the host holds or dropped the frames. */
export const SILENCE_MS = 30_000
export const RECONNECT_BASE_MS = 1_000
export const RECONNECT_CAP_MS = 30_000
const RECONNECT_MAX_ATTEMPTS = 10

/**
 * Where the connection to the host stands, for a UI to show in words.
 * `reconnecting.attempt` counts from 1; `retryInMs` is the wait before it (0 while it runs).
 */
export type ConnectionHealth =
  | { state: 'connecting' }
  | { state: 'connected' }
  | { state: 'reconnecting'; attempt: number; retryInMs: number }
  | { state: 'offline'; reason: string | null }

/** A connection the transport abandoned because it went silent. */
export interface ConnectionStall {
  /** `open`: the host had not welcomed this client yet; `stream`: it had, then the frames stopped. */
  phase: 'open' | 'stream'
  silentMs: number
  connectedMs: number
  frames: number
}

/**
 * The wait before reconnect attempt `attempt` (from 1): doubling from the base up to the cap,
 * plus up to a fifth of jitter so many clients of one host do not all come back at once.
 * The pattern follows Freebuff's `CloudRunReader` (common/src/desktop-cloud/run-stream.ts, commit 0dc95cb,
 * Copyright Codebuff, Apache License 2.0; see NOTICE): there a stream reader, here a WebSocket.
 */
export function reconnectDelay(attempt: number, random: () => number = Math.random, baseMs = RECONNECT_BASE_MS, capMs = RECONNECT_CAP_MS): number {
  return Math.min(capMs, baseMs * 2 ** Math.max(0, attempt - 1)) * (1 + random() * 0.2)
}

const OPEN = 1
/** Close codes the host uses when it refuses this client: trying again would be refused again. */
const REFUSED_CODES = new Set([4400, 4401, 4426])

const realClock: TransportClock = {
  now: () => Date.now(),
  setTimeout: (fn, ms) => setTimeout(fn, ms),
  clearTimeout: (id) => clearTimeout(id as ReturnType<typeof setTimeout>),
  setInterval: (fn, ms) => {
    const id = setInterval(fn, ms)
    // A keepalive must not hold a process open on its own (Bun, Node).
    ;(id as { unref?: () => void }).unref?.()
    return id
  },
  clearInterval: (id) => clearInterval(id as ReturnType<typeof setInterval>),
}

/**
 * A client of a remote host. `start` resolves once the host welcomed it and
 * the engine is ready (from the snapshot, or a later `ready`).
 *
 * Connection health: once welcomed it sends `ping` every {@link KEEPALIVE_MS}
 * (the host answers `pong`), and a watchdog drops a connection that stays silent
 * for {@link SILENCE_MS}. With `reconnect`, a connection that worked once comes
 * back by itself with jittered backoff, and the host's snapshot puts the session
 * right again; `health` and `onHealth` say where it stands.
 */
export class WebSocketTransport implements EngineTransport {
  readonly echoesActions = true
  host: HostInfo | null = null
  clients = 0
  /** The last `seq` applied; older or repeated actions are dropped. */
  lastSeq = -1
  /** The last connection the watchdog abandoned, if any. */
  lastStall: ConnectionStall | null = null

  private socket: SocketLike | null = null
  private listeners = new Set<(action: SessionAction) => void>()
  private snapshotListeners = new Set<(state: SessionState) => void>()
  private closeListeners = new Set<(close: TransportClose) => void>()
  private healthListeners = new Set<(health: ConnectionHealth) => void>()
  private stallListeners = new Set<(stall: ConnectionStall) => void>()
  private terminalListeners = new Set<(frame: Extract<HostFrame, { t: 'term_output' | 'term_exit' }>) => void>()
  private rpcs = new Map<number, { resolve: (value: unknown) => void; reject: (error: Error) => void }>()
  private nextRpc = 1
  private closedByUs = false
  private current: ConnectionHealth = { state: 'offline', reason: null }
  private readonly clock: TransportClock
  /** Each socket's number: events of an abandoned socket are ignored. */
  private generation = 0
  private everConnected = false
  private welcomed = false
  private attempt = 0
  /** Why the host refused this client, when it did: such a refusal is not retried. */
  private refused: string | null = null
  private retryTimer: unknown = null
  private keepalive: unknown = null
  private watchdog: unknown = null
  private openedAt = 0
  private lastFrameAt = 0
  private frames = 0

  constructor(private readonly options: WebSocketTransportOptions) {
    this.clock = options.clock ?? realClock
  }

  /** A snapshot replaces the client's state (on every attach and reconnect). */
  onSnapshot(listener: (state: SessionState) => void): () => void {
    this.snapshotListeners.add(listener)
    return () => this.snapshotListeners.delete(listener)
  }

  get health(): ConnectionHealth {
    return this.current
  }

  onHealth(listener: (health: ConnectionHealth) => void): () => void {
    this.healthListeners.add(listener)
    return () => this.healthListeners.delete(listener)
  }

  /** Told each time the watchdog abandons a silent connection. */
  onStall(listener: (stall: ConnectionStall) => void): () => void {
    this.stallListeners.add(listener)
    return () => this.stallListeners.delete(listener)
  }

  /** The current connection, for a diagnostics line. */
  stats(): { phase: 'open' | 'stream'; frames: number; lastFrameAgeMs: number; connectedMs: number } {
    const now = this.clock.now()
    return { phase: this.welcomed ? 'stream' : 'open', frames: this.frames, lastFrameAgeMs: now - this.lastFrameAt, connectedMs: now - this.openedAt }
  }

  private setHealth(health: ConnectionHealth): void {
    this.current = health
    for (const listener of [...this.healthListeners]) listener(health)
  }

  private get reconnectOptions(): Required<ReconnectOptions> | null {
    const option = this.options.reconnect
    if (!option) return null
    const given = option === true ? {} : option
    return {
      maxAttempts: given.maxAttempts ?? RECONNECT_MAX_ATTEMPTS,
      baseMs: given.baseMs ?? RECONNECT_BASE_MS,
      capMs: given.capMs ?? RECONNECT_CAP_MS,
    }
  }

  start(): Promise<void> {
    this.closedByUs = false
    this.refused = null
    this.setHealth({ state: 'connecting' })
    return new Promise<void>((resolve, reject) => {
      this.open((error) => {
        if (error) return reject(error)
        this.connected()
        resolve()
      })
    })
  }

  /** Opens one socket. `done` hears once: welcomed (and, the first time, the engine ready), or why not. */
  private open(done: (error?: Error) => void, afterDrop = false): void {
    const connect =
      this.options.connect ?? ((url: string) => new (globalThis as unknown as { WebSocket: new (url: string) => SocketLike }).WebSocket(url))
    const generation = ++this.generation
    let settled = false
    const settle = (error?: Error) => {
      if (settled) return
      settled = true
      this.clock.clearTimeout(timer)
      done(error)
    }
    const timer = this.clock.setTimeout(() => {
      settle(new Error('The Neuron host did not answer in time.'))
      this.abandon(generation, 'The Neuron host did not answer in time.')
    }, this.options.timeoutMs ?? 30_000)

    this.welcomed = false
    this.frames = 0
    this.openedAt = this.lastFrameAt = this.clock.now()
    const socket = connect(this.options.url)
    this.socket = socket
    this.watch(generation, settle)
    socket.onopen = () => {
      if (generation !== this.generation) return
      const hello: ClientFrame = {
        t: 'hello',
        protocol: REMOTE_PROTOCOL,
        token: this.options.token,
        client: this.options.client,
      }
      socket.send(JSON.stringify(hello))
    }
    socket.onmessage = (message) => {
      if (generation !== this.generation) return
      this.lastFrameAt = this.clock.now()
      this.frames += 1
      const frame = parseHostFrame(message.data)
      if (!frame) return
      switch (frame.t) {
        case 'welcome':
          this.host = frame.host
          this.clients = frame.clients
          this.welcomed = true
          this.startKeepalive(generation)
          break
        case 'snapshot':
          this.lastSeq = frame.seq
          for (const listener of [...this.snapshotListeners]) listener(frame.state)
          // Back after a drop, the session is right again whatever the engine is doing.
          if (frame.state.connection === 'ready' || afterDrop) settle()
          break
        case 'action':
          if (frame.seq <= this.lastSeq) return
          this.lastSeq = frame.seq
          for (const listener of [...this.listeners]) listener(frame.action)
          if (frame.action.type === 'ready') settle()
          break
        case 'term_output':
        case 'term_exit':
          for (const listener of [...this.terminalListeners]) listener(frame)
          break
        case 'clients':
          this.clients = frame.count
          break
        case 'rpc_result': {
          const pending = this.rpcs.get(frame.id)
          this.rpcs.delete(frame.id)
          if (frame.error !== undefined) pending?.reject(new Error(frame.error))
          else pending?.resolve(frame.result)
          break
        }
        case 'error':
          if (frame.fatal) {
            this.refused = frame.message
            settle(new Error(frame.message))
          }
          break
        case 'pong':
          break
      }
    }
    socket.onerror = () => {
      if (generation !== this.generation) return
      settle(new Error(`Could not reach the Neuron host at ${this.options.url}.`))
    }
    socket.onclose = (event) => {
      if (generation !== this.generation) return
      settle(new Error(event.reason || 'The Neuron host closed the connection.'))
      this.closed(event.code, event.reason || null)
    }
  }

  /** The silence watchdog for one socket (after Freebuff's `CloudRunReader.watch`). */
  private watch(generation: number, settle: (error?: Error) => void): void {
    const silenceMs = this.options.silenceMs ?? SILENCE_MS
    if (silenceMs <= 0) return
    this.watchdog = this.clock.setInterval(
      () => {
        if (generation !== this.generation || !this.socket) return
        const now = this.clock.now()
        if (now - this.lastFrameAt < silenceMs) return
        const stall: ConnectionStall = {
          phase: this.welcomed ? 'stream' : 'open',
          silentMs: now - this.lastFrameAt,
          connectedMs: now - this.openedAt,
          frames: this.frames,
        }
        this.lastStall = stall
        for (const listener of [...this.stallListeners]) {
          try {
            listener(stall)
          } catch {
            // Diagnostics cannot alter the transport.
          }
        }
        settle(new Error('The Neuron host stopped answering.'))
        this.abandon(generation, 'The Neuron host stopped answering.')
      },
      Math.max(250, Math.min(5000, silenceMs / 6)),
    )
  }

  private startKeepalive(generation: number): void {
    const keepaliveMs = this.options.keepaliveMs ?? KEEPALIVE_MS
    if (keepaliveMs <= 0) return
    this.clock.clearInterval(this.keepalive)
    this.keepalive = this.clock.setInterval(() => {
      if (generation !== this.generation) return
      this.ping()
    }, keepaliveMs)
  }

  private ping(): void {
    try {
      if (this.socket?.readyState === OPEN) this.socket.send(JSON.stringify({ t: 'ping' } satisfies ClientFrame))
    } catch {
      // A socket that cannot send is caught by the watchdog.
    }
  }

  private stopTimers(): void {
    this.clock.clearInterval(this.keepalive)
    this.clock.clearInterval(this.watchdog)
    this.keepalive = this.watchdog = null
  }

  /** Gives up on a socket that will not tell us it closed (silent, or never answered), as if it had. */
  private abandon(generation: number, reason: string): void {
    if (generation !== this.generation || !this.socket) return
    const socket = this.socket
    socket.onopen = socket.onmessage = socket.onerror = socket.onclose = null
    try {
      socket.close(4000, reason)
    } catch {
      // Already gone.
    }
    this.closed(null, reason)
  }

  /** The current socket is gone: fail what waited on it, then reconnect, or tell the session it closed. */
  private closed(code: number | null, reason: string | null): void {
    this.generation += 1
    this.socket = null
    this.welcomed = false
    this.stopTimers()
    for (const pending of this.rpcs.values()) pending.reject(new Error('The Neuron host closed the connection.'))
    this.rpcs.clear()
    if (this.closedByUs) return this.offline(code, null)
    if (code !== null && REFUSED_CODES.has(code) && !this.refused) this.refused = reason ?? 'The Neuron host refused this client.'
    if (this.refused) return this.offline(code, this.refused)
    const retry = this.reconnectOptions
    if (!retry || !this.everConnected) return this.offline(code, reason || 'Disconnected from the Neuron host.')
    this.attempt += 1
    if (this.attempt > retry.maxAttempts) return this.offline(code, reason || 'Disconnected from the Neuron host.')
    const wait = Math.round(reconnectDelay(this.attempt, this.options.random ?? Math.random, retry.baseMs, retry.capMs))
    this.setHealth({ state: 'reconnecting', attempt: this.attempt, retryInMs: wait })
    this.clock.clearTimeout(this.retryTimer)
    this.retryTimer = this.clock.setTimeout(() => {
      this.retryTimer = null
      this.retry()
    }, wait)
  }

  private offline(code: number | null, error: string | null): void {
    this.clock.clearTimeout(this.retryTimer)
    this.retryTimer = null
    this.setHealth({ state: 'offline', reason: error })
    const close: TransportClose = { code, error }
    for (const listener of [...this.closeListeners]) listener(close)
  }

  private connected(): void {
    this.everConnected = true
    this.attempt = 0
    this.setHealth({ state: 'connected' })
  }

  /** One reconnect attempt; a failure comes back through `closed`, which plans the next. */
  private retry(): void {
    if (this.closedByUs) return
    this.open((error) => {
      if (!error) this.connected()
    }, true)
  }

  /**
   * Tries again now: skips the wait of a reconnect, or starts again after the transport gave up.
   * Does nothing while connected, while an attempt runs, or after `close`.
   */
  reconnectNow(): void {
    if (this.closedByUs || !this.everConnected) return
    const state = this.current.state
    if (state === 'connected') return
    if (state === 'reconnecting' && this.retryTimer === null) return
    if (state === 'offline') {
      this.attempt = 0
      this.refused = null
    }
    this.clock.clearTimeout(this.retryTimer)
    this.retryTimer = null
    this.setHealth({ state: 'reconnecting', attempt: Math.max(1, this.attempt), retryInMs: 0 })
    this.retry()
  }

  /**
   * The app came back to the foreground. A connection quiet for longer than a keepalive
   * has most likely died while the app slept: it is replaced at once. A fresh one is pinged.
   */
  wake(): void {
    if (this.closedByUs) return
    if (this.current.state !== 'connected') return this.reconnectNow()
    const keepaliveMs = this.options.keepaliveMs ?? KEEPALIVE_MS
    if (this.clock.now() - this.lastFrameAt > keepaliveMs && this.reconnectOptions) {
      this.abandon(this.generation, 'The connection went quiet while the app was away.')
      this.reconnectNow()
    } else {
      this.ping()
    }
  }

  private frame(frame: ClientFrame): void {
    if (!this.socket || this.socket.readyState !== OPEN) throw new Error('Not connected to the Neuron host.')
    this.socket.send(JSON.stringify(frame))
  }

  send(message: ClientMessage): void {
    this.frame({ t: 'send', message })
  }

  /** Calls a host method (`fs.list`, `fs.read`, ...). */
  rpc<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    const id = this.nextRpc++
    return new Promise<T>((resolve, reject) => {
      this.rpcs.set(id, { resolve: resolve as (value: unknown) => void, reject })
      try {
        this.frame({ t: 'rpc', id, method, params })
      } catch (error) {
        this.rpcs.delete(id)
        reject(error as Error)
      }
    })
  }

  /** Output and exits of the terminals this client opened (`rpc('term.open', {cols, rows})`). */
  onTerminal(listener: (frame: Extract<HostFrame, { t: 'term_output' | 'term_exit' }>) => void): () => void {
    this.terminalListeners.add(listener)
    return () => this.terminalListeners.delete(listener)
  }

  terminalInput(id: number, data: string): void {
    this.frame({ t: 'term_input', id, data })
  }

  terminalResize(id: number, cols: number, rows: number): void {
    this.frame({ t: 'term_resize', id, cols, rows })
  }

  subscribe(listener: (action: SessionAction) => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  onClose(listener: (close: TransportClose) => void): () => void {
    this.closeListeners.add(listener)
    return () => this.closeListeners.delete(listener)
  }

  /** Leaves the session. The engine keeps running on the host for the other clients. */
  async close(): Promise<void> {
    this.closedByUs = true
    this.clock.clearTimeout(this.retryTimer)
    this.retryTimer = null
    if (this.socket) {
      this.socket.close(1000, 'client left')
    } else if (this.current.state === 'reconnecting') {
      // Between attempts there is no socket to close: the session ends here.
      this.stopTimers()
      this.offline(1000, null)
    }
  }
}

export function parseHostFrame(data: unknown): HostFrame | null {
  if (typeof data !== 'string') return null
  try {
    const value = JSON.parse(data)
    return value && typeof value === 'object' && typeof value.t === 'string' ? (value as HostFrame) : null
  } catch {
    return null
  }
}
