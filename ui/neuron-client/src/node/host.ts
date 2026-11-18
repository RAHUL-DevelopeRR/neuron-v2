/**
 * A Neuron host (Bun): owns one engine and serves it to remote clients over
 * WebSocket with the protocol in `../remote.ts`. The desktop app uses it in a
 * browser during development, and the mobile app on the LAN; the zero-x.live
 * relay will carry the same frames, and whoever builds it must apply this
 * host's policy, not only `parseClientMessage`.
 *
 * Security: it binds to 127.0.0.1 unless told otherwise, every client must
 * open with the pairing token, and browser origins other than local ones are
 * refused. A frame from a peer meets `parseClientMessage` first, which passes
 * only the commands in `PEER_PASSES` (test/protocol.test.ts). Passing is not
 * permission: `forward` still refuses, for every peer, local or not, what
 * `HOST_POLICY` calls `owner_only` (`shutdown` among it), a rewind that is not
 * a dry run, slash commands outside `REMOTE_SLASH` and a `resume` that names
 * anything but a session id, and `trust_workspace` for a peer on another
 * machine. A second `interrupt` in one turn still ends the engine.
 * The `fs.*` methods only read inside the workspace. Terminals (`term.open`)
 * are only for clients on this machine, since a shell would bypass the
 * engine's permission prompts.
 */

import { timingSafeEqual } from 'crypto'
import { promises as fs } from 'fs'
import path from 'path'

import type { EngineTransport } from '../controller'
import { CAPABILITY_QUESTIONS, FEATURE_CAPABILITIES, commandMessage, type ClientMessage, type EngineCommand } from '../protocol'
import {
  REMOTE_PROTOCOL,
  parseClientFrame,
  type ClientFrame,
  type ClientKind,
  type HostFrame,
  type HostInfo,
} from '../remote'
import { initialSession, reduceSession, type SessionAction, type SessionState } from '../session'
import { gitDiff, gitStatus, searchFiles } from './workspace'

export interface HostOptions {
  transport: EngineTransport
  token: string
  /** The workspace the `fs.*` methods may read. */
  root: string
  name?: string
  /** `127.0.0.1` by default; `0.0.0.0` serves the LAN. */
  hostname?: string
  /** 0 picks a free port. */
  port?: number
  /** Browser origins allowed besides local ones. */
  allowedOrigins?: string[]
  helloTimeoutMs?: number
  /** The environment of terminals; defaults to this process's. */
  terminalEnv?: Record<string, string | undefined>
  /** Whether a client's address is this machine (loopback by default; tests override it). */
  isLocal?: (address: string) => boolean
  /**
   * Treats a client on this machine as the owner: it may use any slash command and resume a session by path,
   * which the desktop app in a browser needs (a new session, the permission mode, login). Off by default.
   * Never turn it on while a share bridge (`neuron engine --share`) is connected: the bridge's phone reaches this
   * host from 127.0.0.1 too, so the host cannot tell it from the owner. What `HOST_POLICY` calls `owner_only`
   * stays refused either way.
   */
  ownerOnLoopback?: boolean
  /**
   * A new engine, continuing `resume` (a session id) when given: lets clients on this
   * machine restart it (`engine.restart`), as trusting the workspace needs.
   */
  restart?: (resume: string | null) => EngineTransport
}

interface Terminal {
  write(data: string): void
  resize(cols: number, rows: number): void
  close(): void
}

interface Peer {
  authed: boolean
  name: string
  kind: ClientKind
  timer: ReturnType<typeof setTimeout> | null
  /**
   * The client connected from this machine (loopback). That says where the socket came from, not who is
   * typing: a phone behind `neuron engine --share` is local too. It opens only `term.open`, `engine.restart`
   * and `trust_workspace`; the slash and resume walls bind every peer.
   */
  local: boolean
  terminals: Map<number, { process: ReturnType<typeof Bun.spawn>; terminal: Terminal }>
}

type Socket = import('bun').ServerWebSocket<Peer>

