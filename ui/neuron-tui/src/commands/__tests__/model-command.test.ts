import { afterEach, describe, expect, mock, test } from 'bun:test'

import {
  setNeuronEngineClientForTesting,
  type NeuronEngineClient,
} from '../../engine/neuron-engine-client'
import { COMMAND_REGISTRY, type RouterParams } from '../command-registry'

function engineWith(connection: NeuronEngineClient['connection']) {
  const models: string[] = []
  const engine = {
    connection,
    setModel: (model: string) => models.push(model),
  } as unknown as NeuronEngineClient
  return { engine, models }
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
      saveToHistory: mock(() => {}),
      setInputValue: mock(() => {}),
      setMessages,
    } as unknown as RouterParams,
  }
}

const model = COMMAND_REGISTRY.find((command) => command.name === 'model')!

afterEach(() => setNeuronEngineClientForTesting(null))

describe('/model', () => {
  test('without a name it opens the picker', async () => {
    setNeuronEngineClientForTesting(engineWith('ready').engine)
    expect(await model.handler(params('/model').params, '')).toEqual({ openModelPicker: true })
  })

  test('with a name it switches at once', async () => {
    const { engine, models } = engineWith('ready')
    setNeuronEngineClientForTesting(engine)
    const { params: p, lines } = params('/model opus')
    expect(await model.handler(p, ' opus ')).toBeUndefined()
    expect(models).toEqual(['opus'])
    expect(lines).toEqual(['user: /model opus'])
  })

  test('without a running engine it says so', async () => {
    const { engine, models } = engineWith('exited')
    setNeuronEngineClientForTesting(engine)
    const { params: p, lines } = params('/model')
    expect(await model.handler(p, '')).toBeUndefined()
    expect(models).toEqual([])
    expect(lines[0]).toContain('not running')
  })
})
