import { useEffect, useRef, useSyncExternalStore } from 'react'

import { initialSession, type SessionController, type SessionState } from '@neuron/client'

const empty = initialSession()

/** The session's state, re-rendering on every change. */
export function useSession(controller: SessionController | null): SessionState {
  return useSyncExternalStore(
    controller?.subscribe ?? noopSubscribe,
    controller?.getState ?? (() => empty),
    controller?.getState ?? (() => empty),
  )
}

const noopSubscribe = () => () => {}

/** Calls `handler` for keys pressed while `active`, after a short guard so a key meant for something else isn't taken. */
export function useKeys(active: boolean, handler: (event: KeyboardEvent) => void, guardMs = 300): void {
  const handlerRef = useRef(handler)
  handlerRef.current = handler
  useEffect(() => {
    if (!active) return
    const since = Date.now()
    const onKey = (event: KeyboardEvent) => {
      if (Date.now() - since < guardMs) return
      handlerRef.current(event)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [active, guardMs])
}
