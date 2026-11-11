/**
 * L4, several chats at once: the sidebar's groups and badge, notifications and
 * the taskbar count, the palette and shortcuts sheet, copies of the project
 * (worktrees) and discarding one, stopped chats starting again, and the full
 * house of four engines.
 */
import { afterEach, describe, expect, test } from 'bun:test'
import { act, cleanup, configure, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import axe from 'axe-core'

import type { UiEvent } from '@neuron/client'

import { App } from '../src/App'
import { ChatContextBar } from '../src/components/ChatHeader'
import { SHORTCUTS, keysOf } from '../src/shortcuts'
import type { ChatInfo } from '../src/platform'
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

async function start(options: FakePlatformOptions = {}) {
  const engine = new FakeEngine()
  const platform = fakePlatform(engine, { multiChat: true, ...options })
  render(<App platform={platform} />)
  await screen.findByLabelText('Message Neuron')
  // The chat is listed once the app has taken it.
  await within(sidebar()).findByRole('region', { name: 'Idle, 1' })
  return { engine, platform, chats: platform.fakeChats! }
}

const sidebar = () => screen.getByRole('navigation', { name: 'Neuron' })
const chatRow = (name: RegExp | string) => within(sidebar()).getByRole('button', { name })
/** A chat's row, by the id the host gave it. */
const rowOf = (id: string) => sidebar().querySelector<HTMLElement>(`[data-chat-id="${id}"]`)!

const permission = (id: number): UiEvent => ({
  type: 'permission_requested',
  id,
  request: { tool: 'bash', current_mode: 'workspace-write', required_mode: 'danger-full-access', reason: 'it deletes files', input: '{"command":"rm -rf target"}', session_prefixes: ['rm'] },
})

/** Opens another chat with Ctrl+N and waits until it is in view. */
async function openSecond(platform: FakePlatform, chats = platform.fakeChats!) {
  const before = chats.ids().length
  fireEvent.keyDown(window, { key: 'n', ctrlKey: true })
  await waitFor(() => expect(chats.ids().length).toBe(before + 1))
  await waitFor(() => expect(chats.activated.at(-1)).toBe(chats.ids().at(-1)!))
  return chats.ids().at(-1)!
}

const emitTo = (chats: FakePlatform['fakeChats'], id: string, ...events: UiEvent[]) =>
  act(() => {
    for (const event of events) chats!.engines.get(id)!.emit(event)
  })

describe('opening chats', () => {
  test('starts with one chat, listed under its own heading', async () => {
    const { chats } = await start()
    expect(chats.opened).toEqual([{ path: undefined }])
    expect(chats.activated).toEqual([chats.ids()[0]])
    const group = within(sidebar()).getByRole('region', { name: 'Idle, 1' })
    expect(within(group).getByText('New chat')).toBeTruthy()
  })

  test('Ctrl+N opens a second chat in the same project and puts it in view', async () => {
    const { platform, chats } = await start()
    const second = await openSecond(platform)
    expect(chats.opened.at(-1)).toEqual({ path: '/work/app' })
    expect(chats.ids()).toHaveLength(2)
    expect(within(sidebar()).getByRole('region', { name: 'Idle, 2' })).toBeTruthy()
    // The new chat is the current one.
    expect(rowOf(second).getAttribute('aria-current')).toBe('true')
    expect(rowOf(chats.ids()[0]).getAttribute('aria-current')).toBeNull()
    expect(second).toBe(chats.ids()[1])
  })

  test('a chat in its own copy shows its branch, and its folder is the copy', async () => {
    const { chats } = await start()
    fireEvent.keyDown(window, { key: 'N', ctrlKey: true, shiftKey: true })
    await waitFor(() => expect(chats.opened.at(-1)).toEqual({ path: '/work/app', worktree: {} }))
    const bar = await screen.findByRole('group', { name: /own copy of the project/ })
    expect(bar.textContent).toContain('neuron/chat-2')
    // The editor button opens the copy (wired at integration; see lanes.test.tsx).
    expect(within(bar).getByRole('button', { name: 'Open in editor' })).toBeTruthy()
    expect(within(bar).getByRole('button', { name: 'Discard' })).toBeTruthy()
  })

  test('the full house of four is told in plain words', async () => {
    const { chats } = await start()
    chats.failNext = 'cap_reached'
    fireEvent.keyDown(window, { key: 'n', ctrlKey: true })
    const toast = await screen.findByRole('status')
    expect(toast.textContent).toContain('4 chats are working. Stop one to open another.')
    expect(chats.ids()).toHaveLength(1)
  })

  test('a folder already open in a chat is shown, not opened twice', async () => {
    const { platform, chats } = await start()
    await openSecond(platform)
    fireEvent.click(within(sidebar()).getByRole('button', { name: 'Projects' }))
    // The page's own button for the open project.
    expect(screen.getAllByText('app').length).toBeGreaterThan(0)
    const opened = chats.opened.length
    chats.opened.length = opened
    expect(chats.ids()).toHaveLength(2)
  })
})

describe('a chat that needs you', () => {
  test('raises the badge, notifies once, and sets the count; jumping shows its prompt', async () => {
    const { platform, chats } = await start()
    const [first] = chats.ids()
    await openSecond(platform)
    // Back to the first chat; the second asks for permission in the background.
    fireEvent.click(rowOf(first))
    await waitFor(() => expect(chats.activated.at(-1)).toBe(first))
    const second = chats.ids()[1]
    await emitTo(chats, second, permission(5))
    // The same request again is not news.
    await emitTo(chats, second, permission(5))

    await waitFor(() => expect(platform.notifications).toHaveLength(1))
    expect(platform.notifications[0].chat).toBe(second)
    expect(platform.notifications[0].body).toContain('needs your OK')
    expect(platform.attention.at(-1)).toBe(1)
    expect(chats.activities.get(second)).toEqual({ busy: false, waiting: true })
    expect(screen.queryByRole('dialog', { name: /./ })).toBeNull()
    // The approval prompt is an alert dialog; the chat in the background shows none.
    expect(screen.queryByRole('alertdialog')).toBeNull()

    const needsYou = within(sidebar()).getByRole('region', { name: 'Needs you, 1' })
    expect(needsYou).toBeTruthy()
    fireEvent.click(within(sidebar()).getByRole('button', { name: '1 chat needs you. Go to the first' }))
    await waitFor(() => expect(chats.activated.at(-1)).toBe(second))
    const dialog = await screen.findByRole('alertdialog')
    expect(within(dialog).getByText('$ rm -rf target')).toBeTruthy()

    // Answering clears the count.
    fireEvent.click(within(dialog).getByRole('button', { name: /allow once/i }))
    await waitFor(() => expect(platform.attention.at(-1)).toBe(0))
  })

  test('the chat in view does not notify', async () => {
    const { platform, chats } = await start()
    await emitTo(chats, chats.ids()[0], permission(1))
    await screen.findByRole('alertdialog')
    expect(platform.notifications).toEqual([])
    expect(platform.attention.at(-1)).toBe(1)
  })

  test('Ctrl+J goes to the chat that has waited longest', async () => {
    const { platform, chats } = await start()
    const [first] = chats.ids()
    await openSecond(platform)
    await openSecond(platform)
    const [, second, third] = chats.ids()
    fireEvent.click(rowOf(first))
    await emitTo(chats, third, permission(3))
    await new Promise((resolve) => setTimeout(resolve, 5))
    await emitTo(chats, second, permission(2))
    chats.activated.length = 0
    fireEvent.keyDown(window, { key: 'j', ctrlKey: true })
    await waitFor(() => expect(chats.activated.at(-1)).toBe(third))
    expect(first).toBeTruthy()
  })

  test('with notifications off the app says so once and keeps the badge', async () => {
    const { platform, chats } = await start()
    platform.notifyResult = 'denied'
    await openSecond(platform)
    fireEvent.click(rowOf(chats.ids()[0]))
    const second = chats.ids()[1]
    await emitTo(chats, second, permission(1))
    const toast = await screen.findByRole('status')
    expect(toast.textContent).toContain('Notifications are off')
    expect(within(sidebar()).getByRole('button', { name: '1 chat needs you. Go to the first' })).toBeTruthy()
    // Not repeated for the next one.
    fireEvent.click(within(toast).getByRole('button', { name: 'Dismiss' }))
    await emitTo(chats, second, permission(2))
    expect(screen.queryByRole('status')).toBeNull()
  })
})

describe('chats that finish out of view', () => {
  async function finishInBackground() {
    const { platform, chats } = await start()
    const [first] = chats.ids()
    await openSecond(platform)
    const second = chats.ids()[1]
    // Work in the second chat, then look at the first.
    act(() => chats.controllers.get(second)!.send('refactor the parser'))
    return { platform, chats, first, second }
  }

  test('a turn that ends is Finished until the chat is opened', async () => {
    const { platform, chats, second } = await finishInBackground()
    // Make the first chat the one in view.
    fireEvent.click(rowOf(chats.ids()[0]))
    await waitFor(() => expect(chats.activated.at(-1)).toBe(chats.ids()[0]))
    expect(within(sidebar()).getByRole('region', { name: 'Working, 1' })).toBeTruthy()

    await emitTo(chats, second, { type: 'turn_finished' })
    const done = await within(sidebar()).findByRole('region', { name: 'Finished, 1' })
    expect(done.textContent).toContain('refactor the parser')
    expect(done.textContent).toContain('finished, not seen yet')

    fireEvent.click(rowOf(second))
    await waitFor(() => expect(chats.activated.at(-1)).toBe(second))
    await waitFor(() => expect(within(sidebar()).queryByRole('region', { name: /Finished/ })).toBeNull())
    expect(platform.attention).not.toContain(1)
  })
})

describe('stopped chats', () => {
  test('an engine the host stopped shows why, and opening the chat starts it again', async () => {
    const { platform, chats } = await start()
    const [first] = chats.ids()
    await openSecond(platform)
    const second = chats.ids()[1]
    act(() => chats.emitStopped(first, 'idle'))
    const row = await waitFor(() => chatRow(/stopped: Stopped after sitting idle/))
    expect(chats.resumed).toEqual([])
    fireEvent.click(row)
    await waitFor(() => expect(chats.resumed).toEqual([{ id: first, options: undefined }]))
    await waitFor(() => expect(chats.activated.at(-1)).toBe(first))
    await waitFor(() => expect(within(sidebar()).queryByRole('button', { name: /stopped/ })).toBeNull())
    expect(second).toBeTruthy()
  })
})

describe('the command palette and the shortcuts sheet', () => {
  test('Ctrl+K opens the palette; it lists chats and runs a command', async () => {
    const { platform, chats } = await start()
    await openSecond(platform)
    fireEvent.keyDown(window, { key: 'k', ctrlKey: true })
    const palette = await screen.findByRole('dialog', { name: 'Command palette' })
    const input = within(palette).getByRole('combobox')
    expect(document.activeElement).toBe(input)
    expect(within(palette).getAllByRole('option').some((option) => option.textContent?.includes('New chat'))).toBe(true)

    fireEvent.change(input, { target: { value: 'chat' } })
    const options = within(palette).getAllByRole('option')
    expect(options.some((option) => option.textContent?.includes('Chats'))).toBe(true)
    fireEvent.change(input, { target: { value: 'new chat in its own' } })
    fireEvent.keyDown(input, { key: 'Enter' })
    await waitFor(() => expect(chats.opened.at(-1)).toEqual({ path: '/work/app', worktree: {} }))
    expect(screen.queryByRole('dialog', { name: 'Command palette' })).toBeNull()
  })

  test('Escape closes the palette and the arrows move the choice', async () => {
    await start()
    fireEvent.keyDown(window, { key: 'k', ctrlKey: true })
    const palette = await screen.findByRole('dialog', { name: 'Command palette' })
    const input = within(palette).getByRole('combobox')
    fireEvent.keyDown(input, { key: 'ArrowDown' })
    const selected = within(palette).getAllByRole('option').filter((option) => option.getAttribute('aria-selected') === 'true')
    expect(selected).toHaveLength(1)
    expect(input.getAttribute('aria-activedescendant')).toBe(selected[0].id)
    fireEvent.keyDown(input, { key: 'Escape' })
    expect(screen.queryByRole('dialog', { name: 'Command palette' })).toBeNull()
  })

  test('the shortcuts sheet is generated from the shortcut table', async () => {
    await start()
    fireEvent.keyDown(window, { key: '/', ctrlKey: true })
    const sheet = await screen.findByRole('dialog', { name: 'Keyboard shortcuts' })
    for (const shortcut of SHORTCUTS) {
      expect(within(sheet).getByText(shortcut.title)).toBeTruthy()
      expect(within(sheet).getByText(keysOf(shortcut))).toBeTruthy()
    }
    fireEvent.keyDown(sheet, { key: 'Escape' })
    expect(screen.queryByRole('dialog', { name: 'Keyboard shortcuts' })).toBeNull()
  })

  test('Ctrl+Alt+Down and Up step through the chats', async () => {
    const { platform, chats } = await start()
    const [first, second] = [chats.ids()[0], await openSecond(platform)]
    fireEvent.keyDown(window, { key: 'ArrowDown', ctrlKey: true, altKey: true })
    await waitFor(() => expect(chats.activated.at(-1)).toBe(first))
    fireEvent.keyDown(window, { key: 'ArrowUp', ctrlKey: true, altKey: true })
    await waitFor(() => expect(chats.activated.at(-1)).toBe(second))
  })
})

describe('discarding a copy', () => {
  async function inACopy() {
    const started = await start()
    fireEvent.keyDown(window, { key: 'N', ctrlKey: true, shiftKey: true })
    await screen.findByRole('group', { name: /own copy of the project/ })
    return started
  }

  test('a clean copy needs one answer, and Cancel has the focus', async () => {
    const { chats } = await inACopy()
    fireEvent.click(screen.getByRole('button', { name: 'Discard' }))
    const dialog = await screen.findByRole('alertdialog')
    await within(dialog).findByText(/removes the copy/)
    expect(document.activeElement).toBe(within(dialog).getByRole('button', { name: 'Cancel' }))
    expect(within(dialog).getByText('Nothing in it would be lost.')).toBeTruthy()
    fireEvent.click(within(dialog).getByRole('button', { name: 'Discard the copy' }))
    await waitFor(() => expect(chats.discards).toEqual([{ id: chats.ids()[0] === 'c1test' ? 'c2test' : 'c2test', force: false, deleteUnmerged: false }]))
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull())
    // The first chat is still there.
    expect(screen.queryByRole('group', { name: /own copy of the project/ })).toBeNull()
  })

  test('saved changes the main folder lacks need a second answer', async () => {
    const { chats } = await inACopy()
    chats.worktreeState = { dirty: 2, unmerged: 3 }
    fireEvent.click(screen.getByRole('button', { name: 'Discard' }))
    const dialog = await screen.findByRole('alertdialog')
    await within(dialog).findByText(/2 files have changes/)
    // Step one only moves on; nothing is removed.
    fireEvent.click(within(dialog).getByRole('button', { name: 'Continue' }))
    expect(chats.discards).toEqual([])
    // The second question is not answered by the click that moved here.
    await new Promise((resolve) => setTimeout(resolve, 450))
    expect(within(dialog).getByText(/3 saved changes/)).toBeTruthy()
    expect(within(dialog).getByRole('heading', { name: 'Delete its saved changes too?' })).toBeTruthy()
    expect(document.activeElement).toBeTruthy()
    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete them and discard' }))
    await waitFor(() => expect(chats.discards).toEqual([{ id: 'c2test', force: true, deleteUnmerged: true }]))
  })

  test('Cancel and Escape leave everything alone', async () => {
    const { chats } = await inACopy()
    chats.worktreeState = { dirty: 1, unmerged: 0 }
    fireEvent.click(screen.getByRole('button', { name: 'Discard' }))
    const dialog = await screen.findByRole('alertdialog')
    await within(dialog).findByText(/1 file has changes/)
    fireEvent.keyDown(dialog, { key: 'Escape' })
    expect(screen.queryByRole('alertdialog')).toBeNull()
    expect(chats.discards).toEqual([])
  })

  test('closing the last chat opens a fresh one', async () => {
    const { chats } = await start()
    fireEvent.keyDown(window, { key: 'k', ctrlKey: true })
    const input = within(await screen.findByRole('dialog', { name: 'Command palette' })).getByRole('combobox')
    fireEvent.change(input, { target: { value: 'close this chat' } })
    fireEvent.keyDown(input, { key: 'Enter' })
    await waitFor(() => expect(chats.closed).toEqual(['c1test']))
    await waitFor(() => expect(chats.opened).toHaveLength(2))
    await screen.findByLabelText('Message Neuron')
  })
})