const MAX_FRAME_BYTES = 1_000_000
const MAX_READ_BYTES = 512 * 1024
const MAX_LIST_ENTRIES = 2_000
const MAX_TERMINALS_PER_CLIENT = 4

/** Slash commands that only read. No peer may send any other, local or not, unless it is on this machine and the host was started with `ownerOnLoopback`. */
const REMOTE_SLASH = new Set(['help', 'status', 'cost', 'context', 'usage', 'doctor', 'changes', 'diff', 'stats', 'memory', 'model'])

const OWNER_ONLY = 'Only the Neuron desktop app can change your files, history or settings.'

/**
 * What the host does with each engine command a peer sends, local or not. Every `EngineCommand` tag is
 * decided here, on purpose: a new one fails `bun run typecheck` until someone chooses.
 * - `owner_only`: refused to every peer. These change the workspace, the history, the user's settings or
 *   what the agent may do, or cost the engine real work, and the phone app has no screen for them.
 *   `parseClientMessage` already drops all of them but `shutdown`, `commit` and `set_mcp_server_enabled`
 *   before they get here; this is the second wall, for the day the first has a hole. (`forward` answers
 *   `shutdown` first, with a message of its own.)
 * - `forward`: goes on to the engine, after what `forward` still checks: `slash` only for `REMOTE_SLASH`
 *   (and `/resume` with a session id), `resume` only by session id, `rewind` only as a dry run and
 *   `trust_workspace` only for a peer on this machine. `capabilities` is dropped: the host speaks for all
 *   its clients. A peer on this machine of a host started with `ownerOnLoopback` is exempt from the `slash`
 *   and `resume` checks, and from nothing else.
 * Every `PEER_REFUSED` tag in test/protocol.test.ts is `owner_only` here.
 */
export const HOST_POLICY: Record<EngineCommand['type'], 'forward' | 'owner_only'> = {
  submit: 'forward',
  slash: 'forward',
  set_model: 'forward',
  list_models: 'forward',
  interrupt: 'forward',
  shutdown: 'owner_only',
  capabilities: 'forward',
  answer: 'forward',
  list_sessions: 'forward',
  resume: 'forward',
  trust_workspace: 'forward',
  list_extensions: 'forward',
  trust_preview: 'forward',
  refresh_account: 'forward',
  set_mcp_server_enabled: 'owner_only',
  commit: 'owner_only',
  rewind: 'forward',
  list_connector_catalog: 'owner_only',
  add_mcp_server: 'owner_only',
  remove_mcp_server: 'owner_only',
  approve_mcp_server: 'owner_only',
  set_reasoning_effort: 'owner_only',
  set_permission_mode: 'owner_only',
  list_branches: 'owner_only',
  switch_branch: 'owner_only',
}

const SESSION_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/
const LOOPBACK_HOST = /^(localhost|127\.0\.0\.1|\[::1\])(:\d{1,5})?$/i

/** A session id or `latest`, never a path. */
const isSessionRef = (value: unknown): boolean => typeof value === 'string' && (value === 'latest' || SESSION_ID.test(value))

const isLoopback = (address: string) => address === '::1' || address.startsWith('127.') || address === '::ffff:127.0.0.1'

/** The user's shell, as a terminal pane runs it. */
function shellCommand(): string[] {
  if (process.platform === 'win32') return ['powershell.exe', '-NoLogo']
  return [process.env.SHELL || '/bin/bash', '-l']
}

export class NeuronHost {
  state: SessionState = initialSession()
  seq = 0
  private sockets = new Set<Socket>()
  private server: ReturnType<typeof Bun.serve<Peer>> | null = null
  private unsubscribers: Array<() => void> = []
  private nextTerminal = 1
  private transport: EngineTransport

  constructor(private readonly options: HostOptions) {
    this.transport = options.transport
    this.attach(options.transport)
  }

