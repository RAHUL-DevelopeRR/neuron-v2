/**
 * A Neuron host (Bun): owns one engine and serves it to remote clients over
 * WebSocket with the protocol in `../remote.ts`. The desktop app uses it in a
 * browser during development, and the mobile app on the LAN; the zero-x.live
 * relay will carry the same frames.
 *
 * Security: it binds to 127.0.0.1 unless told otherwise, every client must
 * open with the pairing token, browser origins other than local ones are
 * refused, and remote clients cannot shut the engine down. The `fs.*` methods
 * only read inside the workspace.
 */

import { timingSafeEqual } from 'crypto'
import { promises as fs } from 'fs'
import path from 'path'

import type { EngineTransport } from '../controller'
import {
  REMOTE_PROTOCOL,
  parseClientFrame,
  type ClientFrame,
  type ClientKind,
  type HostFrame,
  type HostInfo,
} from '../remote'
import { initialSession, reduceSession, type SessionAction, type SessionState } from '../session'

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
}

interface Peer {
  authed: boolean
  name: string
  kind: ClientKind
  timer: ReturnType<typeof setTimeout> | null
}

type Socket = import('bun').ServerWebSocket<Peer>

const MAX_FRAME_BYTES = 1_000_000
const MAX_READ_BYTES = 512 * 1024
const MAX_LIST_ENTRIES = 2_000

export class NeuronHost {
  state: SessionState = initialSession()
  seq = 0
  private sockets = new Set<Socket>()
  private server: ReturnType<typeof Bun.serve<Peer, undefined>> | null = null
  private unsubscribers: Array<() => void> = []

  constructor(private readonly options: HostOptions) {
    this.unsubscribers.push(
      options.transport.subscribe((action) => this.record(action)),
      options.transport.onClose((close) => this.record({ type: 'local/disconnected', error: close.error })),
    )
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
    await this.options.transport.start()
  }

  listen(): void {
    if (this.server) return
    const host = this
    this.server = Bun.serve<Peer, undefined>({
      hostname: this.options.hostname ?? '127.0.0.1',
      port: this.options.port ?? 0,
      fetch(request, server) {
        const url = new URL(request.url)
        if (url.pathname !== '/neuron') return new Response('Neuron host', { status: 404 })
        if (!host.originAllowed(request.headers.get('origin'), request.headers.get('host'))) {
          return new Response('origin not allowed', { status: 403 })
        }
        const upgraded = server.upgrade(request, { data: { authed: false, name: '', kind: 'web', timer: null } })
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
          void host.receive(socket, text)
        },
        close(socket) {
          if (socket.data.timer) clearTimeout(socket.data.timer)
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
      return host === hostHeader
    } catch {
      return false
    }
  }

  private send(socket: Socket, frame: HostFrame): void {
    socket.send(JSON.stringify(frame))
  }

  private broadcast(frame: HostFrame): void {
    const text = JSON.stringify(frame)
    for (const socket of this.sockets) if (socket.data.authed) socket.send(text)
  }

  /** Applies an action to the host's session and sends it to every client. */
  private record(action: SessionAction): void {
    this.seq += 1
    this.state = reduceSession(this.state, action)
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
      case 'rpc':
        try {
          const result = await this.call(frame.method, frame.params ?? {})
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
    socket.data = { authed: true, name: frame.client.name, kind: frame.client.kind, timer: null }
    this.send(socket, { t: 'welcome', protocol: REMOTE_PROTOCOL, host: this.info, clients: this.clientCount })
    this.send(socket, { t: 'snapshot', seq: this.seq, state: this.state })
    this.broadcast({ t: 'clients', count: this.clientCount })
  }

  private forward(socket: Socket, message: import('../protocol').ClientMessage): void {
    const refuse = (text: string) => this.send(socket, { t: 'error', message: text, fatal: false })
    if (this.state.connection !== 'ready') return refuse('The engine is not running.')
    if (message.kind === 'permission_reply') {
      // The first answer wins; later ones (another client's) are dropped.
      if (!this.state.permissions.some((permission) => permission.id === message.id)) return
      this.options.transport.send(message)
      return this.record({ type: 'local/permission_answered', id: message.id, decision: message.decision })
    }
    if (message.type === 'shutdown') return refuse('Only the machine running Neuron can stop the engine.')
    this.options.transport.send(message)
    if (message.type === 'submit') this.record({ type: 'local/submitted', text: message.text, attachments: message.attachments })
    else if (message.type === 'slash') this.record({ type: 'local/slash', command: message.command, args: message.args })
  }

  private async call(method: string, params: Record<string, unknown>): Promise<unknown> {
    switch (method) {
      case 'fs.list':
        return listDirectory(this.options.root, typeof params.path === 'string' ? params.path : '.')
      case 'fs.read':
        return readFile(this.options.root, typeof params.path === 'string' ? params.path : '')
      default:
        throw new Error(`Unknown method ${method}.`)
    }
  }

  async stop(): Promise<void> {
    for (const socket of this.sockets) socket.close(1001, 'host stopping')
    this.server?.stop(true)
    this.server = null
    await this.options.transport.close()
    for (const unsubscribe of this.unsubscribers) unsubscribe()
    this.unsubscribers = []
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
