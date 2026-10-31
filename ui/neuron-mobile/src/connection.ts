/**
 * Pairing with a Neuron host and holding the session. The phone is a remote
 * control: the engine runs on the user's machine (`bun run host --lan` in
 * ui/neuron-client today, the zero-x.live relay later) and every client sees
 * the same session.
 */

import { SessionController, WebSocketTransport, type SocketLike } from '@neuron/client'

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
  })
  const controller = new SessionController(transport)
  await controller.start()
  return { controller, transport }
}
