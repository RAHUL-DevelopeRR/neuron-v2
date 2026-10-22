import {
  SessionController,
  type ClientMessage,
  type EngineTransport,
  type SessionAction,
  type TransportClose,
  type UiEvent,
} from '@neuron/client'

import type { DirListing, FileContent, Platform, TerminalSession } from '../src/platform'

/** An engine in memory: tests emit its events and read what the UI sent. */
export class FakeEngine implements EngineTransport {
  readonly echoesActions = false
  sent: ClientMessage[] = []
  private listeners = new Set<(action: SessionAction) => void>()
  private closeListeners = new Set<(close: TransportClose) => void>()

  constructor(private readonly statusOverrides: Record<string, unknown> = {}) {}

  async start() {
    this.emit({ type: 'ready', protocol: 1, version: '6.3.0' })
    this.emit(this.status())
  }

  status(overrides: Record<string, unknown> = {}): UiEvent {
    return {
      type: 'status',
      model: 'claude-sonnet-4-6',
      permission_mode: 'workspace-write',
      session_id: 'session-1',
      cwd: '/work/app',
      workspace_trusted: true,
      plan_mode: false,
      context_percent: 3,
      git: { branch: 'main', changed: 2, staged: 0, unstaged: 2, untracked: 0, conflicted: 0 },
      signed_in: true,
      sign_in_required: false,
      account: { email: 'dev@example.com', plan: 'pro', source: 'keychain', expires_at: null },
      ...this.statusOverrides,
      ...overrides,
    } as UiEvent
  }

  emit(event: UiEvent) {
    for (const listener of [...this.listeners]) listener(event)
  }
  send(message: ClientMessage) {
    this.sent.push(message)
  }
  subscribe(listener: (action: SessionAction) => void) {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }
  onClose(listener: (close: TransportClose) => void) {
    this.closeListeners.add(listener)
    return () => this.closeListeners.delete(listener)
  }
  async close() {}
}

export const FILES: Record<string, string> = {
  'src/main.rs': 'fn main() {\n    println!("hi");\n}\n',
  'README.md': '# app\n',
}

/** A platform over a {@link FakeEngine} and a small in-memory workspace. */
export function fakePlatform(engine: FakeEngine): Platform & { opened: string[] } {
  let connecting: Promise<SessionController> | null = null
  const opened: string[] = []
  return {
    kind: 'host',
    label: 'test',
    opened,
    connect() {
      connecting ??= (async () => {
        const controller = new SessionController(engine)
        await controller.start()
        return controller
      })()
      return connecting
    },
    async list(path: string): Promise<DirListing> {
      const prefix = path === '.' ? '' : `${path}/`
      const names = new Map<string, boolean>()
      for (const file of Object.keys(FILES)) {
        if (!file.startsWith(prefix)) continue
        const [first, ...rest] = file.slice(prefix.length).split('/')
        names.set(first, rest.length > 0)
      }
      const entries = [...names].map(([name, dir]) => ({ name, path: `${prefix}${name}`, dir, size: dir ? null : 1 }))
      entries.sort((a, b) => (a.dir === b.dir ? a.name.localeCompare(b.name) : a.dir ? -1 : 1))
      return { path, entries, truncated: false }
    },
    async read(path: string): Promise<FileContent> {
      if (!(path in FILES)) throw new Error(`No such file or folder: ${path}`)
      return { path, content: FILES[path], truncated: false, binary: false, size: FILES[path].length }
    },
    openTerminal(): Promise<TerminalSession> {
      return Promise.reject(new Error('no terminal in tests'))
    },
    openExternal(url: string) {
      opened.push(url)
    },
  }
}
