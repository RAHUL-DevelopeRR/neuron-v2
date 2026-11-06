import {
  SessionController,
  type ClientMessage,
  type EngineTransport,
  type SessionAction,
  type TransportClose,
  type UiEvent,
} from '@neuron/client'

import { ChatError, type ChatHost, type ChatInfo, type DirListing, type FileContent, type GitStatusView, type LaunchOptions, type Platform, type Project, type TerminalSession, type UpdateInfo, type WorktreeStatus } from '../src/platform'
import type { Settings } from '../src/settings'

export const ALL_FEATURES = [
  'capabilities',
  'sessions',
  'trust_workspace',
  'todos',
  'subagents',
  'extensions',
  'account',
  'error_details',
  'session_scope',
  'trust_preview',
]

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
  /** Restarts that trusted the folder for that engine only. */
  trustedOnce: Array<string | null>
  openedProjects: string[]
  /** `[path, resume]` for each project opened. */
  projectOpens: Array<[string, string | null]>
  launches: LaunchOptions[]
  git: GitStatusView | null
  /** What the folder picker returns next. */
  picked: string | null
  update: UpdateInfo | null
  /** How many times the updater was asked. */
  checks: number
  saved: Map<string, unknown>
  // L4: multi-chat
  /** Notifications the app asked for. */
  notifications: Array<{ chat: string; title: string; body: string }>
  /** Every count the app put on the taskbar icon. */
  attention: number[]
  /** What `notify` answers next. */
  notifyResult: 'shown' | 'denied' | 'unsupported'
  /** The chats, when the platform was made with `multiChat`. */
  fakeChats: FakeChats | null
}

export interface FakePlatformOptions {
  /** Settings already saved; the welcome steps count as done unless these say otherwise. */
  settings?: Partial<Settings>
  update?: UpdateInfo | null
  /** A build without an updater key: updates come from the releases page. */
  manualUpdates?: boolean
  /** Several chats at once; `engine` is the first chat's. */
  multiChat?: boolean
}