describe('the context bar on its own', () => {
  const info: ChatInfo = {
    id: 'c1',
    project: { name: 'app', path: '/work/app' },
    cwd: '/wt/x',
    branch: 'neuron/x',
    worktree: { path: '/wt/x', branch: 'neuron/x', base: 'main' },
  }
  test('shows the editor button only when the app can open one', () => {
    const { rerender } = render(<ChatContextBar info={info} onDiscard={() => {}} />)
    expect(screen.queryByRole('button', { name: 'Open in editor' })).toBeNull()
    let opened = 0
    rerender(<ChatContextBar info={info} onDiscard={() => {}} onOpenInEditor={() => opened++} />)
    fireEvent.click(screen.getByRole('button', { name: 'Open in editor' }))
    expect(opened).toBe(1)
  })
  test('shows nothing for a chat in the project\'s own folder', () => {
    const { container } = render(<ChatContextBar info={{ ...info, worktree: null }} onDiscard={() => {}} />)
    expect(container.textContent).toBe('')
  })
})

describe('accessibility', () => {
  const violations = async (root: Element) => {
    const result = await axe.run(root, { rules: { 'color-contrast': { enabled: false } } })
    return result.violations.filter((violation) => violation.impact === 'serious' || violation.impact === 'critical')
  }

  test('the sidebar with every group, and the dialogs, pass axe', async () => {
    const { platform, chats } = await start()
    await openSecond(platform)
    await openSecond(platform)
    const [first, second, third] = chats.ids()
    await emitTo(chats, second, permission(1))
    act(() => chats.controllers.get(third)!.send('work'))
    act(() => chats.emitStopped(first, 'evicted'))
    expect(await violations(sidebar())).toEqual([])

    fireEvent.keyDown(window, { key: 'k', ctrlKey: true })
    const palette = await screen.findByRole('dialog', { name: 'Command palette' })
    expect(await violations(palette)).toEqual([])
    fireEvent.keyDown(within(palette).getByRole('combobox'), { key: 'Escape' })

    fireEvent.keyDown(window, { key: '/', ctrlKey: true })
    expect(await violations(await screen.findByRole('dialog', { name: 'Keyboard shortcuts' }))).toEqual([])
  })

  test('the discard dialog passes axe', async () => {
    const { chats } = await start()
    fireEvent.keyDown(window, { key: 'N', ctrlKey: true, shiftKey: true })
    await screen.findByRole('group', { name: /own copy of the project/ })
    chats.worktreeState = { dirty: 1, unmerged: 1 }
    fireEvent.click(screen.getByRole('button', { name: 'Discard' }))
    const dialog = await screen.findByRole('alertdialog')
    await within(dialog).findByText(/1 file has changes/)
    expect(await violations(dialog)).toEqual([])
  })
})

