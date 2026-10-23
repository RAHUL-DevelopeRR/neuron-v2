/**
 * A shared session on a paired computer, as an `EngineTransport` for the
 * shared client's `SessionController`. Remote protocol 1 runs unchanged over
 * an encrypted relay channel, in @neuron/client's own `WebSocketTransport`.
 *
 * Reconnecting has two levels:
 * - the relay socket drops: the channel resumes from its last frame
 *   (./phone.ts), and the session never notices;
 * - the channel is lost (the relay could not resume, the computer restarted
 *   its share): this transport attaches again with backoff, and the host's
 *   fresh snapshot replaces the state. Only when that keeps failing does the
 *   controller hear a close.
 */

import {
  WebSocketTransport,
  type ClientMessage,
  type EngineTransport,
  type HostInfo,
  type SessionAction,
  type SessionState,
  type TransportClose,
} from '@neuron/client'

import { ChannelClose, type PhoneRelay } from './phone'

export type AttachState = 'attaching' | 'attached' | 'reattaching' | 'closed'

export interface RelayTransportOptions {
  relay: PhoneRelay
  hostId: string
  shareId: string
  client?: { name: string; kind: 'mobile' }
  /** How long one attach waits for the snapshot. */
  timeoutMs?: number
  /** Reattach attempts before the session reports itself disconnected. */
  maxReattach?: number
  reattachBaseMs?: number
}

/** Channel closes that another attach will not fix. */
const FINAL = new Set<number>([ChannelClose.untrusted, ChannelClose.refused])

export class RelayEngineTransport implements EngineTransport {
  readonly echoesActions = true
  attachState: AttachState = 'attaching'
  /** Snapshots received: one per attach (resumes do not need one). */
  snapshots = 0
  private inner: WebSocketTransport | null = null
  private listeners = new Set<(action: SessionAction) => void>()
  private snapshotListeners = new Set<(state: SessionState) => void>()
  private closeListeners = new Set<(close: TransportClose) => void>()
  private attachListeners = new Set<(state: AttachState) => void>()
  private closing = false
  private failures = 0
  private retry: ReturnType<typeof setTimeout> | null = null

  constructor(private readonly options: RelayTransportOptions) {}

  get host(): HostInfo | null {
    return this.inner?.host ?? null
  }

  get clients(): number {
    return this.inner?.clients ?? 0
  }

  get lastSeq(): number {
    return this.inner?.lastSeq ?? -1
  }

  onAttach(listener: (state: AttachState) => void): () => void {
    this.attachListeners.add(listener)
    return () => this.attachListeners.delete(listener)
  }

  private setAttach(state: AttachState): void {
    if (state === this.attachState) return
    this.attachState = state
    for (const listener of [...this.attachListeners]) listener(state)
  }

  async start(): Promise<void> {
    this.closing = false
    this.setAttach('attaching')
    await this.attach()
  }

  private attach(): Promise<void> {
    const { relay, hostId, shareId } = this.options
    const inner = new WebSocketTransport({
      url: `relay://${hostId}/${shareId}`,
      // The channel is the credential: the computer's side replaces this before its host sees it.
      token: 'e2e',
      client: this.options.client ?? { name: 'Neuron Mobile', kind: 'mobile' },
      connect: () => relay.openChannel(hostId, shareId),
      timeoutMs: this.options.timeoutMs ?? 20_000,
    })
    this.inner = inner
    inner.subscribe((action) => {
      if (this.inner === inner) for (const listener of [...this.listeners]) listener(action)
    })
    inner.onSnapshot((state) => {
      if (this.inner !== inner) return
      this.snapshots++
      for (const listener of [...this.snapshotListeners]) listener(state)
    })
    inner.onClose((close) => this.innerClosed(inner, close))
    return inner.start().then(() => {
      if (this.inner !== inner) return
      this.failures = 0
      this.setAttach('attached')
    })
  }

  private innerClosed(inner: WebSocketTransport, close: TransportClose): void {
    if (this.inner !== inner) return
    if (this.closing) return this.report(close)
    const final = close.code !== null && FINAL.has(close.code)
    const max = this.options.maxReattach ?? 6
    if (final || this.failures >= max || this.attachState === 'attaching') return this.report(close)
    this.failures++
    this.setAttach('reattaching')
    const delay = Math.min(15_000, (this.options.reattachBaseMs ?? 500) * 2 ** (this.failures - 1))
    this.retry = setTimeout(() => {
      this.retry = null
      if (this.closing) return
      // Wait for the relay link when it is down; the channel opens once it rejoins.
      this.attach().catch(() => {
        // innerClosed already ran for this attempt and scheduled the next one, or reported.
      })
    }, delay)
  }

  private report(close: TransportClose): void {
    this.setAttach('closed')
    for (const listener of [...this.closeListeners]) listener(close)
  }

  /** Attach again now (the Retry button after the session reported a close). */
  async reconnect(): Promise<void> {
    if (this.retry) clearTimeout(this.retry)
    this.retry = null
    this.failures = 0
    this.closing = false
    this.setAttach('reattaching')
    await this.attach()
  }

  send(message: ClientMessage): void {
    if (!this.inner) throw new Error('Not attached to the session.')
    this.inner.send(message)
  }

  rpc<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    if (!this.inner) return Promise.reject(new Error('Not attached to the session.'))
    return this.inner.rpc<T>(method, params)
  }

  subscribe(listener: (action: SessionAction) => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  onSnapshot(listener: (state: SessionState) => void): () => void {
    this.snapshotListeners.add(listener)
    return () => this.snapshotListeners.delete(listener)
  }

  onClose(listener: (close: TransportClose) => void): () => void {
    this.closeListeners.add(listener)
    return () => this.closeListeners.delete(listener)
  }

  /** Leaves the session; the computer keeps running it. */
  async close(): Promise<void> {
    this.closing = true
    if (this.retry) clearTimeout(this.retry)
    this.retry = null
    await this.inner?.close()
    this.setAttach('closed')
  }
}
