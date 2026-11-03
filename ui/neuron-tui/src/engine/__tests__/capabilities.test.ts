import { afterEach, describe, expect, test } from 'bun:test'

import { useEngineStore } from '../../state/engine-store'
import { UI_CAPABILITIES, declareUiCapabilities } from '../capabilities'
import { handleEngineEvent } from '../engine-session'
import {
  setNeuronEngineClientForTesting,
  type NeuronEngineClient,
} from '../neuron-engine-client'
import { commandMessage, encodeClientMessage } from '../protocol'

import type { EngineCommand, ReadyEvent } from '../protocol'

const ready = (features?: string[]): ReadyEvent => ({
  type: 'ready',
  protocol: 1,
  version: '0.9.0',
  ...(features && { features }),
})

/** A client that records what is sent. */
function recordingClient() {
  const sent: EngineCommand[] = []
  const client = { send: (command: EngineCommand) => sent.push(command) }
  return { sent, client }
}

afterEach(() => {
  setNeuronEngineClientForTesting(null)
  useEngineStore.getState().reset()
})

describe('capabilities', () => {
  test('declares reports, live progress and git views, as the protocol spells them', () => {
    expect(UI_CAPABILITIES).toEqual(['reports', 'live_progress', 'git_views'])
    const { sent, client } = recordingClient()
    expect(
      declareUiCapabilities(client, ready(['capabilities', 'live_progress', 'git_views'])),
    ).toBe(true)
    expect(sent).toEqual([
      {
        type: 'capabilities',
        features: ['reports', 'live_progress', 'git_views'],
        client: 'terminal',
      },
    ])
    expect(encodeClientMessage(commandMessage(sent[0]!))).toBe(
      '{"kind":"command","type":"capabilities","features":["reports","live_progress","git_views"],"client":"terminal"}\n',
    )
  })

  test('an engine without the feature (or without features) gets nothing', () => {
    const { sent, client } = recordingClient()
    expect(declareUiCapabilities(client, ready())).toBe(false)
    expect(declareUiCapabilities(client, ready(['live_progress']))).toBe(false)
    expect(sent).toEqual([])
  })

  test('a failed write is not thrown at the event handler', () => {
    const client = {
      send: () => {
        throw new Error('EPIPE')
      },
    }
    expect(declareUiCapabilities(client, ready(['capabilities']))).toBe(false)
  })

  test('the session sends it once, on ready', () => {
    const { sent, client } = recordingClient()
    setNeuronEngineClientForTesting(client as unknown as NeuronEngineClient)
    handleEngineEvent(ready(['capabilities', 'live_progress']))
    handleEngineEvent({ type: 'turn_finished' })
    expect(sent.map((command) => command.type)).toEqual(['capabilities'])
    expect(useEngineStore.getState().version).toBe('0.9.0')

    sent.length = 0
    handleEngineEvent(ready())
    expect(sent).toEqual([])
  })
})
