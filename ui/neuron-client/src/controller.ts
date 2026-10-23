/**
 * A session a UI can hold: an {@link EngineTransport} (a child process, a
 * Tauri sidecar, a WebSocket to a host) plus the reduced {@link SessionState},
 * with a subscribe/getState pair that React's `useSyncExternalStore` reads.
 */

import {
  CAPABILITY_QUESTIONS,
  FEATURE_CAPABILITIES,
  type Attachment,
  type ClientMessage,
  type PermissionDecision,
  commandMessage,
  permissionReplyMessage,
} from './protocol'
import { initialSession, openQuestionItem, reduceSession, type SessionAction, type SessionState } from './session'

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
      transport.subscribe((action) => {
        this.apply(action)
        // An engine of its own hears what this client handles. A remote host tells its
        // engine itself, for all its clients.
        if (action.type === 'ready' && !transport.echoesActions && action.features?.includes(FEATURE_CAPABILITIES)) {
          transport.send(commandMessage({ type: 'capabilities', features: [CAPABILITY_QUESTIONS] }))
        }
      }),
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

  /**
   * A prompt as typed: `/cmd args` goes as a slash command, anything else as a
   * prompt. While the agent waits for an answer, it is the answer, as typed.
   */
  send(input: string, attachments: Attachment[] = []): void {
    const text = input.trim()
    if (!text) return
    const question = openQuestionItem(this.state)
    if (question?.id !== undefined) return this.answer(question.id, text)
    const slash = /^\/(\S+)\s*([\s\S]*)$/.exec(text)
    if (slash && !question) {
      this.slash(slash[1], slash[2].trim())
    } else {
      this.submit(text, attachments)
    }
  }

  /** Answers the agent's question with this id. */
  answer(id: number, text: string): void {
    this.transport.send(commandMessage({ type: 'answer', id, text }))
    this.local({ type: 'local/answered', id, text })
  }

  /** The workspace's saved chats arrive as `state.sessions` (feature `sessions`). */
  listSessions(): void {
    this.transport.send(commandMessage({ type: 'list_sessions' }))
  }

  /** Continues a saved chat; its transcript replaces what is shown (feature `sessions`). */
  resume(session: string): void {
    this.transport.send(commandMessage({ type: 'resume', session }))
  }

  /**
   * A slash command from a menu rather than the message box: not shown as the
   * user's message (the engine still reports it with a notice).
   */
  command(name: string, args = ''): void {
    this.transport.send(commandMessage({ type: 'slash', command: name.replace(/^\//, ''), args }))
  }

  /** A fresh chat in the same engine. */
  newChat(): void {
    this.slash('clear', '--confirm')
  }

  /** Trusts the workspace (feature `trust_workspace`); `state.restartRequired` follows. */
  trustWorkspace(): void {
    this.transport.send(commandMessage({ type: 'trust_workspace' }))
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