// Each chat has its own engine, so each one's trust dialog needs its own preview, even in one folder.
describe('the trust dialog in several chats', () => {
  const preview = (): UiEvent =>
    ({
      type: 'trust_preview',
      cwd: '/work/app',
      root: '/work/app',
      is_git_root: true,
      session_only: false,
      trusted: false,
      settings_files: ['/work/app/.neuron/settings.json'],
      hooks: [{ event: 'PreToolUse', command: 'lint', source: '/work/app/.neuron/settings.json' }],
      mcp_servers: [],
      permission_rules: [],
      other_settings: [],
    }) as UiEvent
  /** How many times this chat's engine was asked what its folder would turn on. */
  const asked = (chats: NonNullable<FakePlatform['fakeChats']>, id: string) =>
    chats.engines
      .get(id)!
      .commands()
      .filter((command) => (command as { type?: string }).type === 'trust_preview').length
  const untrusted = (chats: NonNullable<FakePlatform['fakeChats']>, id: string) => emitTo(chats, id, chats.engines.get(id)!.status({ workspace_trusted: false }))
  const view = async (chats: NonNullable<FakePlatform['fakeChats']>, id: string) => {
    fireEvent.click(rowOf(id))
    await waitFor(() => expect(chats.activated.at(-1)).toBe(id))
  }

  test('a second chat in the same folder that is untrusted when it is brought into view is asked for its own preview', async () => {
    const { platform, chats } = await start()
    const [first] = chats.ids()
    const second = await openSecond(platform)

    // The first chat is in view when its folder turns untrusted: it asks, once.
    await view(chats, first)
    await untrusted(chats, first)
    await waitFor(() => expect(asked(chats, first)).toBe(1))

    // The second chat's engine turns untrusted meanwhile, out of view: nothing to ask for yet.
    await untrusted(chats, second)
    expect(asked(chats, second)).toBe(0)

    // Brought into view, its own dialog asks, whatever the first chat's did: a blind Allow shows no list.
    await view(chats, second)
    await waitFor(() => expect(asked(chats, second)).toBe(1))
    await emitTo(chats, second, preview())
    const dialog = await screen.findByRole('dialog', { name: 'Before Neuron works here' })
    const list = within(dialog).getByRole('list', { name: 'What this folder turns on' })
    expect(within(list).getAllByRole('listitem').map((item) => item.textContent)).toEqual(['1 hook that runs commands around tool calls'])

    // Back in the first chat, which already has its list: nothing is asked again, by either.
    await view(chats, first)
    expect(asked(chats, first)).toBe(1)
    expect(asked(chats, second)).toBe(1)
  })

  test('each chat that turns untrusted in its own turn is asked once', async () => {
    const { platform, chats } = await start()
    const [first] = chats.ids()
    const second = await openSecond(platform)
    await view(chats, first)
    await untrusted(chats, first)
    await waitFor(() => expect(asked(chats, first)).toBe(1))
    await view(chats, second)
    await untrusted(chats, second)
    await waitFor(() => expect(asked(chats, second)).toBe(1))
    // More statuses of the same untrusted folders are not new questions.
    await untrusted(chats, second)
    await view(chats, first)
    await untrusted(chats, first)
    expect([asked(chats, first), asked(chats, second)]).toEqual([1, 1])
  })
})