  /** Follows `transport`'s events. Its clients take structured questions, so its engine sends them. */
  private attach(transport: EngineTransport): void {
    this.unsubscribers.push(
      transport.subscribe((action) => {
        this.record(action)
        if (action.type === 'ready' && action.features?.includes(FEATURE_CAPABILITIES)) {
          transport.send(commandMessage({ type: 'capabilities', features: [CAPABILITY_QUESTIONS] }))
        }
      }),
      transport.onClose((close) => this.record({ type: 'local/disconnected', error: close.error })),
    )
  }

  private detach(): void {
    for (const unsubscribe of this.unsubscribers) unsubscribe()
    this.unsubscribers = []
  }

  /** Replaces the engine with a new one continuing `resume`. Clients see it start again. */
  async restartEngine(resume: string | null): Promise<void> {
    if (!this.options.restart) throw new Error('This host cannot restart its engine.')
    const old = this.transport
    this.detach()
    await old.close().catch(() => {})
    this.transport = this.options.restart(resume)
    this.attach(this.transport)
    this.record({ type: 'local/connecting' })
    await this.transport.start()
  }

  get info(): HostInfo {
    return {
      name: this.options.name ?? 'Neuron',
      version: this.state.version,
      cwd: this.state.status?.cwd ?? this.options.root,
    }
  }

  get url(): string {
    if (!this.server) throw new Error('The host is not listening.')
    const shown = this.server.hostname === '0.0.0.0' ? '127.0.0.1' : this.server.hostname
    return `ws://${shown}:${this.server.port}/neuron`
  }

  get clientCount(): number {
    return [...this.sockets].filter((socket) => socket.data.authed).length
  }

  /** Listens, then starts the engine. Resolves once both are up. */
  async start(): Promise<void> {
    this.listen()
    this.record({ type: 'local/connecting' })
    await this.transport.start()
  }

  listen(): void {
    if (this.server) return
    const host = this
    this.server = Bun.serve<Peer>({
      hostname: this.options.hostname ?? '127.0.0.1',
      port: this.options.port ?? 0,
      fetch(request, server) {
        const url = new URL(request.url)
        if (url.pathname !== '/neuron') return new Response('Neuron host', { status: 404 })
        if (!host.originAllowed(request.headers.get('origin'), request.headers.get('host'))) {
          return new Response('origin not allowed', { status: 403 })
        }
        const local = (host.options.isLocal ?? isLoopback)(server.requestIP(request)?.address ?? '')
        const upgraded = server.upgrade(request, {
          data: { authed: false, name: '', kind: 'web', timer: null, local, terminals: new Map() },
        })
        return upgraded ? undefined : new Response('WebSocket upgrade required', { status: 426 })
      },
      websocket: {
        maxPayloadLength: MAX_FRAME_BYTES,
        open(socket) {
          host.sockets.add(socket)
          socket.data.timer = setTimeout(() => {
            if (!socket.data.authed) socket.close(4408, 'hello timed out')
          }, host.options.helloTimeoutMs ?? 10_000)
        },
        message(socket, message) {
          const text = typeof message === 'string' ? message : new TextDecoder().decode(message)
          // Bun exits on an unhandled rejection, so nothing a frame sets off may escape: its peer is told, the host serves on.
          host.receive(socket, text).catch(() => host.failed(socket))
        },
        close(socket) {
          if (socket.data.timer) clearTimeout(socket.data.timer)
          for (const { process: child, terminal } of socket.data.terminals.values()) {
            terminal.close()
            child.kill()
          }
          socket.data.terminals.clear()
          const wasAuthed = socket.data.authed
          host.sockets.delete(socket)
          if (wasAuthed) host.broadcast({ t: 'clients', count: host.clientCount })
        },
      },
    })
  }

  /** Local pages (the desktop app in a browser, Expo web) and native clients (no origin). */
  private originAllowed(origin: string | null, hostHeader: string | null): boolean {
    if (!origin) return true
    if (this.options.allowedOrigins?.includes(origin)) return true
    try {
      const { hostname, host } = new URL(origin)
      if (['localhost', '127.0.0.1', '[::1]', 'tauri.localhost'].includes(hostname)) return true
      // A page served by this host itself: its Host header must still be a loopback name.
      return hostHeader !== null && host === hostHeader && LOOPBACK_HOST.test(hostHeader)
    } catch {
      return false
    }
  }