/** A platform over a {@link FakeEngine} and a small in-memory workspace with git. */
export function fakePlatform(engine: FakeEngine, options: FakePlatformOptions = {}): FakePlatform {
  let connecting: Promise<SessionController> | null = null
  let project: Project = { name: 'app', path: '/work/app' }
  const saved = new Map<string, unknown>([['settings', { firstRunDone: true, ...options.settings }]])
  const platform: FakePlatform = {
    kind: 'host',
    label: 'test',
    can: { openProject: true, restart: true, terminal: false, git: true, pickFolder: true, updates: true, multiChat: options.multiChat === true },
    opened: [],
    restarts: [],
    trustedOnce: [],
    openedProjects: [],
    projectOpens: [],
    launches: [],
    picked: '/work/picked',
    update: options.update ?? null,
    checks: 0,
    manualUpdates: options.manualUpdates ?? false,
    saved,
    notifications: [],
    attention: [],
    notifyResult: 'shown',
    fakeChats: options.multiChat ? new FakeChats(engine) : null,
    notify: async (notification) => {
      platform.notifications.push(notification)
      return platform.notifyResult
    },
    setAttention: (count) => void platform.attention.push(count),
    store: {
      get: async <T,>(key: string) => saved.get(key) as T | undefined,
      set: async (key, value) => void saved.set(key, value),
    },
    setLaunchOptions(launch) {
      platform.launches.push(launch)
    },
    pickFolder: async () => platform.picked,
    appVersion: async () => '0.2.0',
    checkForUpdate: async () => {
      platform.checks++
      return platform.update
    },
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
    async restart(resume, options) {
      if (options?.trustOnce) platform.trustedOnce.push(resume)
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
  if (platform.fakeChats) platform.chats = platform.fakeChats
  return platform
}

/** The chats of a multi-chat platform: each has its own {@link FakeEngine}. */
export class FakeChats implements ChatHost {
  engines = new Map<string, FakeEngine>()
  controllers = new Map<string, SessionController>()
  infos = new Map<string, ChatInfo>()
  opened: Array<{ path?: string; resume?: string | null; worktree?: { name?: string } }> = []
  activated: Array<string | null> = []
  stopped: string[] = []
  closed: string[] = []
  resumed: Array<{ id: string; options?: { resume?: string | null; trustOnce?: boolean } }> = []
  activities = new Map<string, { busy: boolean; waiting: boolean }>()
  discards: Array<{ id: string; force: boolean; deleteUnmerged: boolean }> = []
  /** What `worktreeStatus` answers, and what a discard checks. */
  worktreeState: Omit<WorktreeStatus, 'branch' | 'path'> = { dirty: 0, unmerged: 0 }
  /** Rejects the next `open` with this host error text (`cap_reached`, ...). */
  failNext: string | null = null
  private count = 0
  private stoppedListeners = new Set<(id: string, reason: string) => void>()

  constructor(private readonly first: FakeEngine) {}

  ids(): string[] {
    return [...this.infos.keys()]
  }

  async open(request: { path?: string; resume?: string | null; worktree?: { name?: string } }) {
    this.opened.push(request)
    if (this.failNext) {
      const text = this.failNext
      this.failNext = null
      throw text === 'cap_reached' ? new ChatError('4 chats are working. Stop one to open another.', 'cap_reached') : new ChatError(text)
    }
    const index = this.count++
    const id = `c${index + 1}test`
    const path = request.path ?? '/work/app'
    const slug = request.worktree ? (request.worktree.name ?? `chat-${index + 1}`).toLowerCase().replace(/[^a-z0-9]+/g, '-') : null
    const engine = index === 0 ? this.first : new FakeEngine({ cwd: path })
    const info: ChatInfo = {
      id,
      project: { name: path.split('/').pop() ?? path, path },
      cwd: slug ? `/wt/${slug}` : path,
      branch: slug ? `neuron/${slug}` : 'main',
      worktree: slug ? { path: `/wt/${slug}`, branch: `neuron/${slug}`, base: 'main' } : null,
    }
    const controller = new SessionController(engine)
    this.engines.set(id, engine)
    this.controllers.set(id, controller)
    this.infos.set(id, info)
    await controller.start()
    return { info, controller }
  }

  async reattach() {
    return []
  }

  async resume(id: string, options?: { resume?: string | null; trustOnce?: boolean }) {
    this.resumed.push({ id, options })
    const controller = this.controllers.get(id)
    controller?.apply({ type: 'local/connecting' })
    await this.engines.get(id)?.start()
  }

  async stop(id: string) {
    this.stopped.push(id)
    this.emitStopped(id, 'exited')
  }

  async close(id: string) {
    this.closed.push(id)
    this.infos.delete(id)
  }

  activate(id: string | null) {
    this.activated.push(id)
  }

  reportActivity(id: string, activity: { busy: boolean; waiting: boolean }) {
    this.activities.set(id, activity)
  }

  onStopped(listener: (id: string, reason: string) => void) {
    this.stoppedListeners.add(listener)
    return () => this.stoppedListeners.delete(listener)
  }

  /** The host ended a chat's engine (idle, evicted, ...). */
  emitStopped(id: string, reason: string) {
    for (const listener of this.engines.get(id)?.closeListeners ?? []) listener({ code: 0, error: null })
    for (const listener of [...this.stoppedListeners]) listener(id, reason)
  }

  async list() {
    return [...this.infos.values()]
  }

  async worktreeStatus(id: string): Promise<WorktreeStatus> {
    const info = this.infos.get(id)
    return { branch: info?.worktree?.branch ?? '', path: info?.worktree?.path ?? '', ...this.worktreeState }
  }

  async discardWorktree(id: string, options: { force: boolean; deleteUnmerged: boolean }) {
    // The host checks in this order and removes nothing until both are answered.
    if (this.worktreeState.unmerged > 0 && !options.deleteUnmerged) throw new ChatError('This copy has saved changes that the main folder lacks.', 'unmerged', this.worktreeState.unmerged)
    if (this.worktreeState.dirty > 0 && !options.force) throw new ChatError('This copy has unsaved changes.', 'dirty', this.worktreeState.dirty)
    this.discards.push({ id, ...options })
    this.infos.delete(id)
  }
}
