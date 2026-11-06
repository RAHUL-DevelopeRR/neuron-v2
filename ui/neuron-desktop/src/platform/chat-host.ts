/**
 * The chats of one window over Tauri's IPC: one engine, one transport and one
 * SessionController per chat. The app's Rust side sends every engine's lines as
 * `neuron://chat-line {chat, generation, line}` and its end as
 * `neuron://chat-exit {chat, generation, code, error, reason}`; one listener
 * for each routes them by chat. Written against an injected `invoke` and
 * `listen`, so tests drive it without Tauri.
 */

import {
  SessionController,
  parseUiEventLine,
  type ClientMessage,
  type EngineTransport,
  type SessionAction,
  type TransportClose,
} from '@neuron/client'

import { ChatError, type ChatHost, type ChatInfo, type LaunchOptions, type Project, type WorktreeInfo, type WorktreeStatus } from './types'

export interface Ipc {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>
  listen<T>(event: string, handler: (payload: T) => void): Promise<() => void>
}

interface ChatLine {
  chat: string
  generation: number
  line: string
}

interface ChatExit {
  chat: string
  generation: number
  code: number | null
  error: string | null
  reason: string
}

/** What the host returns for a chat. */
interface Summary {
  chat: string
  generation: number
  project: Project
  cwd: string
  branch: string | null
  worktree: WorktreeInfo | null
  running: boolean
  session: string | null
}

/**
 * Lines that arrive before the chat is registered (the host emits while
 * `chat_open` still returns) wait for it, up to this many per chat; past that
 * the oldest go. A chat nobody registers is forgotten after PENDING_TTL_MS.
 */
const BUFFER_LINES = 1000
const PENDING_TTL_MS = 60_000
const PRUNE_EVERY_MS = 30_000

/** An engine that has not said `ready` by then has failed to start. */
const START_TIMEOUT_MS = 30_000

export const NOT_KEEPING_UP = 'The Neuron engine is not keeping up. Try again in a moment.'

const startTimeout = (ms: number) => `The Neuron engine did not start within ${ms >= 1000 ? Math.round(ms / 1000) : ms / 1000} seconds.`

const infoOf = (summary: Summary): ChatInfo => ({
  id: summary.chat,
  project: summary.project,
  cwd: summary.cwd,
  branch: summary.branch,
  worktree: summary.worktree,
})

/** The host's refusals in words, and the codes the app answers itself. */
export function chatError(raw: unknown): ChatError {
  if (raw instanceof ChatError) return raw
  const text = raw instanceof Error ? raw.message : String(raw)
  const count = (prefix: string) => Number(text.slice(prefix.length)) || 0
  if (text === 'cap_reached') return new ChatError('4 chats are working. Stop one to open another.', 'cap_reached')
  if (text === 'too_many_chats') return new ChatError('Too many chats are open. Close one to open another.', 'too_many_chats')
  if (text === 'discarding') return new ChatError("This chat's copy is being removed.", 'discarding')
  if (text === 'starting') return new ChatError('This chat is still starting. Try again in a moment.', 'starting')
  if (text === 'has_worktree') return new ChatError('This chat works in its own copy of the project. Discard the copy to close the chat, or keep it.', 'has_worktree')
  if (text.startsWith('unmerged:')) return new ChatError('This copy has saved changes that the main folder lacks.', 'unmerged', count('unmerged:'))
  if (text.startsWith('dirty:')) return new ChatError('This copy has unsaved changes.', 'dirty', count('dirty:'))
  return new ChatError(text)
}

interface Handler {
  line(payload: ChatLine): void
  exit(payload: ChatExit): void
}

interface Buffered {
  at: number
  run: (handler: Handler) => void
}

class ChatBus {
  private handlers = new Map<string, Handler>()
  private pending = new Map<string, Buffered[]>()
  private stopped = new Set<(id: string, reason: string) => void>()
  private closed = new Set<string>()
  private listening: Promise<void> | null = null