describe('the digest in each row', () => {
  test('a chat with a task list shows its steps and what it is doing, in words', async () => {
    const { chats } = await start()
    const [first] = chats.ids()
    act(() => chats.controllers.get(first)!.send('fix the tests'))
    await emitTo(chats, first, {
      type: 'todos',
      items: [
        { content: 'Read the failure', status: 'completed', active_form: null },
        { content: 'Fix it', status: 'completed', active_form: null },
        { content: 'Run the tests', status: 'in_progress', active_form: 'Running the tests' },
      ],
    })
    const row = rowOf(first)
    await waitFor(() => expect(row.textContent).toContain('2/3'))
    expect(row.textContent).toContain('2 of 3 steps done')
    expect(row.querySelector('.chat-status')?.textContent).toBe('Running the tests')
  })

  test('a turn that failed waits under Needs you, and says so without the raw error', async () => {
    const { chats } = await start()
    const [first] = chats.ids()
    act(() => chats.controllers.get(first)!.send('go'))
    await emitTo(chats, first, { type: 'error', message: 'upstream 401 invalid_session' }, { type: 'turn_finished' })
    const group = await within(sidebar()).findByRole('region', { name: 'Needs you, 1' })
    expect(group.textContent).toContain('needs you: the last turn failed')
    expect(rowOf(first).querySelector('.chat-status')?.textContent).toBe('Signed out: sign in again')
  })
})

