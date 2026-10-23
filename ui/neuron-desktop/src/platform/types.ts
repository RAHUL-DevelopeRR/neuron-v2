/**
 * What the desktop UI needs from where it runs: inside Tauri (the engine is a
 * child of the app's Rust side) or in a browser during development (the engine
 * sits behind a Neuron host, `bun run host --mock` in ui/neuron-client).
 */

import type { SessionController } from '@neuron/client'

export interface DirEntry {
  name: string
  /** Relative to the workspace, with `/`. */
  path: string
  dir: boolean
  size: number | null
}

export interface DirListing {
  path: string
  entries: DirEntry[]
  truncated: boolean
}

export interface FileContent {
  path: string
  content: string
  truncated: boolean
  binary: boolean
  size: number
}

export interface TerminalSession {
  write(data: string): void
  resize(cols: number, rows: number): void
  close(): void
  onData(listener: (data: string) => void): () => void
  onExit(listener: (code: number | null) => void): () => void
}

export interface GitChange {
  path: string
  /** Porcelain XY: the index's state, then the work tree's (`M`, `A`, `D`, `R`, `?`, ` `). */
  index: string
  worktree: string
}

export interface GitStatusView {
  branch: string | null
  changes: GitChange[]
}

export interface Project {
  name: string
  path: string
}

/** Small values the app keeps between launches: settings, and which sessions were looked at. */
export interface KeyValueStore {
  get<T>(key: string): Promise<T | undefined>
  set(key: string, value: unknown): Promise<void>
}

/** What new engines start with: Settings › Sessions. */
export interface LaunchOptions {
  model?: string
  permissionMode?: string
}

export interface UpdateInfo {
  version: string
  notes: string | null
}

export interface Platform {
  kind: 'tauri' | 'host'
  /** Where the engine runs, for the title bar. */
  label: string
  /** What this platform can do beyond chatting. */
  can: { openProject: boolean; restart: boolean; terminal: boolean; git: boolean; pickFolder: boolean; updates: boolean }
  store: KeyValueStore
  /** For the engines started from now on. */
  setLaunchOptions(options: LaunchOptions): void
  /** Starts (or attaches to) the engine. */
  connect(): Promise<SessionController>
  /** The open project, once connected. */
  project(): Project | null
  /** Projects opened before, newest first. */
  recentProjects(): Promise<Project[]>
  /** Opens another folder: a new engine there, on a fresh chat or continuing `resume`. */
  openProject(path: string, resume?: string | null): Promise<void>
  /** The system's folder picker; `null` when cancelled. */
  pickFolder(): Promise<string | null>
  /** This app's version; `null` in a browser build. */
  appVersion(): Promise<string | null>
  /** A newer release, if the update manifest names one. */
  checkForUpdate(): Promise<UpdateInfo | null>
  /** A new engine on the same project, continuing `resume` (a session id). */
  restart(resume: string | null): Promise<void>
  list(path: string): Promise<DirListing>
  read(path: string): Promise<FileContent>
  /** Workspace files whose path contains `query`, for `@` mentions. */
  searchFiles(query: string): Promise<string[]>
  /** `null` outside a git repository. */
  gitStatus(): Promise<GitStatusView | null>
  /** One file's change against HEAD, as a unified diff. */
  gitDiff(path: string): Promise<string>
  /** A shell in the workspace; rejects where there is none. */
  openTerminal(cols: number, rows: number): Promise<TerminalSession>
  openExternal(url: string): void
}