  constructor(
    private readonly ipc: Ipc,
    private readonly now: () => number,
  ) {}

  init(): Promise<void> {
    this.listening ??= (async () => {
      const timer = setInterval(() => this.prune(), PRUNE_EVERY_MS) as unknown as { unref?: () => void }
      timer.unref?.()
      await this.ipc.listen<ChatLine>('neuron://chat-line', (payload) => this.route(payload.chat, { at: this.now(), run: (handler) => handler.line(payload) }))
      await this.ipc.listen<ChatExit>('neuron://chat-exit', (payload) => {
        if (this.closed.has(payload.chat)) return
        this.route(payload.chat, { at: this.now(), run: (handler) => handler.exit(payload) })
        for (const listener of [...this.stopped]) listener(payload.chat, payload.reason)
      })
    })()
    return this.listening
  }

  private route(chat: string, item: Buffered): void {
    // A closed chat's engine may still talk for a moment; nobody is listening.
    if (this.closed.has(chat)) return
    const handler = this.handlers.get(chat)
    if (handler) return item.run(handler)
    this.prune()
    const waiting = this.pending.get(chat) ?? []
    waiting.push(item)
    if (waiting.length > BUFFER_LINES) waiting.splice(0, waiting.length - BUFFER_LINES)
    this.pending.set(chat, waiting)
  }

  /** Forgets what waits for chats that never came. */
  prune(): void {
    const now = this.now()
    for (const [chat, waiting] of this.pending) {
      if (now - (waiting.at(-1)?.at ?? 0) > PENDING_TTL_MS) this.pending.delete(chat)
    }
  }

  register(chat: string, handler: Handler): void {
    this.closed.delete(chat)
    this.handlers.set(chat, handler)
    const waiting = this.pending.get(chat) ?? []
    this.pending.delete(chat)
    for (const item of waiting) item.run(handler)
  }

  unregister(chat: string): void {
    this.handlers.delete(chat)
    this.pending.delete(chat)
    this.closed.add(chat)
  }

  onStopped(listener: (id: string, reason: string) => void): () => void {
    this.stopped.add(listener)
    return () => this.stopped.delete(listener)
  }
}

/** One chat's engine as the controller sees it. */
class ChatTransport implements EngineTransport, Handler {
  readonly echoesActions = false
  private listeners = new Set<(action: SessionAction) => void>()
  private closeListeners = new Set<(close: TransportClose) => void>()
  private ready = false
  private launchError: string | null = null
  /** Why the engine ended before it was ready, for a `start` that comes after. */
  private failure: string | null = null
  /** A host call is starting a new engine: its lines (a higher generation) may come before the call returns. */
  private launching = false
  private waiting: { resolve: () => void; reject: (error: Error) => void } | null = null

  constructor(
    private readonly ipc: Ipc,
    readonly chat: string,
    private generation: number,
    private readonly timeoutMs: number,
  ) {}

  /** Whether an event is this transport's engine's; a newer one is adopted while a launch runs. */
  private current(generation: number): boolean {
    if (this.launching && generation > this.generation) this.generation = generation
    return generation === this.generation
  }

  line({ generation, line }: ChatLine): void {
    if (!this.current(generation)) return
    const parsed = parseUiEventLine(line)
    if (!parsed.ok) return
    if (parsed.event.type === 'ready') {
      this.ready = true
      this.waiting?.resolve()
    } else if (parsed.event.type === 'error' && !this.ready) {
      this.launchError = parsed.event.message
    }
    for (const listener of [...this.listeners]) listener(parsed.event)
  }

  exit({ generation, code, error: reported }: ChatExit): void {
    if (!this.current(generation)) return
    const wasReady = this.ready
    this.ready = false
    // The host's own message comes first: it may carry a classified reason.
    const error = this.launchError ?? reported ?? (code !== 0 || !wasReady ? `The Neuron engine stopped (exit code ${code}).` : null)
    if (!wasReady) this.failure = error ?? 'The Neuron engine stopped before it was ready.'
    this.waiting?.reject(new Error(error ?? 'The Neuron engine stopped before it was ready.'))
    for (const listener of [...this.closeListeners]) listener({ code, error })
  }

