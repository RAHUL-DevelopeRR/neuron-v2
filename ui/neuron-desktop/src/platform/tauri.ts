/**
 * The desktop UI inside Tauri: the app's Rust side (src-tauri) runs
 * `neuron engine --stdio` and passes its lines through. Commands go out with
 * `invoke`, engine lines and terminal output come back as events.
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

import type { DirListing, FileContent, Platform, TerminalSession } from './types'

interface EngineExit {
  code: number | null
  error: string | null
}

class TauriTransport implements EngineTransport {
  readonly echoesActions = false
  private listeners = new Set<(action: SessionAction) => void>()
  private closeListeners = new Set<(close: TransportClose) => void>()
  private unlisten: UnlistenFn[] = []
  private ready = false

  async start(): Promise<void> {
    let resolveReady!: () => void
    let rejectReady!: (error: Error) => void
    const readyOrFailed = new Promise<void>((resolve, reject) => {
      resolveReady = resolve
      rejectReady = reject
    })
    let launchError: string | null = null
    this.unlisten.push(
      await listen<string>('neuron://engine', ({ payload }) => {
        const parsed = parseUiEventLine(payload)
        if (!parsed.ok) return
        if (parsed.event.type === 'ready') {
          this.ready = true
          resolveReady()
        } else if (parsed.event.type === 'error' && !this.ready) {
          launchError = parsed.event.message
        }
        for (const listener of [...this.listeners]) listener(parsed.event)
      }),
      await listen<EngineExit>('neuron://engine-exit', ({ payload }) => {
        const wasReady = this.ready
        this.ready = false
        const error = launchError ?? payload.error ?? (payload.code !== 0 || !wasReady ? `The Neuron engine stopped (exit code ${payload.code}).` : null)
        rejectReady(new Error(error ?? 'The Neuron engine stopped before it was ready.'))
        for (const listener of [...this.closeListeners]) listener({ code: payload.code, error })
      }),
    )
    await invoke('engine_start')
    await readyOrFailed
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
  // Once per window: React may run the effect that connects twice.
  let connecting: Promise<SessionController> | null = null
  return {
    kind: 'tauri',
    label: 'this computer',
    connect() {
      connecting ??= (async () => {
        const controller = new SessionController(new TauriTransport())
        await controller.start()
        return controller
      })()
      return connecting
    },
    list: (path) => invoke<DirListing>('fs_list', { path }),
    read: (path) => invoke<FileContent>('fs_read', { path }),
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