  private send(socket: Socket, frame: HostFrame): void {
    socket.send(JSON.stringify(frame))
  }

  /** A frame made the host throw: tells that peer, without the details, and goes on serving. */
  private failed(socket: Socket): void {
    try {
      this.send(socket, { t: 'error', message: 'The host could not handle that frame.', fatal: false })
    } catch {
      // The peer is gone: there is no one to tell, and nothing may escape from here either.
    }
  }

  /** Hands `message` to the engine. When the engine has just died, the peer is told and this returns false. */
  private toEngine(socket: Socket, message: ClientMessage): boolean {
    try {
      this.transport.send(message)
      return true
    } catch {
      this.send(socket, { t: 'error', message: 'The engine is not running.', fatal: false })
      return false
    }
  }

  private broadcast(frame: HostFrame): void {
    const text = JSON.stringify(frame)
    for (const socket of this.sockets) if (socket.data.authed) socket.send(text)
  }

  /** Applies an action to the host's session and sends it to every client. */
  private record(action: SessionAction): void {
    this.seq += 1
    this.state = reduceSession(this.state, action, Date.now())
    this.broadcast({ t: 'action', seq: this.seq, action })
  }

  private async receive(socket: Socket, text: string): Promise<void> {
    const frame = parseClientFrame(text)
    if (!socket.data.authed) {
      if (frame?.t !== 'hello') return socket.close(4400, 'hello first')
      return this.hello(socket, frame)
    }
    if (!frame) return this.send(socket, { t: 'error', message: 'Not a Neuron remote frame.', fatal: false })
    switch (frame.t) {
      case 'hello':
        return this.send(socket, { t: 'error', message: 'Already connected.', fatal: false })
      case 'ping':
        return this.send(socket, { t: 'pong' })
      case 'send':
        return this.forward(socket, frame.message)
      case 'term_input':
        return socket.data.terminals.get(frame.id)?.terminal.write(frame.data)
      case 'term_resize':
        return socket.data.terminals.get(frame.id)?.terminal.resize(frame.cols, frame.rows)
      case 'rpc':
        try {
          const result = await this.call(socket, frame.method, frame.params ?? {})
          return this.send(socket, { t: 'rpc_result', id: frame.id, result })
        } catch (error) {
          return this.send(socket, { t: 'rpc_result', id: frame.id, error: error instanceof Error ? error.message : String(error) })
        }
    }
  }

  private hello(socket: Socket, frame: Extract<ClientFrame, { t: 'hello' }>): void {
    if (frame.protocol !== REMOTE_PROTOCOL) {
      this.send(socket, { t: 'error', message: `This host speaks remote protocol ${REMOTE_PROTOCOL}.`, fatal: true })
      return socket.close(4426, 'protocol mismatch')
    }
    if (!sameSecret(frame.token, this.options.token)) {
      this.send(socket, { t: 'error', message: 'The pairing token is not right.', fatal: true })
      return socket.close(4401, 'bad token')
    }
    if (socket.data.timer) clearTimeout(socket.data.timer)
    socket.data = { ...socket.data, authed: true, name: frame.client.name, kind: frame.client.kind, timer: null }
    this.send(socket, { t: 'welcome', protocol: REMOTE_PROTOCOL, host: this.info, clients: this.clientCount })
    this.send(socket, { t: 'snapshot', seq: this.seq, state: this.state })
    this.broadcast({ t: 'clients', count: this.clientCount })
  }

