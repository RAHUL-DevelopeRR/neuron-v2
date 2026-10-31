/**
 * A fake engine for the git-view tests: it records the commands the UI sends
 * and plays the engine's events through `handleEngineEvent` (as the real
 * session's subscription does) and to `runExchange`'s listeners.
 */

import { handleEngineEvent } from '../../engine/engine-session'
import {
  setNeuronEngineClientForTesting,
  type EngineConnection,
  type NeuronEngineClient,
} from '../../engine/neuron-engine-client'

import type { EngineCommand, UiEvent } from '../../engine/protocol'

export interface FakeGitEngine {
  engine: NeuronEngineClient
  sent: EngineCommand[]
  emit: (...events: UiEvent[]) => void
}

export function installFakeGitEngine(connection: EngineConnection = 'ready'): FakeGitEngine {
  const sent: EngineCommand[] = []
  const listeners = new Set<(event: UiEvent) => void>()
  const engine = {
    connection,
    send: (command: EngineCommand) => sent.push(command),
    slash: (command: string, args = '') => sent.push({ type: 'slash', command, args }),
    subscribe: (listener: (event: UiEvent) => void) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    onExit: () => () => {},
  } as unknown as NeuronEngineClient
  setNeuronEngineClientForTesting(engine)
  const emit = (...events: UiEvent[]) => {
    for (const event of events) {
      handleEngineEvent(event)
      for (const listener of [...listeners]) listener(event)
    }
  }
  return { engine, sent, emit }
}

/** Lets `runExchange` send (it waits for the exchanges before it). */
export const flushExchanges = () => new Promise((resolve) => setTimeout(resolve, 0))
