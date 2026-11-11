/**
 * What the desktop UI needs from where it runs: inside Tauri (the engine is a
 * child of the app's Rust side) or in a browser during development (the engine
 * sits behind a Neuron host, `bun run host --mock` in ui/neuron-client).
 */

import type { SessionController } from '@neuron/client'

import type { PullRequestSnapshot } from '../pullRequest'

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
  /** L3: how hard the model thinks, for models that take it; the engine's own default when absent. */
  reasoningEffort?: 'low' | 'medium' | 'high'
}

export interface UpdateInfo {
  version: string
  notes: string | null
}

/** Which engine the app runs (L5): `NEURON_BIN`, the one bundled beside the app, one on the PATH, or none. */
export interface EngineInfo {
  source: 'env' | 'bundled' | 'path' | 'missing'
  path: string | null
  bundledPath: string | null
  bundledPresent: boolean
}

export interface Platform {
  kind: 'tauri' | 'host'
  /** Where the engine runs, for the title bar. */
  label: string
  /** What this platform can do beyond chatting. */
  can: { openProject: boolean; restart: boolean; terminal: boolean; git: boolean; pickFolder: boolean; updates: boolean; multiChat: boolean }
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
  /** True when this build cannot install updates: new versions are on the releases page. */
  manualUpdates?: boolean
  /** A newer release, if the update manifest names one. */
  checkForUpdate(): Promise<UpdateInfo | null>
  /**
   * A new engine on the same project, continuing `resume` (a session id).
   * `trustOnce` trusts the folder for that engine only (the home folder, which
   * is never recorded). With several chats (`can.multiChat`), the chat in view's.
   */
  restart(resume: string | null, options?: { trustOnce?: boolean }): Promise<void>
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
  // L5: extensions and resilience
  /** Which engine runs; `null` where the app does not start it (a browser). */
  engineInfo(): Promise<EngineInfo | null>
  /** The host's part of the support report (app version, OS, engine); `null` in a browser. Never holds account data. */
  hostDiagnostics(): Promise<string | null>
  /**
   * L3: copies a pasted or dropped image into the app's staging folder and returns the
   * path the engine can read. Png, jpeg, gif and webp, at most 10 MB; rejects otherwise.
   */
  stageAttachment(name: string, data: Uint8Array, mediaType: string): Promise<{ path: string; media_type: string }>
  /**
   * L3: saves a chat export. The system's save dialog picks the file (the app never
   * names the path); returns the saved path, or `null` when cancelled.
   */
  exportSave(suggestedName: string, content: string): Promise<string | null>
  // L2: review and git. Both act in the chat in view's folder (its own copy for a chat in a worktree).
  gitRevert(path: string): Promise<void>
  /** A file of the folder, or `.` for the folder itself (L4's chat bar). */
  openInEditor(path: string): Promise<void>
  /** L4: several chats at once; absent where one engine serves the window (a browser host). */
  chats?: ChatHost
  /** L4: a system notification for a chat that needs the user; `denied` or `unsupported` when it cannot show one. */
  notify(notification: { chat: string; title: string; body: string }): Promise<'shown' | 'denied' | 'unsupported'>
  /** L4: how many chats need the user, on the taskbar or dock icon and in the window title. */
  setAttention(count: number): void
  /**
   * v1.1: the bytes of an image, video or sound in the chat in view's folder, for the
   * Files tab (size-capped by the host); absent where the host cannot send them.
   */
  readMedia?(path: string): Promise<Uint8Array>
  /**
   * v1.1: the host's question before quitting while chats work. It asks with a
   * numbered request; the window answers it once (Quit or Keep working).
   */
  quit?: {
    onRequest(listener: (request: QuitRequest) => void): () => void
    answer(id: number, quit: boolean): void
  }
}

/** v1.1: the host is about to close the window while chats work or wait for the user. */
export interface QuitRequest {
  id: number
  /** Chats working on a turn. */
  working: number
  /** Chats waiting for the user's answer or approval. */
  waiting: number
}

/** L4: a copy of the project a chat works in (a git worktree). */
export interface WorktreeInfo {
  path: string
  branch: string
  base: string
  /** v1.1: false until the copy has a name of its own; the chat's first prompt gives it one. */
  named?: boolean
}