  private forward(socket: Socket, message: ClientMessage): void {
    const refuse = (text: string) => this.send(socket, { t: 'error', message: text, fatal: false })
    if (this.state.connection !== 'ready') return refuse('The engine is not running.')
    if (message.kind === 'permission_reply') {
      // "Always allow in this project" saves rules into the project's settings for good: the desktop app's call.
      if (message.decision.kind === 'allow_for_project') return refuse(OWNER_ONLY)
      // The first answer wins; later ones (another client's) are dropped.
      if (!this.state.permissions.some((permission) => permission.id === message.id)) return
      // Answered only once the engine has the answer: when it just died, the request stays open.
      if (!this.toEngine(socket, message)) return
      return this.record({ type: 'local/permission_answered', id: message.id, decision: message.decision })
    }
    if (message.type === 'shutdown') return refuse('Only the machine running Neuron can stop the engine.')
    // The host speaks for all its clients.
    if (message.type === 'capabilities') return
    // Whatever `HOST_POLICY` does not say `forward` (a tag it does not know fails closed), and a rewind that is not a dry run.
    if (HOST_POLICY[message.type] !== 'forward' || (message.type === 'rewind' && message.dry_run !== true)) {
      return refuse(OWNER_ONLY)
    }
    // The next two walls hold for every peer, local or not: a phone behind `neuron engine --share` reaches this host
    // from 127.0.0.1, so `local` says where a socket came from, not who is typing. Only a host started with
    // `ownerOnLoopback` lets a peer on this machine past them.
    const owner = this.options.ownerOnLoopback === true && socket.data.local
    if (!owner && message.type === 'slash') {
      const name = message.command.trim().toLowerCase()
      const allowed =
        REMOTE_SLASH.has(name) || (name === 'resume' && isSessionRef(typeof message.args === 'string' ? message.args.trim() : ''))
      if (!allowed) return refuse(OWNER_ONLY)
    }
    if (!owner && message.type === 'resume' && !isSessionRef(message.session)) {
      return refuse('A remote client can only resume a session by its id.')
    }
    // `local` is for this, `term.open`, `engine.restart` and `ownerOnLoopback`; the share bridge filters `trust_workspace` for its phone itself.
    if (message.type === 'trust_workspace' && !socket.data.local) {
      return refuse('Only the machine running Neuron can trust its workspace.')
    }
    // Nothing is recorded for a message the engine did not get.
    if (!this.toEngine(socket, message)) return
    if (message.type === 'submit') this.record({ type: 'local/submitted', text: message.text, attachments: message.attachments })
    else if (message.type === 'slash') this.record({ type: 'local/slash', command: message.command, args: message.args })
    else if (message.type === 'answer') this.record({ type: 'local/answered', id: message.id, text: message.text })
  }

  private async call(socket: Socket, method: string, params: Record<string, unknown>): Promise<unknown> {
    switch (method) {
      case 'term.open':
        return this.openTerminal(socket, params)
      case 'term.close': {
        const entry = socket.data.terminals.get(Number(params.id))
        entry?.terminal.close()
        entry?.process.kill()
        return { closed: entry !== undefined }
      }
      case 'fs.list':
        return listDirectory(this.options.root, typeof params.path === 'string' ? params.path : '.')
      case 'fs.read':
        return readFile(this.options.root, typeof params.path === 'string' ? params.path : '')
      case 'fs.search':
        return searchFiles(this.options.root, typeof params.query === 'string' ? params.query : '')
      case 'git.status':
        return gitStatus(this.options.root, this.state.status?.workspace_trusted === true)
      case 'git.diff':
        return gitDiff(this.options.root, typeof params.path === 'string' ? params.path : '', this.state.status?.workspace_trusted === true)
      case 'engine.restart': {
        if (!socket.data.local) throw new Error('Only the machine running Neuron can restart its engine.')
        const resume = typeof params.resume === 'string' && params.resume ? params.resume : null
        await this.restartEngine(resume)
        return { restarted: true }
      }
      default:
        throw new Error(`Unknown method ${method}.`)
    }
  }

