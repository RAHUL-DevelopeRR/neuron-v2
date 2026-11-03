import { useSyncExternalStore } from 'react'

import type { SessionController, SessionState } from '@neuron/client'

export function useSession(controller: SessionController): SessionState {
  return useSyncExternalStore(controller.subscribe, controller.getState, controller.getState)
}
