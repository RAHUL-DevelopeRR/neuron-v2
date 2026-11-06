/**
 * The review fixes of the app shell and the multi-chat UI: composer wiring, the
 * mode shortcut, the effort restart, global keys while a dialog is open, the
 * discard dialog, closing chats (including ones with a copy of their own) and
 * the command palette.
 */
import { afterEach, describe, expect, test } from 'bun:test'
import { act, cleanup, configure, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import axe from 'axe-core'

import type { ClientMessage, UiEvent } from '@neuron/client'

import { App } from '../src/App'
import { ChatError } from '../src/platform'
import { SHORTCUTS, keysOf, shortcutFor } from '../src/shortcuts'
import { FakeEngine, fakePlatform, type FakePlatform, type FakePlatformOptions } from './fake-platform'

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
const box = () => screen.getByLabelText('Message Neuron') as HTMLTextAreaElement
const rowOf = (id: string) => sidebar().querySelector<HTMLElement>(`[data-chat-id="${id}"]`)!

async function start(options: FakePlatformOptions = {}, engine = new FakeEngine()) {
  const platform = fakePlatform(engine, options)
  render(<App platform={platform} />)
  await screen.findByLabelText('Message Neuron')
  if (options.multiChat) await within(sidebar()).findByRole('region', { name: /^(Idle|Working|Needs you), 1$/ })
  return { engine, platform, chats: platform.fakeChats as FakePlatform['fakeChats'] & object }
}

const emit = (engine: FakeEngine, ...events: UiEvent[]) =>
  act(() => {
    for (const event of events) engine.emit(event)
  })

const type = (text: string) => {
  fireEvent.change(box(), { target: { value: text, selectionStart: text.length } })
  fireEvent.keyDown(box(), { key: 'Enter' })
}

const commands = (engine: FakeEngine, kind: string) =>
  engine.commands().filter((message: ClientMessage) => message.kind === 'command' && message.type === kind) as Array<Record<string, unknown>>
const submits = (engine: FakeEngine) => commands(engine, 'submit').map((message) => message.text)
const finishTurn = (engine: FakeEngine) => emit(engine, { type: 'turn_finished' }, engine.status())
const pause = (ms = 50) => new Promise((resolve) => setTimeout(resolve, ms))

const permission = (id: number): UiEvent => ({
  type: 'permission_requested',
  id,
  request: { tool: 'bash', current_mode: 'workspace-write', required_mode: 'danger-full-access', reason: 'it deletes files', input: '{"command":"rm -rf target"}', session_prefixes: ['rm'] },
})

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

describe('composer wiring', () => {
  test('a failed turn holds the queue', async () => {
    const { engine } = await start()
    type('first')
    type('second')
    expect(screen.getByRole('list', { name: 'Queued messages' })).toBeTruthy()
    await emit(engine, { type: 'error', message: 'The model could not be reached.' }, { type: 'turn_finished' })
    expect(await screen.findByText(/Queue paused/)).toBeTruthy()
    await pause()
    expect(submits(engine)).toEqual(['first'])
  })

  test('a new session clears the queue', async () => {
    const { engine } = await start()
    type('first')
    type('second')
    expect(screen.getByRole('list', { name: 'Queued messages' })).toBeTruthy()
    await emit(engine, engine.status({ session_id: 'session-2' }))
    await waitFor(() => expect(screen.queryByRole('list', { name: 'Queued messages' })).toBeNull())
    await finishTurn(engine)
    await pause()
    expect(submits(engine)).toEqual(['first'])
  })
})

describe('the mode shortcut', () => {
  const cycle = (init: KeyboardEventInit = {}) => fireEvent.keyDown(window, { key: 'M', ctrlKey: true, shiftKey: true, ...init })
  const slashes = (engine: FakeEngine) => commands(engine, 'slash').map((message) => `${message.command} ${message.args}`)

  test('is in the table that feeds the shortcuts sheet', () => {
    const entry = SHORTCUTS.find((shortcut) => shortcut.id === 'cycleMode')!
    expect(entry).toBeTruthy()
    expect(keysOf(entry)).toBe('Ctrl+Shift+M')
    expect(keysOf(entry, true)).toBe('⌘⇧M')
    expect(shortcutFor({ key: 'M', ctrlKey: false, metaKey: true, shiftKey: true, altKey: false })).toBe('cycleMode')
  })

  test('Ctrl+Shift+M cycles from anywhere, and the sheet lists it', async () => {
    const { engine } = await start()
    cycle()
    expect(slashes(engine)).toEqual(['plan on'])
    fireEvent.keyDown(window, { key: '/', ctrlKey: true })
    const sheet = await screen.findByRole('dialog', { name: 'Keyboard shortcuts' })
    expect(within(sheet).getByText('Cycle the permission mode')).toBeTruthy()
    expect(within(sheet).getByText('Ctrl+Shift+M')).toBeTruthy()
  })

  test('the Cmd key works too', async () => {
    const { engine } = await start()
    cycle({ ctrlKey: false, metaKey: true })
    expect(slashes(engine)).toEqual(['plan on'])
  })

  test('in the composer it cycles once, not twice', async () => {
    const { engine } = await start()
    fireEvent.keyDown(box(), { key: 'M', ctrlKey: true, shiftKey: true })
    expect(slashes(engine)).toEqual(['plan on'])
  })

  test('two quick presses advance twice, before the engine has answered', async () => {
    const { engine } = await start()
    cycle()
    cycle()
    expect(slashes(engine)).toEqual(['plan on', 'plan off', 'permissions read-only'])
    // The engine answers: its status is the base again.
    await emit(engine, engine.status({ permission_mode: 'read-only', plan_mode: false }))
    cycle()
    expect(slashes(engine).at(-1)).toBe('permissions workspace-write')
  })

  test('never reaches full access', async () => {
    const { engine } = await start({}, new FakeEngine({ permission_mode: 'danger-full-access' }))
    cycle()
    expect(slashes(engine)).toEqual(['permissions read-only'])
  })
})

describe('the MCP switch', () => {
  test('a throw from the controller (engine down) is shown inline by the panel', async () => {
    const features = ['capabilities', 'sessions', 'extensions', 'mcp_toggle']
    const engine = new FakeEngine({}, features)
    await start({}, engine)
    fireEvent.click(screen.getByRole('button', { name: 'Extensions' }))
    await emit(engine, {
      type: 'extensions',
      mcp_servers: [{ name: 'docs', scope: 'user', transport: 'stdio', status: 'ready', tools: [], error: null }],
      plugins: [],
      skills: [],
      agents: [],
    } as unknown as UiEvent)
    engine.send = () => {
      throw new Error('The Neuron engine is not running.')
    }
    fireEvent.click(screen.getByRole('switch', { name: 'docs' }))
    expect((await screen.findByRole('alert')).textContent).toContain('The Neuron engine is not running.')
  })
})

describe('the effort restart', () => {
  const effort = (value: string) => fireEvent.change(screen.getByLabelText('Reasoning effort'), { target: { value } })

  test('the queue holds while a restart is pending, and the restart comes first', async () => {
    const { engine, platform } = await start()
    type('first')
    type('second')
    effort('high')
    expect(platform.restarts).toEqual([])
    await finishTurn(engine)
    await waitFor(() => expect(platform.restarts).toEqual(['session-1']))
    // The queued prompt did not run into the old engine in between.
    expect(submits(engine)).toEqual(['first'])
  })

  test('with an open question it does not restart; it waits for the answer', async () => {
    const { engine, platform } = await start()
    type('go')
    effort('low')
    await emit(engine, { type: 'question', id: 4, question: 'Which one?', options: [] }, { type: 'turn_finished' })
    await pause(80)
    expect(platform.restarts).toEqual([])
    type('the first')
    await emit(engine, { type: 'turn_finished' })
    await waitFor(() => expect(platform.restarts).toEqual(['session-1']))
  })

  test('with an approval pending it does not restart', async () => {
    const { engine, platform } = await start()
    type('go')
    effort('low')
    await emit(engine, permission(7))
    await pause(80)
    expect(platform.restarts).toEqual([])
    fireEvent.click(await screen.findByRole('button', { name: /^Allow/ }))
    await finishTurn(engine)
    await waitFor(() => expect(platform.restarts).toEqual(['session-1']))
  })

  test('restarts once, however often the screen changes while it runs', async () => {
    const { engine, platform } = await start()
    effort('medium')
    await waitFor(() => expect(platform.restarts.length).toBe(1))
    await emit(engine, engine.status(), engine.status())
    await pause()
    expect(platform.restarts.length).toBe(1)
  })

  test('the browser build has no effort picker, the desktop one has', async () => {
    await start({ kind: 'host' })
    expect(screen.queryByLabelText('Reasoning effort')).toBeNull()
    cleanup()
    await start({ kind: 'tauri' })
    expect(screen.getByLabelText('Reasoning effort')).toBeTruthy()
  })
})

describe('the command palette', () => {
  const openPalette = async () => {
    fireEvent.keyDown(window, { key: 'k', ctrlKey: true })
    return await screen.findByRole('dialog', { name: 'Command palette' })
  }

  test('the empty state is not inside the listbox, and the input announces a list', async () => {
    await start()
    const palette = await openPalette()
    const input = within(palette).getByRole('combobox')
    expect(input.getAttribute('aria-autocomplete')).toBe('list')
    fireEvent.change(input, { target: { value: 'zzzzqqqq' } })
    const message = within(palette).getByText('Nothing matches.')
    expect(message.closest('[role="listbox"]')).toBeNull()
    expect(within(palette).queryAllByRole('option')).toHaveLength(0)
    const result = await axe.run(palette, { rules: { 'color-contrast': { enabled: false } } })
    expect(result.violations.filter((violation) => violation.impact === 'serious' || violation.impact === 'critical').map((violation) => violation.id)).toEqual([])
  })

  test('the arrows scroll the choice into view', async () => {
    await start()
    const palette = await openPalette()
    const scrolled: string[] = []
    const original = Element.prototype.scrollIntoView
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled.push(this.id)
    }
    try {
      fireEvent.keyDown(within(palette).getByRole('combobox'), { key: 'ArrowDown' })
      const chosen = within(palette).getAllByRole('option').find((option) => option.getAttribute('aria-selected') === 'true')!
      await waitFor(() => expect(scrolled).toContain(chosen.id))
    } finally {
      Element.prototype.scrollIntoView = original
    }
  })

  test('Enter while an IME composes a word runs nothing', async () => {
    const { platform } = await start()
    const palette = await openPalette()
    const input = within(palette).getByRole('combobox')
    fireEvent.change(input, { target: { value: 'settings' } })
    fireEvent.keyDown(input, { key: 'Enter', isComposing: true })
    expect(screen.queryByRole('dialog', { name: 'Command palette' })).not.toBeNull()
    expect(platform.opened).toEqual([])
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(screen.queryByRole('dialog', { name: 'Command palette' })).toBeNull()
  })

  test('Ctrl+K opens and closes it, also while it is the dialog that is open', async () => {
    await start()
    await openPalette()
    fireEvent.keyDown(window, { key: 'k', ctrlKey: true })
    expect(screen.queryByRole('dialog', { name: 'Command palette' })).toBeNull()
  })
})
