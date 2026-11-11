/**
 * Pairing with a Neuron host and holding the session. The phone is a remote
 * control: the engine runs on the user's machine (`bun run host --lan` in
 * ui/neuron-client today, the zero-x.live relay later) and every client sees
 * the same session.
 */

import { SessionController, WebSocketTransport, type ConnectionHealth, type SocketLike } from '@neuron/client'

/**
 * A host address as people type it, as a WebSocket URL: `192.168.1.5:51967`,
 * `ws://…`, `http://…` (read as ws), with `/neuron` added when there is no path.
 */
export function normalizeHostUrl(input: string): string {
  let text = input.trim()
  if (!text) throw new Error('Enter the address the Neuron host printed.')
  if (/^https?:\/\//i.test(text)) text = text.replace(/^http/i, 'ws')
  if (!/^wss?:\/\//i.test(text)) text = `ws://${text}`
  let url: URL
  try {
    url = new URL(text)
  } catch {
    throw new Error(`"${input.trim()}" is not an address.`)
  }
  if (url.search || url.hash) throw new Error('The address has no query or fragment; the token goes in its own field.')
  if (url.pathname === '/' || url.pathname === '') url.pathname = '/neuron'
  return url.toString()
}

export interface Connection {
  controller: SessionController
  transport: WebSocketTransport
}

/** Pairs with the host and attaches to its session. */
export async function connect(
  address: string,
  token: string,
  options: { name?: string; socket?: (url: string) => SocketLike; timeoutMs?: number } = {},
): Promise<Connection> {
  if (!token.trim()) throw new Error('Enter the pairing token from the Neuron host.')
  const transport = new WebSocketTransport({
    url: normalizeHostUrl(address),
    token: token.trim(),
    client: { name: options.name ?? 'Neuron Mobile', kind: 'mobile' },
    connect: options.socket,
    timeoutMs: options.timeoutMs ?? 15_000,
    // A phone loses its network and sleeps: once paired, it comes back by itself.
    reconnect: true,
  })
  const controller = new SessionController(transport)
  await controller.start()
  return { controller, transport }
}

/** The connection in plain words for the session screen; null while all is well. */
export function healthText(health: ConnectionHealth): string | null {
  switch (health.state) {
    case 'connected':
      return null
    case 'connecting':
      return 'Connecting…'
    case 'reconnecting':
      return health.retryInMs > 0
        ? `Reconnecting… (${health.attempt}), next try in ${Math.ceil(health.retryInMs / 1000)} s`
        : `Reconnecting… (${health.attempt})`
    case 'offline':
      return health.reason ? `Offline: ${health.reason}` : 'Offline'
  }
}

/** The part of React Native's `AppState` this uses. */
export interface AppStateLike {
  currentState: string | null | undefined
  addEventListener(type: 'change', listener: (state: string) => void): { remove(): void }
}

/**
 * Wakes the connection each time the app returns to the foreground: a socket
 * the system froze in the background is most likely dead, and waiting for the
 * watchdog would leave the session stale for half a minute.
 */
export function watchForeground(appState: AppStateLike, transport: { wake(): void }): () => void {
  let last = appState.currentState
  const subscription = appState.addEventListener('change', (next) => {
    if (next === 'active' && last !== 'active') transport.wake()
    last = next
  })
  return () => subscription.remove()
}
