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

export interface WebSocketTransportOptions {
  url: string
  token: string
  client: { name: string; kind: ClientKind }
  /** Makes the socket; defaults to the global `WebSocket`. */
  connect?: (url: string) => SocketLike
  /** How long `start` waits for the welcome and the engine's `ready`. */
  timeoutMs?: number
}

const OPEN = 1

/**
 * A client of a remote host. `start` resolves once the host welcomed it and
 * the engine is ready (from the snapshot, or a later `ready`).
 */
export class WebSocketTransport implements EngineTransport {
  readonly echoesActions = true
  host: HostInfo | null = null
  clients = 0
  /** The last `seq` applied; older or repeated actions are dropped. */
  lastSeq = -1

  private socket: SocketLike | null = null
  private listeners = new Set<(action: SessionAction) => void>()
  private snapshotListeners = new Set<(state: SessionState) => void>()
  private closeListeners = new Set<(close: TransportClose) => void>()
  private terminalListeners = new Set<(frame: Extract<HostFrame, { t: 'term_output' | 'term_exit' }>) => void>()
  private rpcs = new Map<number, { resolve: (value: unknown) => void; reject: (error: Error) => void }>()
  private nextRpc = 1
  private closedByUs = false

  constructor(private readonly options: WebSocketTransportOptions) {}

  /** A snapshot replaces the client's state (on every attach and reconnect). */
  onSnapshot(listener: (state: SessionState) => void): () => void {
    this.snapshotListeners.add(listener)
    return () => this.snapshotListeners.delete(listener)
  }

  start(): Promise<void> {
    const connect =
      this.options.connect ?? ((url: string) => new (globalThis as unknown as { WebSocket: new (url: string) => SocketLike }).WebSocket(url))
    this.closedByUs = false
    return new Promise<void>((resolve, reject) => {
      let settled = false
      const settle = (error?: Error) => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        if (error) reject(error)
        else resolve()
      }
      const timer = setTimeout(() => {
        settle(new Error('The Neuron host did not answer in time.'))
        socket.close()
      }, this.options.timeoutMs ?? 30_000)

      const socket = connect(this.options.url)
      this.socket = socket
      socket.onopen = () => {
        const hello: ClientFrame = {
          t: 'hello',
          protocol: REMOTE_PROTOCOL,
          token: this.options.token,
          client: this.options.client,
        }
        socket.send(JSON.stringify(hello))
      }
      socket.onmessage = (message) => {
        const frame = parseHostFrame(message.data)
        if (!frame) return
        switch (frame.t) {
          case 'welcome':
            this.host = frame.host
            this.clients = frame.clients
            break
          case 'snapshot':
            this.lastSeq = frame.seq
            for (const listener of [...this.snapshotListeners]) listener(frame.state)
            if (frame.state.connection === 'ready') settle()
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
            if (frame.fatal) settle(new Error(frame.message))
            break
          case 'pong':
            break
        }
      }
      socket.onerror = () => settle(new Error(`Could not reach the Neuron host at ${this.options.url}.`))
      socket.onclose = (event) => {
        settle(new Error(event.reason || 'The Neuron host closed the connection.'))
        this.socket = null
        for (const pending of this.rpcs.values()) pending.reject(new Error('The Neuron host closed the connection.'))
        this.rpcs.clear()
        const close: TransportClose = { code: event.code, error: this.closedByUs ? null : event.reason || 'Disconnected from the Neuron host.' }
        for (const listener of [...this.closeListeners]) listener(close)
      }
    })
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
    this.socket?.close(1000, 'client left')
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