  private openTerminal(socket: Socket, params: Record<string, unknown>): { id: number } {
    if (!socket.data.local) throw new Error('Terminals are only available on the machine running Neuron.')
    if (socket.data.terminals.size >= MAX_TERMINALS_PER_CLIENT) throw new Error('Too many terminals open.')
    const size = (value: unknown, fallback: number) =>
      Number.isSafeInteger(value) && (value as number) > 0 && (value as number) <= 1000 ? (value as number) : fallback
    const id = this.nextTerminal++
    const decoder = new TextDecoder()
    const child = Bun.spawn(shellCommand(), {
      cwd: this.options.root,
      env: this.options.terminalEnv ?? process.env,
      terminal: {
        cols: size(params.cols, 80),
        rows: size(params.rows, 24),
        data: (_terminal: unknown, data: Uint8Array) => {
          if (socket.readyState === 1) this.send(socket, { t: 'term_output', id, data: decoder.decode(data, { stream: true }) })
        },
      },
    } as Parameters<typeof Bun.spawn>[1])
    const terminal = (child as unknown as { terminal: Terminal }).terminal
    socket.data.terminals.set(id, { process: child, terminal })
    void child.exited.then((code) => {
      socket.data.terminals.delete(id)
      if (socket.readyState === 1) this.send(socket, { t: 'term_exit', id, code })
    })
    return { id }
  }

  async stop(): Promise<void> {
    for (const socket of this.sockets) socket.close(1001, 'host stopping')
    this.server?.stop(true)
    this.server = null
    await this.transport.close()
    this.detach()
  }
}

function sameSecret(given: string, expected: string): boolean {
  const a = Buffer.from(given)
  const b = Buffer.from(expected)
  return a.length === b.length && timingSafeEqual(a, b)
}

/** A path inside `root`, or an error. Symlinks that lead out are refused too. */
async function insideRoot(root: string, relative: string): Promise<{ absolute: string; relative: string }> {
  const base = await fs.realpath(root)
  const absolute = await fs.realpath(path.resolve(base, relative)).catch(() => {
    throw new Error(`No such file or folder: ${relative}`)
  })
  const inside = path.relative(base, absolute)
  if (inside.startsWith('..') || path.isAbsolute(inside)) throw new Error('That path is outside the workspace.')
  return { absolute, relative: inside.split(path.sep).join('/') || '.' }
}

export interface DirEntry {
  name: string
  /** Relative to the workspace, with `/`. */
  path: string
  dir: boolean
  size: number | null
}

export async function listDirectory(root: string, relative: string): Promise<{ path: string; entries: DirEntry[]; truncated: boolean }> {
  const where = await insideRoot(root, relative)
  const dirents = await fs.readdir(where.absolute, { withFileTypes: true })
  const entries: DirEntry[] = []
  for (const dirent of dirents.slice(0, MAX_LIST_ENTRIES)) {
    const entryPath = where.relative === '.' ? dirent.name : `${where.relative}/${dirent.name}`
    const dir = dirent.isDirectory()
    let size: number | null = null
    if (!dir) size = await fs.stat(path.join(where.absolute, dirent.name)).then((stat) => stat.size, () => null)
    entries.push({ name: dirent.name, path: entryPath, dir, size })
  }
  entries.sort((a, b) => (a.dir === b.dir ? a.name.localeCompare(b.name) : a.dir ? -1 : 1))
  return { path: where.relative, entries, truncated: dirents.length > MAX_LIST_ENTRIES }
}

export async function readFile(
  root: string,
  relative: string,
): Promise<{ path: string; content: string; truncated: boolean; binary: boolean; size: number }> {
  const where = await insideRoot(root, relative)
  const handle = await fs.open(where.absolute, 'r')
  try {
    const { size } = await handle.stat()
    const buffer = Buffer.alloc(Math.min(size, MAX_READ_BYTES))
    await handle.read(buffer, 0, buffer.length, 0)
    const binary = buffer.subarray(0, 8_000).includes(0)
    return { path: where.relative, content: binary ? '' : buffer.toString('utf8'), truncated: size > MAX_READ_BYTES, binary, size }
  } finally {
    await handle.close()
  }
}
