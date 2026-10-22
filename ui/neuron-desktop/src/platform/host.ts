/** The desktop UI in a browser, attached to a Neuron host over WebSocket. */

import { SessionController, WebSocketTransport, type SocketLike } from '@neuron/client'

import type { DirListing, FileContent, Platform, TerminalSession } from './types'

export interface HostPlatformOptions {
  url: string
  token: string
  /** Makes the socket; the global WebSocket by default. */
  connect?: (url: string) => SocketLike
}

export function hostPlatform(options: HostPlatformOptions): Platform {
  const transport = new WebSocketTransport({
    url: options.url,
    token: options.token,
    client: { name: 'Neuron Desktop', kind: 'desktop' },
    connect: options.connect,
  })
  // Once per page: React may run the effect that connects twice.
  let connecting: Promise<SessionController> | null = null
  return {
    kind: 'host',
    label: new URL(options.url.replace(/^ws/, 'http')).host,
    connect() {
      connecting ??= (async () => {
        const controller = new SessionController(transport)
        await controller.start()
        return controller
      })()
      return connecting
    },
    list: (path) => transport.rpc<DirListing>('fs.list', { path }),
    read: (path) => transport.rpc<FileContent>('fs.read', { path }),
    async openTerminal(cols, rows): Promise<TerminalSession> {
      const { id } = await transport.rpc<{ id: number }>('term.open', { cols, rows })
      const dataListeners = new Set<(data: string) => void>()
      const exitListeners = new Set<(code: number | null) => void>()
      const unsubscribe = transport.onTerminal((frame) => {
        if (frame.id !== id) return
        if (frame.t === 'term_output') for (const listener of dataListeners) listener(frame.data)
        else {
          for (const listener of exitListeners) listener(frame.code)
          unsubscribe()
        }
      })
      return {
        write: (data) => transport.terminalInput(id, data),
        resize: (cols, rows) => transport.terminalResize(id, cols, rows),
        close: () => {
          unsubscribe()
          void transport.rpc('term.close', { id }).catch(() => {})
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
    openExternal: (url) => window.open(url, '_blank', 'noopener,noreferrer'),
  }
}