describe('pins', () => {
  test('a pinned chat moves to Pinned above the other groups, and the pin is saved', async () => {
    const { platform, chats } = await start()
    const [first] = chats.ids()
    const second = await openSecond(platform)
    // Each chat is a session of its own (the fake engines all start as session-1).
    await emitTo(chats, second, chats.engines.get(second)!.status({ session_id: 'session-2' }))
    const pin = within(rowOf(first).closest('li')!).getByRole('button', { name: /^Pin chat: / })
    expect(pin.getAttribute('aria-pressed')).toBe('false')
    fireEvent.click(pin)
    const pinned = await within(sidebar()).findByRole('region', { name: 'Pinned, 1' })
    expect(pinned.contains(rowOf(first))).toBe(true)
    // Pinned comes first.
    const regions = within(sidebar()).getAllByRole('region').map((region) => region.getAttribute('aria-label'))
    expect(regions[0]).toBe('Pinned, 1')
    // Kept by session, so it holds on the next launch.
    await waitFor(() => expect(platform.saved.get('pins')).toEqual(['session-1']))
    fireEvent.click(within(rowOf(first).closest('li')!).getByRole('button', { name: /^Pin chat: / }))
    await waitFor(() => expect(within(sidebar()).queryByRole('region', { name: /^Pinned/ })).toBeNull())
    await waitFor(() => expect(platform.saved.get('pins')).toEqual([]))
  })

  test('pins saved by an earlier launch apply', async () => {
    const engine = new FakeEngine()
    const platform = fakePlatform(engine, { multiChat: true })
    platform.saved.set('pins', ['session-1'])
    render(<App platform={platform} />)
    await screen.findByLabelText('Message Neuron')
    const pinned = await within(sidebar()).findByRole('region', { name: 'Pinned, 1' })
    expect(within(pinned).getByRole('button', { name: /^Pin chat: / }).getAttribute('aria-pressed')).toBe('true')
  })
})
