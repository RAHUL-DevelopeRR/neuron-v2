import { afterEach, beforeAll, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import { MultilineInput } from '../multiline-input'
import { initializeThemeStore } from '../../hooks/use-theme'
import { useChatStore } from '../../state/chat-store'

let cleanup: (() => void) | undefined

beforeAll(() => {
  initializeThemeStore()
})

afterEach(() => {
  cleanup?.()
  cleanup = undefined
  useChatStore.getState().reset()
})

/** The composer as chat.tsx wires it: the text lives in the chat store. */
async function mountComposer(
  submitted: string[],
  focus: { focused?: boolean; takesKeysUnfocused?: boolean } = {},
) {
  const Harness = () => {
    const inputValue = useChatStore((s) => s.inputValue)
    const cursorPosition = useChatStore((s) => s.cursorPosition)
    const setInputValue = useChatStore((s) => s.setInputValue)
    return (
      <MultilineInput
        value={inputValue}
        cursorPosition={cursorPosition}
        onChange={setInputValue}
        onSubmit={() => submitted.push(useChatStore.getState().inputValue)}
        onPaste={() => {}}
        focused={focus.focused ?? true}
        takesKeysUnfocused={focus.takesKeysUnfocused}
      />
    )
  }
  const setup = await createTestRenderer({ width: 60, height: 8 })
  const root = createRoot(setup.renderer)
  flushSync(() => root.render(<Harness />))
  await setup.renderOnce()
  cleanup = () => {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
  return setup
}

describe('MultilineInput: keys that come before the next render', () => {
  test('three quick Backspaces delete three characters', async () => {
    // A held Backspace repeats this fast; a console delivers the burst at once.
    const setup = await mountComposer([])
    await setup.mockInput.typeText('zq9')
    await setup.renderOnce()
    expect(useChatStore.getState().inputValue).toBe('zq9')

    await setup.mockInput.pressKeys(['\b', '\b', '\b'])
    expect(useChatStore.getState().inputValue).toBe('')
  })

  test('Enter right after the last character submits all of it', async () => {
    const submitted: string[] = []
    const setup = await mountComposer(submitted)
    await setup.mockInput.pressKeys(['f', 'i', 'x', ' ', 'i', 't', '\r'])
    expect(submitted).toEqual(['fix it'])
  })

  test('typing right after quick Backspaces builds on the shortened text', async () => {
    const setup = await mountComposer([])
    await setup.mockInput.typeText('zq9')
    await setup.renderOnce()
    await setup.mockInput.pressKeys(['\b', '\b', '\b', '/', 'e', 'x', 'i', 't'])
    expect(useChatStore.getState().inputValue).toBe('/exit')
  })
})

describe('MultilineInput: keys while the terminal reports no focus', () => {
  test('the chat input takes them: only a focused terminal gets keys', async () => {
    const submitted: string[] = []
    const setup = await mountComposer(submitted, { focused: false, takesKeysUnfocused: true })
    await setup.mockInput.typeText('hi')
    await setup.mockInput.pressKeys(['\r'])
    expect(submitted).toEqual(['hi'])
  })

  test('an input that is simply not the active one still ignores them', async () => {
    const setup = await mountComposer([], { focused: false })
    await setup.mockInput.typeText('hi')
    expect(useChatStore.getState().inputValue).toBe('')
  })
})
