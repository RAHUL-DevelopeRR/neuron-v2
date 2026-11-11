import { useEffect, useSyncExternalStore } from 'react'
import { AppState } from 'react-native'

import type { ConnectionHealth, SessionController, SessionState, WebSocketTransport } from '@neuron/client'

import { watchForeground } from './connection'

export function useSession(controller: SessionController): SessionState {
  return useSyncExternalStore(controller.subscribe, controller.getState, controller.getState)
}

/** Where the connection to the computer stands; the connection wakes when the app returns to the foreground. */
export function useConnectionHealth(transport: WebSocketTransport): ConnectionHealth {
  useEffect(() => watchForeground(AppState, transport), [transport])
  return useSyncExternalStore(
    (listener) => transport.onHealth(listener),
    () => transport.health,
    () => transport.health,
  )
}
