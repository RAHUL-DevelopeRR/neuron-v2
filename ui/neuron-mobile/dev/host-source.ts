/**
 * Shares a session that a `NeuronHost` (ui/neuron-client, Bun) serves: the
 * bridge connects to it over loopback like any other client, and reads its
 * state for presence. `neuron engine --share` does the same inside the engine.
 */

import type { NeuronHost } from '../../neuron-client/src/node/host'
import { presenceOf } from '../src/relay/presence'
import type { LocalConnection, ShareSource } from '../src/relay/share-host'

export function neuronHostSource(host: NeuronHost, options: { token: string; shareId: string; root: string }): ShareSource {
  return {
    shareId: options.shareId,
    token: options.token,
    connect() {
      return new Promise<LocalConnection>((resolve, reject) => {
        const socket = new WebSocket(host.url)
        const connection: LocalConnection = {
          send: (text) => socket.send(text),
          close: () => socket.close(1000, 'phone left'),
          onmessage: null,
          onclose: null,
        }
        socket.onopen = () => resolve(connection)
        socket.onerror = () => reject(new Error('Could not reach the local Neuron host.'))
        socket.onmessage = (event) => connection.onmessage?.(typeof event.data === 'string' ? event.data : String(event.data))
        socket.onclose = () => connection.onclose?.()
      })
    },
    presence: () => presenceOf(host.state, host.seq, options.root),
    onChange(listener) {
      // The host has no change event; its `seq` goes up with every action.
      let seen = host.seq
      const timer = setInterval(() => {
        if (host.seq === seen) return
        seen = host.seq
        listener()
      }, 100)
      return () => clearInterval(timer)
    },
  }
}