/** v1.1: the branches a copy can start from, and the one it starts from by default. */
export interface BranchList {
  branches: string[]
  default: string | null
}

/**
 * v1.1: one file a chat's branch changed since it left its base, committed or not.
 * Shape ported from Freebuff, common/src/desktop-cloud/changes.ts (`ChangedFile`), commit 0dc95cb
 * (https://github.com/CodebuffAI/freebuff). Copyright Codebuff, Apache License 2.0. Changed for
 * Neuron: no modification time; see ../../NOTICE.
 */
export interface BranchFile {
  /** Relative to the copy's top folder. */
  path: string
  status: 'added' | 'modified' | 'deleted'
  /** Lines added and removed; `null` for a binary file. */
  adds: number | null
  dels: number | null
  binary: boolean
  /** New, and not known to git yet. */
  untracked: boolean
}

/** v1.1: everything a chat's branch changed since it left its base (its merge base), plus what is not committed. */
export interface BranchChanges {
  branch: string | null
  /** The branch or commit the copy was made from. */
  base: string
  /** Commits on the branch since it left the base. */
  commits: number
  files: BranchFile[]
  totals: { files: number; adds: number; dels: number }
  /** More files changed than are listed. */
  truncated: boolean
}

export interface WorktreeStatus {
  branch: string
  path: string
  /** Changed or new files. */
  dirty: number
  /** Commits the main folder lacks. */
  unmerged: number
}

/** L4: one chat as the host knows it. */
export interface ChatInfo {
  id: string
  project: Project
  cwd: string
  branch: string | null
  worktree: WorktreeInfo | null
}

/** Why a refused host call was refused, for the few the UI answers itself. */
export type ChatErrorCode = 'cap_reached' | 'too_many_chats' | 'unmerged' | 'dirty' | 'has_worktree' | 'discarding' | 'starting'

export class ChatError extends Error {
  constructor(
    message: string,
    readonly code?: ChatErrorCode,
    /** For `unmerged` and `dirty`: how many commits or files. */
    readonly count?: number,
  ) {
    super(message)
  }
}

/** L4: the chats of one window, each with its own engine and controller. */
export interface ChatHost {
  /**
   * Opens a chat in `path` (the first project when absent), in a copy of its own when `worktree` is given.
   * The copy starts from `worktree.start` (an existing local or remote branch), else the repository's default branch.
   */
  open(request: { path?: string; resume?: string | null; worktree?: { name?: string; start?: string } }): Promise<{ info: ChatInfo; controller: SessionController }>
  /** Chats a reloaded window finds already open; they start again where they were. */
  reattach(): Promise<Array<{ info: ChatInfo; controller: SessionController }>>
  /** A new engine for the chat; also starts a stopped one. `resume`: a session id, `''` for a fresh one. */
  resume(id: string, options?: { resume?: string | null; trustOnce?: boolean }): Promise<void>
  stop(id: string): Promise<void>
  /** Closes the chat. Rejects with a {@link ChatError} `has_worktree` for a chat with a copy of its own: discard the copy first. */
  close(id: string): Promise<void>
  /** The chat in view. Files, git and terminals follow it. */
  activate(id: string | null): void
  /** What the chat's state says, so a working or waiting engine is never stopped for idleness. */
  reportActivity(id: string, activity: { busy: boolean; waiting: boolean }): void
  /** Called when a chat's engine ends: `exited`, `idle`, `evicted` or `discarded`. */
  onStopped(listener: (id: string, reason: string) => void): () => void
  list(): Promise<ChatInfo[]>
  worktreeStatus(id: string): Promise<WorktreeStatus>
  /** Removes the chat's copy and forgets the chat. Rejects with a {@link ChatError} `unmerged` or `dirty` until confirmed. */
  discardWorktree(id: string, options: { force: boolean; deleteUnmerged: boolean }): Promise<void>
  // v1.1, where the host offers them
  /** The branches a copy of `path`'s repository can start from. */
  branches?(path?: string): Promise<BranchList>
  /** What the chat's copy changed since its branch left its base. */
  branchChanges?(id: string): Promise<BranchChanges>
  /** One file's change on the chat's branch, as a unified diff. */
  branchDiff?(id: string, path: string): Promise<string>
  /** The pull request of the chat's branch; nothing when none can be known. */
  pullRequest?(id: string): Promise<PullRequestSnapshot>
}
