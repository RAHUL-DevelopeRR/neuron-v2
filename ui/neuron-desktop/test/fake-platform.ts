import {
  SessionController,
  type ClientMessage,
  type EngineTransport,
  type SessionAction,
  type TransportClose,
  type UiEvent,
} from '@neuron/client'

import type { DirListing, FileContent, GitStatusView, LaunchOptions, Platform, Project, TerminalSession, UpdateInfo } from '../src/platform'
import type { Settings } from '../src/settings'

export const ALL_FEATURES = ['capabilities', 'sessions', 'trust_workspace', 'todos', 'subagents', 'extensions']

/** An engine in memory: tests emit its events and read what the UI sent. */
export class FakeEngine implements EngineTransport {
  readonly echoesActions = false
  sent: ClientMessage[] = []
  private listeners = new Set<(action: SessionAction) => void>()
  closeListeners = new Set<(close: TransportClose) => void>()

  constructor(
    private readonly statusOverrides: Record<string, unknown> = {},
    private readonly features: string[] = ALL_FEATURES,
  ) {}

  async start() {
    this.emit({ type: 'ready', protocol: 1, version: '6.3.0', features: this.features })
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
  /** What the UI sent, as commands (without permission replies). */
  commands() {
    return this.sent.filter((message) => message.kind === 'command')
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
  'src/lib.rs': 'pub fn add() {}\n',
  'README.md': '# app\n',
}

export interface FakePlatform extends Platform {
  opened: string[]
  restarts: Array<string | null>
  openedProjects: string[]
  /** `[path, resume]` for each project opened. */
  projectOpens: Array<[string, string | null]>
  launches: LaunchOptions[]
  git: GitStatusView | null
  /** What the folder picker returns next. */
  picked: string | null
  update: UpdateInfo | null
  saved: Map<string, unknown>
}

export interface FakePlatformOptions {
  /** Settings already saved; the welcome steps count as done unless these say otherwise. */
  settings?: Partial<Settings>
  update?: UpdateInfo | null
}

/** A platform over a {@link FakeEngine} and a small in-memory workspace with git. */
export function fakePlatform(engine: FakeEngine, options: FakePlatformOptions = {}): FakePlatform {
  let connecting: Promise<SessionController> | null = null
  let project: Project = { name: 'app', path: '/work/app' }
  const saved = new Map<string, unknown>([['settings', { firstRunDone: true, ...options.settings }]])
  const platform: FakePlatform = {
    kind: 'host',
    label: 'test',
    can: { openProject: true, restart: true, terminal: false, git: true, pickFolder: true, updates: true },
    opened: [],
    restarts: [],
    openedProjects: [],
    projectOpens: [],
    launches: [],
    picked: '/work/picked',
    update: options.update ?? null,
    saved,
    store: {
      get: async <T,>(key: string) => saved.get(key) as T | undefined,
      set: async (key, value) => void saved.set(key, value),
    },
    setLaunchOptions(launch) {
      platform.launches.push(launch)
    },
    pickFolder: async () => platform.picked,
    appVersion: async () => '0.2.0',
    checkForUpdate: async () => platform.update,
    git: {
      branch: 'main',
      changes: [
        { path: 'src/main.rs', index: ' ', worktree: 'M' },
        { path: 'notes.txt', index: '?', worktree: '?' },
      ],
    },
    connect() {
      connecting ??= (async () => {
        const controller = new SessionController(engine)
        await controller.start()
        return controller
      })()
      return connecting
    },
    project: () => project,
    recentProjects: async () => [project, { name: 'website', path: '/work/website' }],
    async openProject(path, resume = null) {
      platform.openedProjects.push(path)
      platform.projectOpens.push([path, resume])
      project = { name: path.split('/').pop() ?? path, path }
    },
    async restart(resume) {
      platform.restarts.push(resume)
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
      const content = path === 'notes.txt' ? 'a new note\n' : FILES[path]
      if (content === undefined) throw new Error(`No such file or folder: ${path}`)
      return { path, content, truncated: false, binary: false, size: content.length }
    },
    searchFiles: async (query) => Object.keys(FILES).filter((file) => file.includes(query)),
    gitStatus: async () => platform.git,
    gitDiff: async (path) =>
      path === 'src/main.rs' ? '@@ -1,3 +1,3 @@\n fn main() {\n-    println!("hi");\n+    println!("hello");\n }\n' : '',
    openTerminal(): Promise<TerminalSession> {
      return Promise.reject(new Error('no terminal in tests'))
    },
    openExternal(url: string) {
      platform.opened.push(url)
    },
  }
  return platform
}
