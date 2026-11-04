import { afterEach, describe, expect, mock, test } from 'bun:test'

import { isColonyStop, startsColonyRun } from '../../engine/colony-control'
import {
  setNeuronEngineClientForTesting,
  type NeuronEngineClient,
} from '../../engine/neuron-engine-client'
import { routeUserPrompt } from '../router'

import type { UiEvent } from '../../engine/protocol'
import type { RouterParams } from '../command-registry'

/** An engine that records what it was sent; `finish` ends the exchange in progress. */
function fakeEngine() {
  const listeners = new Set<(event: UiEvent) => void>()
  const sent: string[] = []
  const engine = {
    connection: 'ready',
    slash: (command: string, args = '') => sent.push(`/${command} ${args}`.trim()),
    interrupt: () => sent.push('interrupt'),
    subscribe: (listener: (event: UiEvent) => void) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    onExit: () => () => {},
  } as unknown as NeuronEngineClient
  const finish = () => {
    for (const listener of [...listeners]) {
      listener({ type: 'turn_finished' } as unknown as UiEvent)
    }
  }
  return { engine, sent, finish }
}

function params(inputValue: string) {
  const lines: string[] = []
  const setMessages = mock((update: any) => {
    for (const message of update([])) lines.push(`${message.variant}: ${message.content}`)
  })
  return {
    lines,
    params: {
      agentMode: 'DEFAULT',
      inputRef: { current: null },
      inputValue,
      isChainInProgressRef: { current: false },
      isStreaming: false,
      streamMessageIdRef: { current: null },
      addToQueue: mock(() => {}),
      hasQueuedMessages: () => false,
      saveToHistory: mock(() => {}),
      scrollToLatest: mock(() => {}),
      sendMessage: mock(async () => {}),
      setInputFocused: mock(() => {}),
      setInputValue: mock(() => {}),
      setMessages,
    } as unknown as RouterParams,
  }
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0))

afterEach(() => setNeuronEngineClientForTesting(null))

describe('/colony stop', () => {
  test('during a run it interrupts the run instead of queuing behind it', async () => {
    const { engine, sent, finish } = fakeEngine()
    setNeuronEngineClientForTesting(engine)

    await routeUserPrompt(params('/colony add tests').params)
    await settle()
    expect(sent).toEqual(['/colony add tests'])

    const stop = params('/colony stop')
    await routeUserPrompt(stop.params)
    await settle()
    expect(sent).toEqual(['/colony add tests', 'interrupt'])
    expect(stop.lines).toContain('ai: Stopping the Colony run…')

    // The run's exchange ends; with no run going, it goes to the engine, which
    // says so.
    finish()
    await settle()
    await routeUserPrompt(params('/colony stop').params)
    await settle()
    expect(sent).toEqual(['/colony add tests', 'interrupt', '/colony stop'])
    finish()
    await settle()
  })

  test('a status or stop request does not count as a run', async () => {
    const { engine, sent, finish } = fakeEngine()
    setNeuronEngineClientForTesting(engine)

    await routeUserPrompt(params('/colony status').params)
    await settle()
    await routeUserPrompt(params('/colony stop').params)
    await settle()
    finish()
    await settle()
    finish()
    await settle()
    expect(sent).toEqual(['/colony status', '/colony stop'])
  })

  test('arguments', () => {
    expect(isColonyStop(['stop'])).toBe(true)
    expect(isColonyStop(['STOP'])).toBe(true)
    expect(isColonyStop(['stop', 'now'])).toBe(false)
    expect(startsColonyRun(['add', 'tests'])).toBe(true)
    expect(startsColonyRun(['stop'])).toBe(false)
    expect(startsColonyRun(['status'])).toBe(false)
    expect(startsColonyRun([''])).toBe(false)
  })
})