  /** Waits for the engine's `ready` (it may already have come). */
  start(): Promise<void> {
    if (this.ready) return Promise.resolve()
    if (this.failure) return Promise.reject(new Error(this.failure))
    return new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.waiting = null
        reject(new Error(startTimeout(this.timeoutMs)))
      }, this.timeoutMs)
      this.waiting = {
        resolve: () => {
          clearTimeout(timer)
          resolve()
        },
        reject: (error) => {
          clearTimeout(timer)
          reject(error)
        },
      }
    })
  }

  /** Runs `launch` (a host call that starts an engine), then waits for its `ready`. */
  async relaunch(launch: () => Promise<Summary>): Promise<Summary> {
    this.ready = false
    this.launchError = null
    this.failure = null
    this.launching = true
    const readyOrFailed = new Promise<void>((resolve, reject) => {
      this.waiting = { resolve, reject }
    })
    readyOrFailed.catch(() => {})
    // The host call and the engine's `ready` together have this long.
    let timer: ReturnType<typeof setTimeout> | undefined
    const deadline = new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error(startTimeout(this.timeoutMs))), this.timeoutMs)
    })
    deadline.catch(() => {})
    try {
      const summary = await Promise.race([launch(), deadline])
      this.generation = summary.generation
      await Promise.race([readyOrFailed, deadline])
      return summary
    } finally {
      clearTimeout(timer)
      this.launching = false
      this.waiting = null
    }
  }

  send(message: ClientMessage): void {
    if (!this.ready) throw new Error('The Neuron engine is not running.')
    void this.ipc.invoke('chat_send', { chat: this.chat, line: JSON.stringify(message) }).catch((failure: unknown) => this.sendFailed(failure))
  }

  /** The host could not hand a message to the engine: this chat is no longer connected. */
  private sendFailed(failure: unknown): void {
    const error = chatError(failure).message
    // A full input queue is a refusal of this message, not the end of the engine.
    if (error === NOT_KEEPING_UP) {
      for (const listener of [...this.listeners]) {
        listener({ type: 'error', message: error })
        listener({ type: 'turn_finished' })
      }
      return
    }
    if (!this.ready) return
    this.ready = false
    for (const listener of [...this.closeListeners]) listener({ code: null, error })
  }

  subscribe(listener: (action: SessionAction) => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  onClose(listener: (close: TransportClose) => void): () => void {
    this.closeListeners.add(listener)
    return () => this.closeListeners.delete(listener)
  }

  async close(): Promise<void> {
    await this.ipc.invoke('chat_stop', { chat: this.chat })
  }
}

export interface TauriChats extends ChatHost {
  /** The chat in view, for the host calls that read its folder. */
  activeId(): string | null
  info(id: string): ChatInfo | null
}

