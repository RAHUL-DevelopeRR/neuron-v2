import { afterEach, beforeAll, beforeEach, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import {
  setNeuronEngineClientForTesting,
  type NeuronEngineClient,
} from '../../engine/neuron-engine-client'
import { initializeThemeStore } from '../../hooks/use-theme'
import { useEngineStore } from '../../state/engine-store'
import { ModelPicker } from '../model-picker'

import type { ModelsEvent } from '../../engine/protocol'

// Fourteen models, like the gateway's catalog in the end-to-end run, so the
// eight-row window has to scroll to show the current one.
const IDS = ['auto', ...Array.from({ length: 13 }, (_, index) => `model-${index + 1}`)]

const catalog = (current: string): ModelsEvent => ({
  type: 'models',
  current,
  models: IDS.map((id) => ({
    id,
    display_name: id === 'auto' ? 'Auto' : `Model ${id.slice('model-'.length)}`,
    available: true,
    unavailable_reason: null,
    context_window: null,
    tiers: [],
  })),
})

let cleanup: (() => void) | undefined
const switched: string[] = []

beforeAll(() => {
  initializeThemeStore()
})

beforeEach(() => {
  switched.length = 0
  useEngineStore.getState().reset()
  setNeuronEngineClientForTesting({
    connection: 'ready',
    listModels: () => {},
    setModel: (model: string) => switched.push(model),
  } as unknown as NeuronEngineClient)
})

afterEach(() => {
  cleanup?.()
  cleanup = undefined
  setNeuronEngineClientForTesting(null)
})

/** Opens the picker over the models the engine store holds. */
async function openPicker() {
  const setup = await createTestRenderer({ width: 80, height: 14, kittyKeyboard: true })
  const root = createRoot(setup.renderer)
  let closes = 0
  cleanup = () => {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
  flushSync(() =>
    root.render(<ModelPicker onClose={() => (closes += 1)} width={80} maxVisibleRows={8} />),
  )
  const settle = async () => {
    await setup.renderOnce()
    await new Promise((resolve) => setTimeout(resolve, 20))
    await setup.renderOnce()
  }
  await settle()
  return Object.assign(setup, {
    settle,
    closes: () => closes,
    /** The focused row's line. */
    focused: () =>
      setup
        .captureCharFrame()
        .split('\n')
        .find((line) => line.includes('›'))
        ?.trim() ?? null,
  })
}

describe('ModelPicker focus', () => {
  test('opens on the current model, scrolled into view, after a switch away from Auto', async () => {
    useEngineStore.getState().setModels(catalog('model-9'))
    const picker = await openPicker()

    expect(picker.focused()).toContain('› Model 9  (current)')
    expect(picker.captureCharFrame()).toContain('10/14')
    expect(picker.captureCharFrame()).not.toContain('Auto')

    // Enter keeps the current model instead of switching back to Auto.
    picker.mockInput.pressEnter()
    await picker.settle()
    expect(switched).toEqual([])
    expect(picker.closes()).toBe(1)
  })

  test('moves to the current model when the fresh list replaces a stale one', async () => {
    // The list kept from the last open still says Auto; the reply to this
    // open's list_models, with as many rows, names the model switched to.
    useEngineStore.getState().setModels(catalog('auto'))
    const picker = await openPicker()
    expect(picker.focused()).toContain('› Auto')

    flushSync(() => useEngineStore.getState().setModels(catalog('model-9')))
    await picker.settle()
    expect(picker.focused()).toContain('› Model 9  (current)')
    expect(picker.captureCharFrame()).toContain('10/14')
  })

  test('a refresh with the same current model keeps the row the user moved to', async () => {
    useEngineStore.getState().setModels(catalog('model-2'))
    const picker = await openPicker()
    expect(picker.focused()).toContain('› Model 2  (current)')

    picker.mockInput.pressArrow('down')
    await picker.settle()
    expect(picker.focused()).toContain('› Model 3')

    flushSync(() => useEngineStore.getState().setModels(catalog('model-2')))
    await picker.settle()
    expect(picker.focused()).toContain('› Model 3')
  })
})
