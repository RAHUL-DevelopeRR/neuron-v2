/**
 * The desktop UI inside Tauri: the app's Rust side (src-tauri) runs
 * `neuron engine --stdio` in the open project and passes its lines through.
 * Commands go out with `invoke`; engine lines and terminal output come back
 * as events. Each engine the app starts has a generation, so the last lines of
 * one that was restarted never reach the next.
 */

import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import {
  SessionController,
  parseUiEventLine,
  type ClientMessage,
  type EngineTransport,
  type SessionAction,
  type TransportClose,
} from '@neuron/client'

import type { DirListing, FileContent, GitStatusView, Platform, Project, TerminalSession } from './types'

interface EngineLine {
  generation: number
  line: string
}

interface EngineExit {
  generation: number
  code: number | null
  error: string | null
}

interface Started {
  generation: number
  project: Project
}

class TauriTransport implements EngineTransport {
  readonly echoesActions = false
  project: Project | null = null
  private generation = -1
  private listeners = new Set<(action: SessionAction) => void>()
  private closeListeners = new Set<(close: TransportClose) => void>()
  private unlisten: UnlistenFn[] = []
  private ready = false
  private waiting: { resolve: () => void; reject: (error: Error) => void } | null = null
  private launchError: string | null = null

  private async listenOnce(): Promise<void> {
    if (this.unlisten.length) return
    this.unlisten.push(
      await listen<EngineLine>('neuron://engine', ({ payload }) => {
        if (payload.generation !== this.generation) return
        const parsed = parseUiEventLine(payload.line)
        if (!parsed.ok) return
        if (parsed.event.type === 'ready') {
          this.ready = true
          this.waiting?.resolve()
        } else if (parsed.event.type === 'error' && !this.ready) {
          this.launchError = parsed.event.message
        }
        for (const listener of [...this.listeners]) listener(parsed.event)
      }),
      await listen<EngineExit>('neuron://engine-exit', ({ payload }) => {
        if (payload.generation !== this.generation) return
        const wasReady = this.ready
        this.ready = false
        const error =
          this.launchError ?? payload.error ?? (payload.code !== 0 || !wasReady ? `The Neuron engine stopped (exit code ${payload.code}).` : null)
        this.waiting?.reject(new Error(error ?? 'The Neuron engine stopped before it was ready.'))
        for (const listener of [...this.closeListeners]) listener({ code: payload.code, error })
      }),
    )
  }

  /** Runs `launch` (which starts an engine and returns its generation), then waits for its `ready`. */
  private async launch(launch: () => Promise<Started>): Promise<void> {
    await this.listenOnce()
    this.ready = false
    this.launchError = null
    const readyOrFailed = new Promise<void>((resolve, reject) => (this.waiting = { resolve, reject }))
    const started = await launch()
    this.generation = started.generation
    this.project = started.project
    try {
      await readyOrFailed
    } finally {
      this.waiting = null
    }
  }

  start(): Promise<void> {
    return this.launch(() => invoke<Started>('engine_start'))
  }

  restart(resume: string | null): Promise<void> {
    return this.launch(() => invoke<Started>('engine_restart', { resume }))
  }

  openProject(path: string): Promise<void> {
    return this.launch(() => invoke<Started>('project_open', { path }))
  }

  send(message: ClientMessage): void {
    if (!this.ready) throw new Error('The Neuron engine is not running.')
    void invoke('engine_send', { line: JSON.stringify(message) })
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
    await invoke('engine_stop')
    for (const unlisten of this.unlisten) unlisten()
    this.unlisten = []
  }
}

interface TerminalOutput {
  id: number
  data: string
}

interface TerminalExit {
  id: number
  code: number | null
}

export function tauriPlatform(): Platform {
  const transport = new TauriTransport()
  // Once per window: React may run the effect that connects twice.
  let connecting: Promise<SessionController> | null = null
  let controller: SessionController | null = null
  return {
    kind: 'tauri',
    label: 'this computer',
    can: { openProject: true, restart: true, terminal: true, git: true },
    connect() {
      connecting ??= (async () => {
        controller = new SessionController(transport)
        await controller.start()
        return controller
      })()
      return connecting
    },
    project: () => transport.project,
    recentProjects: () => invoke<Project[]>('projects_recent'),
    async openProject(path) {
      // The old chat belongs to the old folder.
      controller?.apply({ type: 'local/reset' })
      await transport.openProject(path)
    },
    restart: (resume) => transport.restart(resume),
    list: (path) => invoke<DirListing>('fs_list', { path }),
    read: (path) => invoke<FileContent>('fs_read', { path }),
    searchFiles: (query) => invoke<string[]>('fs_search', { query }),
    gitStatus: () => invoke<GitStatusView | null>('git_status'),
    gitDiff: (path) => invoke<string>('git_diff', { path }),
    async openTerminal(cols, rows): Promise<TerminalSession> {
      const id = await invoke<number>('term_open', { cols, rows })
      const dataListeners = new Set<(data: string) => void>()
      const exitListeners = new Set<(code: number | null) => void>()
      const unlisten = [
        await listen<TerminalOutput>('neuron://term-output', ({ payload }) => {
          if (payload.id === id) for (const listener of dataListeners) listener(payload.data)
        }),
        await listen<TerminalExit>('neuron://term-exit', ({ payload }) => {
          if (payload.id === id) for (const listener of exitListeners) listener(payload.code)
        }),
      ]
      return {
        write: (data) => void invoke('term_write', { id, data }),
        resize: (cols, rows) => void invoke('term_resize', { id, cols, rows }),
        close: () => {
          for (const stop of unlisten) stop()
          void invoke('term_close', { id })
        },
        onData: (listener) => {
          dataListeners.add(listener)
          return () => dataListeners.delete(listener)
        },
        onExit: (listener) => {
          exitListeners.add(listener)
          return () => exitListeners.delete(listener)
        },
      }
    },
    openExternal: (url) => void invoke('open_external', { url }),
  }
}
