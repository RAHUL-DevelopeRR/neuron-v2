/**
 * What the desktop UI needs from where it runs: inside Tauri (the engine is a
 * sidecar of the app's Rust side) or in a browser during development (the
 * engine sits behind a Neuron host, `bun run host --mock` in ui/neuron-client).
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

export interface Platform {
  kind: 'tauri' | 'host'
  /** Where the engine runs, for the title bar. */
  label: string
  /** Starts (or attaches to) the engine. */
  connect(): Promise<SessionController>
  list(path: string): Promise<DirListing>
  read(path: string): Promise<FileContent>
  /** A shell in the workspace; rejects where there is none. */
  openTerminal(cols: number, rows: number): Promise<TerminalSession>
  openExternal(url: string): void
}
