/**
 * A session a UI can hold: an {@link EngineTransport} (a child process, a
 * Tauri sidecar, a WebSocket to a host) plus the reduced {@link SessionState},
 * with a subscribe/getState pair that React's `useSyncExternalStore` reads.
 */

import {
  type Attachment,
  type ClientMessage,
  type PermissionDecision,
  commandMessage,
  permissionReplyMessage,
} from './protocol'
import { initialSession, reduceSession, type SessionAction, type SessionState } from './session'

export interface TransportClose {
  code: number | null
  error: string | null
}

export interface EngineTransport {
  /**
   * The far side reports the client's own actions back (a remote host
   * broadcasts them to every client), so the controller must not apply them
   * twice. A local engine reports nothing but its events.
   */
  readonly echoesActions: boolean
  /** Connects (or spawns) and resolves once the engine's `ready` arrived. */
  start(): Promise<void>
  /** Sends one message; throws when the engine is not connected. */
  send(message: ClientMessage): void
  /** Engine events, and for remote hosts the actions of every client. */
  subscribe(listener: (action: SessionAction) => void): () => void
  onClose(listener: (close: TransportClose) => void): () => void
  /** Remote hosts: the session as it stands, replacing the client's state. */
  onSnapshot?(listener: (state: SessionState) => void): () => void
  /** Asks the engine to save and stop (or, for a remote host, leaves), then closes. */
  close(): Promise<void>
}

export class SessionController {
  private state: SessionState = initialSession()
  private listeners = new Set<() => void>()
  private unsubscribers: Array<() => void> = []

  constructor(readonly transport: EngineTransport) {
    this.unsubscribers.push(
      transport.subscribe((action) => this.apply(action)),
      transport.onClose((close) => this.apply({ type: 'local/disconnected', error: close.error })),
    )
    if (transport.onSnapshot) this.unsubscribers.push(transport.onSnapshot((state) => this.replace(state)))
  }

  private replace(state: SessionState): void {
    this.state = state
    for (const listener of [...this.listeners]) listener()
  }

  getState = (): SessionState => this.state

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  /** Applies an action to the state and tells the subscribers. */
  apply(action: SessionAction): void {
    const next = reduceSession(this.state, action)
    if (next === this.state) return
    this.state = next
    for (const listener of [...this.listeners]) listener()
  }

  async start(): Promise<void> {
    this.apply({ type: 'local/connecting' })
    try {
      await this.transport.start()
    } catch (error) {
      this.apply({ type: 'local/disconnected', error: error instanceof Error ? error.message : String(error) })
      throw error
    }
  }

  private local(action: SessionAction): void {
    if (!this.transport.echoesActions) this.apply(action)
  }

  submit(text: string, attachments: Attachment[] = []): void {
    this.transport.send(commandMessage({ type: 'submit', text, attachments }))
    this.local({ type: 'local/submitted', text, attachments })
  }

  /** `/name args`, sent as a slash command. */
  slash(command: string, args = ''): void {
    const name = command.replace(/^\//, '')
    this.transport.send(commandMessage({ type: 'slash', command: name, args }))
    this.local({ type: 'local/slash', command: name, args })
  }

  /** A prompt as typed: `/cmd args` goes as a slash command, anything else as a prompt. */
  send(input: string, attachments: Attachment[] = []): void {
    const text = input.trim()
    if (!text) return
    const slash = /^\/(\S+)\s*([\s\S]*)$/.exec(text)
    // An answer to the agent's question is sent as typed, even when it starts with "/".
    if (slash && !this.state.items.some((item) => item.kind === 'question' && !item.answered)) {
      this.slash(slash[1], slash[2].trim())
    } else {
      this.submit(text, attachments)
    }
  }

  setModel(model: string): void {
    this.transport.send(commandMessage({ type: 'set_model', model }))
  }

  listModels(): void {
    this.transport.send(commandMessage({ type: 'list_models' }))
  }

  interrupt(): void {
    if (this.state.connection !== 'ready') return
    this.transport.send(commandMessage({ type: 'interrupt' }))
  }

  answerPermission(id: number, decision: PermissionDecision): void {
    this.transport.send(permissionReplyMessage({ id, decision }))
    this.local({ type: 'local/permission_answered', id, decision })
  }

  async close(): Promise<void> {
    try {
      await this.transport.close()
    } finally {
      for (const unsubscribe of this.unsubscribers) unsubscribe()
      this.unsubscribers = []
    }
  }
}
