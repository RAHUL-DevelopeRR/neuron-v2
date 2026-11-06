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
import { DiscardDialog } from '../src/components/ChatHeader'
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
    let release = () => {}
    const restarting = new Promise<void>((resolve) => (release = resolve))
    const record = platform.restart
    platform.restart = async (resume, options) => {
      await record(resume, options)
      await restarting
    }
    type('first')
    type('second')
    effort('high')
    expect(platform.restarts).toEqual([])
    await finishTurn(engine)
    await waitFor(() => expect(platform.restarts).toEqual(['session-1']))
    // The queued prompt does not run into the old engine while the restart is under way.
    await pause(80)
    expect(submits(engine)).toEqual(['first'])
    await act(async () => release())
    await waitFor(() => expect(submits(engine)).toEqual(['first', 'second']))
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
    fireEvent.click(await screen.findByRole('button', { name: /^Allow once/ }))
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

const MISSING = "[engine:missing] The Neuron engine was not found. Reinstall Neuron, or set NEURON_BIN to the engine's path."
const COPY_NOTICE = 'This chat works in its own copy. Discard the copy to close the chat, or keep it.'

describe('the discard dialog', () => {
  const status = { branch: 'neuron/x', path: '/wt/x', dirty: 1, unmerged: 2 }
  const mount = (discard: (options: { force: boolean; deleteUnmerged: boolean }) => Promise<void>, loadStatus = async () => status) =>
    render(
      <DiscardDialog
        info={{ id: 'c1', project: { name: 'app', path: '/work/app' }, cwd: '/wt/x', branch: 'neuron/x', worktree: { path: '/wt/x', branch: 'neuron/x', base: 'main' } }}
        loadStatus={loadStatus}
        discard={discard}
        onClose={() => {}}
      />,
    )

  test('a double click on Continue does not answer the second question', async () => {
    const calls: Array<{ force: boolean; deleteUnmerged: boolean }> = []
    mount(async (options) => void calls.push(options))
    const dialog = await screen.findByRole('alertdialog')
    const first = await within(dialog).findByRole('button', { name: 'Continue' })
    await waitFor(() => expect((first as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(first)
    fireEvent.click(within(dialog).getByRole('button', { name: /Delete them and discard|Continue/ }))
    fireEvent.click(within(dialog).getByRole('button', { name: /Delete them and discard|Continue/ }))
    expect(within(dialog).getByRole('heading', { name: 'Delete its saved changes too?' })).toBeTruthy()
    expect(calls).toEqual([])
    // Focus is on the safe answer, and the confirm button is a new element.
    expect(document.activeElement).toBe(within(dialog).getByRole('button', { name: 'Cancel' }))
    expect(within(dialog).getByRole('button', { name: 'Delete them and discard' })).not.toBe(first)
    // After a moment the person can answer.
    await pause(450)
    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete them and discard' }))
    await waitFor(() => expect(calls).toEqual([{ force: true, deleteUnmerged: true }]))
  })

  test('a changed copy (dirty or unmerged answer) reloads the status and asks from the start', async () => {
    let loads = 0
    mount(
      async () => {
        throw new ChatError('This copy has unsaved changes.', 'dirty', 3)
      },
      async () => ({ ...status, dirty: ++loads === 1 ? 0 : 3, unmerged: 0 }),
    )
    const dialog = await screen.findByRole('alertdialog')
    await within(dialog).findByText('Nothing in it would be lost.')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Discard the copy' }))
    await within(dialog).findByText(/3 files have changes/)
    expect(loads).toBe(2)
    expect(within(dialog).getByRole('heading', { name: 'Discard this copy?' })).toBeTruthy()
  })
})

describe('keys while a dialog is open', () => {
  async function inACopyWithDialog() {
    const started = await start({ multiChat: true })
    const copy = await openAnother(started.platform, true)
    fireEvent.click(await screen.findByRole('button', { name: 'Discard' }))
    await screen.findByRole('alertdialog')
    return { ...started, copy, first: started.chats.ids()[0] }
  }

  test('the dialog stays about the chat it was opened for', async () => {
    const { chats, copy, first } = await inACopyWithDialog()
    fireEvent.click(rowOf(first))
    await waitFor(() => expect(chats.activated.at(-1)).toBe(first))
    const dialog = screen.getByRole('alertdialog')
    fireEvent.click(await within(dialog).findByRole('button', { name: 'Discard the copy' }))
    await waitFor(() => expect(chats.discards.map((discard) => discard.id)).toEqual([copy]))
  })

  test('global shortcuts and the focus jump do nothing', async () => {
    const { platform, chats, copy, first } = await inACopyWithDialog()
    const before = chats.ids().length
    const active = chats.activated.length
    for (const init of [
      { key: 'n', ctrlKey: true },
      { key: 'N', ctrlKey: true, shiftKey: true },
      { key: 'j', ctrlKey: true },
      { key: 'ArrowUp', ctrlKey: true, altKey: true },
      { key: 'ArrowDown', ctrlKey: true, altKey: true },
      { key: '/', ctrlKey: true },
      { key: 'k', ctrlKey: true },
    ]) {
      fireEvent.keyDown(window, init)
    }
    await pause()
    expect(chats.ids().length).toBe(before)
    expect(chats.activated.length).toBe(active)
    expect(screen.queryByRole('dialog', { name: 'Keyboard shortcuts' })).toBeNull()
    expect(screen.queryByRole('dialog', { name: 'Command palette' })).toBeNull()
    // A chat in the background asks for the user; coming back to the window stays where it is.
    await emit(chats.engines.get(first)!, permission(3))
    await waitFor(() => expect(platform.notifications.length).toBe(1))
    fireEvent(window, new Event('focus'))
    await pause()
    expect(chats.activated.at(-1)).toBe(copy)
  })
})

describe('global keys', () => {
  test('a held key is one press', async () => {
    const { platform, chats } = await start({ multiChat: true })
    for (let i = 0; i < 6; i++) fireEvent.keyDown(window, { key: 'n', ctrlKey: true, repeat: true })
    await pause()
    expect(chats.ids()).toHaveLength(1)
    await openAnother(platform)
    expect(chats.ids()).toHaveLength(2)
  })

  test('a second press while a chat is starting opens nothing more', async () => {
    const { chats } = await start({ multiChat: true })
    fireEvent.keyDown(window, { key: 'n', ctrlKey: true })
    fireEvent.keyDown(window, { key: 'n', ctrlKey: true })
    fireEvent.keyDown(window, { key: 'N', ctrlKey: true, shiftKey: true })
    await waitFor(() => expect(chats.ids()).toHaveLength(2))
    await pause(80)
    expect(chats.ids()).toHaveLength(2)
  })
})

describe('closing chats', () => {
  async function closeFromPalette() {
    fireEvent.keyDown(window, { key: 'k', ctrlKey: true })
    const palette = await screen.findByRole('dialog', { name: 'Command palette' })
    const input = within(palette).getByRole('combobox')
    fireEvent.change(input, { target: { value: 'close this chat' } })
    fireEvent.keyDown(input, { key: 'Enter' })
  }

  test('the last chat is replaced; when that fails the splash says why and Retry opens one', async () => {
    const { chats } = await start({ multiChat: true })
    chats.failNext = 'cap_reached'
    await closeFromPalette()
    expect(await screen.findByText('4 chats are working. Stop one to open another.')).toBeTruthy()
    expect(screen.queryByText(/Starting the Neuron engine/)).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
    expect(await screen.findByLabelText('Message Neuron')).toBeTruthy()
    expect(chats.ids()).toHaveLength(1)
  })

  test('a missing engine is the problem card, with a Retry', async () => {
    const { chats } = await start({ multiChat: true })
    chats.failNext = MISSING
    await closeFromPalette()
    expect(await screen.findByRole('alert', { name: /engine/i })).toBeTruthy()
    chats.failNext = MISSING
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
    await screen.findByRole('alert', { name: /engine/i })
    expect(screen.queryByText(/Starting the Neuron engine/)).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
    expect(await screen.findByLabelText('Message Neuron')).toBeTruthy()
  })

  test('a first chat that failed to start is tried again by Retry, not remembered as failed', async () => {
    const platform = fakePlatform(new FakeEngine(), { multiChat: true })
    platform.fakeChats!.failNext = MISSING
    render(<App platform={platform} />)
    await screen.findByRole('alert', { name: /engine/i })
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
    expect(await screen.findByLabelText('Message Neuron')).toBeTruthy()
    expect(platform.fakeChats!.opened).toHaveLength(2)
  })

  test('a chat with a copy of its own: closing it opens the discard dialog for that chat, from the palette', async () => {
    const { platform, chats } = await start({ multiChat: true })
    const copy = await openAnother(platform, true)
    chats.closeNeedsDiscard.add(copy)
    await closeFromPalette()
    const dialog = await screen.findByRole('alertdialog')
    expect(within(dialog).getByText(COPY_NOTICE)).toBeTruthy()
    fireEvent.click(await within(dialog).findByRole('button', { name: 'Discard the copy' }))
    await waitFor(() => expect(chats.discards.map((discard) => discard.id)).toEqual([copy]))
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull())
    expect(chats.ids()).not.toContain(copy)
  })

  test('the sidebar row has a close button named for its row; a chat with a copy asks first', async () => {
    const { platform, chats } = await start({ multiChat: true })
    const copy = await openAnother(platform, true)
    const first = chats.ids()[0]
    chats.closeNeedsDiscard.add(copy)
    const closeOf = (id: string) => within(rowOf(id).closest('li')!).getByRole('button', { name: /^Close chat: / })
    fireEvent.click(closeOf(copy))
    const dialog = await screen.findByRole('alertdialog')
    expect(within(dialog).getByText(COPY_NOTICE)).toBeTruthy()
    fireEvent.keyDown(dialog, { key: 'Escape' })
    expect(chats.ids()).toContain(copy)
    // An ordinary chat just closes.
    fireEvent.click(closeOf(first))
    await waitFor(() => expect(chats.closed).toEqual([first]))
    await waitFor(() => expect(rowOf(first)).toBeNull())
  })
})

describe('turns that end', () => {
  test('switching from a working chat to an idle one is not a turn ending', async () => {
    const engine = new FakeEngine()
    const platform = fakePlatform(engine, { multiChat: true })
    platform.saved.set('seen', { seen: {}, failed: { 'session-1': true } })
    render(<App platform={platform} />)
    await screen.findByLabelText('Message Neuron')
    await within(sidebar()).findByRole('region', { name: /, 1$/ })
    type('work')
    await openAnother(platform)
    await pause(100)
    expect((platform.saved.get('seen') as { failed: Record<string, boolean> }).failed['session-1']).toBe(true)
  })
})

describe('the host refusals in the UI', () => {
  test('a discard refused because the chat is still starting says so and stays open', async () => {
    render(
      <DiscardDialog
        info={{ id: 'c1', project: { name: 'app', path: '/work/app' }, cwd: '/wt/x', branch: 'neuron/x', worktree: { path: '/wt/x', branch: 'neuron/x', base: 'main' } }}
        loadStatus={async () => ({ branch: 'neuron/x', path: '/wt/x', dirty: 0, unmerged: 0 })}
        discard={async () => {
          throw new ChatError('This chat is still starting. Try again in a moment.', 'starting')
        }}
        onClose={() => {}}
      />,
    )
    const dialog = await screen.findByRole('alertdialog')
    fireEvent.click(await within(dialog).findByRole('button', { name: 'Discard the copy' }))
    expect((await within(dialog).findByRole('alert')).textContent).toBe('This chat is still starting. Try again in a moment.')
    expect(within(dialog).getByRole('heading', { name: 'Discard this copy?' })).toBeTruthy()
  })

  test('a message the engine queue refused is an error line and comes back to the composer', async () => {
    const { engine } = await start()
    type('please refactor')
    await emit(engine, { type: 'error', message: 'The Neuron engine is not keeping up. Try again in a moment.' }, { type: 'turn_finished' })
    expect(screen.getByText('The Neuron engine is not keeping up. Try again in a moment.')).toBeTruthy()
    await waitFor(() => expect(box().value).toBe('please refactor'))
  })
})
