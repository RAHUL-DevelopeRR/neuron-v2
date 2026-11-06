/**
 * Every lane in the multi-chat model (L4): review and git (L2) act in the chat
 * in view, the composer's effort (L3) restarts the chat in view, engine
 * problems and Try again (L5) restart the chat in view, and the live
 * transcript's todo strip (L1) sits above the composer.
 */
import { afterEach, describe, expect, test } from 'bun:test'
import { act, cleanup, configure, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

import type { UiEvent } from '@neuron/client'

import { App } from '../src/App'
import { FakeEngine, fakePlatform, type FakePlatform, type FakePlatformOptions } from './fake-platform'

// A busy machine (parallel builds) is slow to render several chats.
configure({ asyncUtilTimeout: 5000 })

afterEach(() => {
  cleanup()
  try {
    localStorage.clear()
  } catch {
    // No storage: nothing to clear.
  }
})

const sidebar = () => screen.getByRole('navigation', { name: 'Neuron' })

async function start(options: FakePlatformOptions = {}) {
  const engine = new FakeEngine()
  const platform = fakePlatform(engine, { multiChat: true, ...options })
  render(<App platform={platform} />)
  await screen.findByLabelText('Message Neuron')
  await within(sidebar()).findByRole('region', { name: 'Idle, 1' })
  return { engine, platform, chats: platform.fakeChats! }
}

/** Opens another chat (Ctrl+N, or Ctrl+Shift+N for one in its own copy) and waits until it is in view. */
async function openAnother(platform: FakePlatform, copy = false) {
  const chats = platform.fakeChats!
  const before = chats.ids().length
  fireEvent.keyDown(window, { key: copy ? 'N' : 'n', ctrlKey: true, shiftKey: copy })
  await waitFor(() => expect(chats.ids().length).toBe(before + 1))
  const id = chats.ids().at(-1)!
  await waitFor(() => expect(chats.activated.at(-1)).toBe(id))
  await screen.findByLabelText('Message Neuron')
  return id
}

const emitTo = (platform: FakePlatform, id: string, ...events: UiEvent[]) =>
  act(() => {
    for (const event of events) platform.fakeChats!.engines.get(id)!.emit(event)
  })

/** The row of a changed file in the Changes pane. */
const changeRow = (path: string) => within(screen.getByRole('complementary', { name: 'Details' })).getByText(path).closest('li') as HTMLElement

describe('L2, review and git, in the chat in view', () => {
  test("the chat bar opens the chat's own copy in the editor", async () => {
    const { platform } = await start()
    const copy = await openAnother(platform, true)
    const bar = await screen.findByRole('group', { name: 'This chat works in its own copy of the project' })
    fireEvent.click(within(bar).getByRole('button', { name: 'Open in editor' }))
    await waitFor(() => expect(platform.editorOpens).toEqual(['.']))
    expect(platform.reviewChats).toEqual([{ call: 'openInEditor', path: '.', chat: copy }])

    // A refusal is told, not thrown.
    platform.openInEditor = async () => {
      throw new Error('That file is not there.')
    }
    fireEvent.click(within(bar).getByRole('button', { name: 'Open in editor' }))
    expect(await screen.findByText('That file is not there.')).toBeTruthy()
  })

  test('a chat in the project folder has no chat bar', async () => {
    await start()
    expect(screen.queryByRole('group', { name: 'This chat works in its own copy of the project' })).toBeNull()
  })

  test('reverting and opening a changed file act in the chat in view', async () => {
    const { platform } = await start()
    const second = await openAnother(platform)
    fireEvent.click(within(changeRow('src/main.rs')).getByRole('button', { name: 'Open in editor' }))
    fireEvent.click(within(changeRow('src/main.rs')).getByRole('button', { name: 'Revert' }))
    fireEvent.click(within(screen.getByRole('dialog', { name: /Revert src\/main\.rs/ })).getByRole('button', { name: 'Revert file' }))
    await waitFor(() => expect(platform.reverted).toEqual(['src/main.rs']))
    expect(platform.reviewChats).toEqual([
      { call: 'openInEditor', path: 'src/main.rs', chat: second },
      { call: 'gitRevert', path: 'src/main.rs', chat: second },
    ])
  })
})

describe('L3, the composer, with several chats', () => {
  test('a new reasoning effort is a launch option and restarts the chat in view', async () => {
    const { platform, chats } = await start()
    const second = await openAnother(platform)
    fireEvent.change(screen.getByLabelText('Reasoning effort'), { target: { value: 'high' } })
    expect(platform.launches.at(-1)?.reasoningEffort).toBe('high')
    // Idle and nothing said yet: a fresh engine for this chat only.
    await waitFor(() => expect(chats.resumed).toEqual([{ id: second, options: { resume: null, trustOnce: false } }]))
    expect(platform.restarts).toEqual([null])
  })
})

describe('L5, engine problems, with several chats', () => {
  test("the first chat's coded start error is the engine problem card", async () => {
    const engine = new FakeEngine()
    const platform = fakePlatform(engine, { multiChat: true })
    platform.fakeChats!.failNext = "[engine:missing] The Neuron engine was not found. Reinstall Neuron, or set NEURON_BIN to the engine's path."
    render(<App platform={platform} />)
    const card = await screen.findByRole('alert', { name: "Neuron's engine is missing" })
    expect(card.textContent).toContain('NEURON_BIN')
  })

  test('Try again on a crashed engine restarts the chat in view', async () => {
    const { platform, chats } = await start()
    const second = await openAnother(platform)
    await act(async () => {
      for (const listener of chats.engines.get(second)!.closeListeners)
        listener({ code: 3, error: '[engine:crashed] The Neuron engine stopped unexpectedly (exit code 3). Last message: out of memory' })
    })
    const card = await screen.findByRole('alert', { name: 'The engine stopped unexpectedly' })
    fireEvent.click(within(card).getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(chats.resumed).toEqual([{ id: second, options: { resume: 'session-1', trustOnce: false } }]))
    expect(platform.restarts).toEqual(['session-1'])
  })
})

describe('L1, the live transcript, with several chats', () => {
  test('the todo strip sits right above the composer and follows the chat', async () => {
    const { platform, chats } = await start()
    const first = chats.ids()[0]
    await emitTo(platform, first, { type: 'todos', items: [{ content: 'Build', status: 'in_progress', active_form: 'Building' }] })
    const strip = await screen.findByRole('region', { name: 'Tasks' })
    expect(strip.nextElementSibling?.className).toBe('composer')
    // The next chat has no steps of its own.
    await openAnother(platform)
    await waitFor(() => expect(screen.queryByRole('region', { name: 'Tasks' })).toBeNull())
  })
})