export function createChatHost(
  ipc: Ipc,
  options: {
    capabilities: readonly string[]
    /** What new engines start with (Settings). */
    launchOptions: () => LaunchOptions
    now?: () => number
    /** How long an engine has to say `ready` (default 30 s). */
    startTimeoutMs?: number
  },
): TauriChats {
  const bus = new ChatBus(ipc, options.now ?? Date.now)
  const transports = new Map<string, ChatTransport>()
  const controllers = new Map<string, SessionController>()
  const infos = new Map<string, ChatInfo>()
  let active: string | null = null

  const call = async <T>(command: string, args?: Record<string, unknown>): Promise<T> => {
    try {
      return await ipc.invoke<T>(command, args)
    } catch (failure) {
      throw chatError(failure)
    }
  }

  const attach = (summary: Summary) => {
    const transport = new ChatTransport(ipc, summary.chat, summary.generation, options.startTimeoutMs ?? START_TIMEOUT_MS)
    const controller = new SessionController(transport, { client: 'desktop', capabilities: options.capabilities })
    transports.set(summary.chat, transport)
    controllers.set(summary.chat, controller)
    infos.set(summary.chat, infoOf(summary))
    return { info: infoOf(summary), controller, transport }
  }

  const forget = (id: string) => {
    bus.unregister(id)
    transports.delete(id)
    controllers.delete(id)
    infos.delete(id)
    if (active === id) active = null
  }

  return {
    activeId: () => active,
    info: (id) => infos.get(id) ?? null,
    async open(request) {
      await bus.init()
      const summary = await call<Summary>('chat_open', {
        path: request.path ?? null,
        resume: request.resume ?? null,
        worktree: request.worktree ?? null,
        options: options.launchOptions(),
      })
      const { info, controller, transport } = attach(summary)
      try {
        // Registering delivers what the engine said meanwhile, after `start` has reset the state.
        const started = controller.start()
        bus.register(summary.chat, transport)
        await started
      } catch (failure) {
        // The host keeps the chat; one that never started is not worth keeping.
        forget(summary.chat)
        void ipc.invoke('chat_close', { chat: summary.chat }).catch(() => {})
        throw failure
      }
      return { info, controller }
    },
    async reattach() {
      await bus.init()
      const summaries = await call<Summary[]>('chat_list')
      const attached = summaries.map((summary) => ({ ...attach(summary), summary }))
      for (const { controller, transport, summary } of attached) {
        bus.register(summary.chat, transport)
        // A chat the host stopped (idle, evicted) stays stopped until it is looked at.
        if (!summary.running) {
          controller.apply({ type: 'local/disconnected', error: null })
          continue
        }
        controller.apply({ type: 'local/connecting' })
        // The window's own `ready` is gone: a new engine continues the session. The
        // chats are listed now; each shows its engine starting, none holds the others up.
        void transport
          .relaunch(() => call<Summary>('chat_resume', { chat: summary.chat, resume: null, options: options.launchOptions(), trustOnce: false }))
          .catch((failure: unknown) => controller.apply({ type: 'local/disconnected', error: failure instanceof Error ? failure.message : String(failure) }))
      }
      return attached.map(({ info, controller }) => ({ info, controller }))
    },
    async resume(id, resumeOptions = {}) {
      const transport = transports.get(id)
      if (!transport) throw new ChatError('No such chat.')
      controllers.get(id)?.apply({ type: 'local/connecting' })
      try {
        await transport.relaunch(() =>
          call<Summary>('chat_resume', {
            chat: id,
            // `''` starts a fresh session; without it the chat's own continues.
            resume: resumeOptions.resume === undefined ? null : (resumeOptions.resume ?? ''),
            options: options.launchOptions(),
            trustOnce: resumeOptions.trustOnce === true,
          }),
        )
      } catch (failure) {
        const error = chatError(failure)
        controllers.get(id)?.apply({ type: 'local/disconnected', error: error.message })
        throw error
      }
    },
    stop: (id) => call<void>('chat_stop', { chat: id }),
    async close(id) {
      // A chat with a copy of its own is not closed: the caller asks what to do with the copy.
      try {
        await call<void>('chat_close', { chat: id })
      } catch (failure) {
        if (failure instanceof ChatError && failure.code === 'has_worktree') throw failure
      }
      forget(id)
    },
    activate(id) {
      active = id
      void ipc.invoke('chat_activate', { chat: id }).catch(() => {})
    },
    reportActivity(id, activity) {
      void ipc.invoke('chat_activity', { chat: id, busy: activity.busy, waiting: activity.waiting }).catch(() => {})
    },
    onStopped: (listener) => bus.onStopped(listener),
    list: async () => (await call<Summary[]>('chat_list')).map(infoOf),
    worktreeStatus: (id) => call<WorktreeStatus>('worktree_status', { chat: id }),
    async discardWorktree(id, discard) {
      await call<void>('worktree_discard', { chat: id, force: discard.force, deleteUnmerged: discard.deleteUnmerged })
      forget(id)
    },
  }
}
