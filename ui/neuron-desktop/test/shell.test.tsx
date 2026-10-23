/**
 * WP3, the desktop shell: the sidebar across projects with badges, the account
 * row and menu, usage, settings, the folder-trust gate, first run, gateway
 * error cards, the folder picker, the version and update notice, and the
 * keyboard and screen-reader basics on those surfaces.
 */
import { afterEach, describe, expect, test } from 'bun:test'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import axe from 'axe-core'

import { accountView, type ClientMessage, type UiEvent } from '@neuron/client'

import { App } from '../src/App'
import { UsagePanel } from '../src/components/SettingsPage'
import { LINKS } from '../src/links'
import { FakeEngine, fakePlatform, type FakePlatformOptions } from './fake-platform'

afterEach(() => {
  cleanup()
  try {
    localStorage.clear()
  } catch {
    // No storage: nothing to clear.
  }
})

async function start(statusOverrides: Record<string, unknown> = {}, options?: FakePlatformOptions, features?: string[]) {
  const engine = new FakeEngine(statusOverrides, features)
  const platform = fakePlatform(engine, options)
  render(<App platform={platform} />)
  await screen.findByLabelText('Message Neuron')
  return { engine, platform }
}

const emit = (engine: FakeEngine, ...events: UiEvent[]) =>
  act(() => {
    for (const event of events) engine.emit(event)
  })

type Command = Extract<ClientMessage, { kind: 'command' }> & { command?: string; text?: string }

const sent = (engine: FakeEngine, type: string) =>
  engine.commands().filter((message): message is Command => message.kind === 'command' && message.type === type)

const sidebar = () => screen.getByRole('navigation', { name: 'Neuron' })

/** No serious or critical axe violations in `root` (contrast needs layout, which happy-dom has none of). */
async function expectAccessible(root: Element = document.body) {
  const result = await axe.run(root, { rules: { 'color-contrast': { enabled: false } } })
  const serious = result.violations.filter((violation) => violation.impact === 'serious' || violation.impact === 'critical')
  expect(serious.map((violation) => `${violation.id}: ${violation.nodes.map((node) => node.target.join(' ')).join(', ')}`)).toEqual([])
}

const now = Date.now()
const MIN = 60_000

/** Three projects of four sessions each; `neuron` is the most recent, then `zero-x.live`, then the open `app`. */
function threeProjects() {
  const sessions = []
  const projects: Array<[string, number]> = [
    ['/work/neuron', 5],
    ['/work/zero-x.live', 60],
    ['/work/app', 600],
  ]
  for (const [path, minutes] of projects) {
    for (let index = 0; index < 4; index++) {
      sessions.push({
        id: `${path.split('/').pop()}-${index}`,
        title: `${path.split('/').pop()} task ${index}`,
        model: 'sonnet',
        message_count: 3,
        updated_at_ms: now - (minutes + index) * MIN,
        current: false,
        branch: null,
        project: path,
      })
    }
  }
  sessions[8].id = 'session-1'
  return sessions
}

describe('D1–D4, D12: the sidebar', () => {
  test('three projects of four sessions: three headings, newest activity first', async () => {
    const { engine } = await start()
    await emit(engine, { type: 'sessions', current: 'session-1', sessions: threeProjects() } as UiEvent)
    const list = within(sidebar()).getByRole('list', { name: 'On this device' })
    const headings = within(list)
      .getAllByRole('button', { expanded: true })
      .map((button) => button.textContent?.replace(/^▾\s*/, '').replace(/\s*\d+$/, ''))
    expect(headings).toEqual(['neuron', 'zero-x.live', 'app'])
    expect(within(list).getAllByRole('button', { name: /task \d/ })).toHaveLength(12)
    // Folding a project hides its sessions.
    fireEvent.click(within(list).getByRole('button', { name: /^neuron 4$/ }))
    expect(within(list).queryByRole('button', { name: /neuron task 0/ })).toBeNull()
  })

  test('a session of another project reopens that folder on it', async () => {
    const { engine, platform } = await start()
    await emit(engine, { type: 'sessions', current: 'session-1', sessions: threeProjects() } as UiEvent)
    fireEvent.click(within(sidebar()).getByRole('button', { name: /neuron task 2/ }))
    await waitFor(() => expect(platform.projectOpens).toEqual([['/work/neuron', 'neuron-2']]))
    expect(sent(engine, 'resume')).toEqual([])
  })

  test('search finds titles in every project; group by date or status', async () => {
    const { engine } = await start()
    await emit(engine, { type: 'sessions', current: 'session-1', sessions: threeProjects() } as UiEvent)
    fireEvent.change(within(sidebar()).getByLabelText('Search sessions'), { target: { value: 'task 3' } })
    expect(within(sidebar()).getAllByRole('button', { name: /task 3/ }).map((button) => button.getAttribute('title'))).toEqual([
      'neuron task 3',
      'zero-x.live task 3',
      'app task 3',
    ])
    fireEvent.change(within(sidebar()).getByLabelText('Search sessions'), { target: { value: 'nothing like this' } })
    expect(within(sidebar()).getByText('No session title matches.')).toBeTruthy()
    fireEvent.change(within(sidebar()).getByLabelText('Search sessions'), { target: { value: '' } })
    fireEvent.change(within(sidebar()).getByLabelText('Group sessions by'), { target: { value: 'date' } })
    expect(within(sidebar()).getByRole('button', { name: /Today/ })).toBeTruthy()
  })

  test('badges: running, in the terminal, unread after finishing elsewhere, failed', async () => {
    const { engine } = await start()
    const base = threeProjects()
    // The terminal UI has neuron-1 open.
    base[1] = { ...base[1], run_status: 'running' } as (typeof base)[number]
    await emit(engine, { type: 'sessions', current: 'session-1', sessions: base } as UiEvent)
    const row = (title: RegExp) => within(sidebar()).getByRole('button', { name: title })
    expect(row(/neuron task 1/).textContent).toContain('running in the terminal')

    // zero-x.live-0 finishes in the terminal while this window is elsewhere.
    const later = base.map((session) => (session.id === 'zero-x.live-0' ? { ...session, updated_at_ms: now } : session))
    await emit(engine, { type: 'sessions', current: 'session-1', sessions: later } as UiEvent)
    expect(row(/zero-x.live task 0/).textContent).toContain('unread')

    // The open session runs, then its turn fails.
    fireEvent.change(screen.getByLabelText('Message Neuron'), { target: { value: 'deploy it' } })
    fireEvent.keyDown(screen.getByLabelText('Message Neuron'), { key: 'Enter' })
    expect(row(/app task 0/).textContent).toContain('running')
    await emit(engine, { type: 'error', message: 'cargo failed' }, { type: 'turn_finished' })
    await waitFor(() => expect(row(/app task 0/).textContent).toContain('failed'))
  })

  test('the icon rail keeps navigation by name, and the keyboard toggles it', async () => {
    await start()
    fireEvent.click(within(sidebar()).getByRole('button', { name: 'Collapse the sidebar' }))
    for (const name of ['New session', 'Search', 'Projects', 'Extensions', 'Settings']) {
      expect(within(sidebar()).getByRole('button', { name })).toBeTruthy()
    }
    expect(within(sidebar()).queryByLabelText('Search sessions')).toBeNull()
    fireEvent.keyDown(window, { key: 'B', ctrlKey: true, shiftKey: true })
    expect(within(sidebar()).getByLabelText('Search sessions')).toBeTruthy()
    // Ctrl+K goes to the search box.
    fireEvent.keyDown(window, { key: 'b', ctrlKey: true, shiftKey: true })
    fireEvent.keyDown(window, { key: 'k', ctrlKey: true })
    await waitFor(() => expect(document.activeElement?.getAttribute('aria-label')).toBe('Search sessions'))
  })
})

describe('D5–D7: the account row and menu', () => {
  test('initials, name and plan; every item opens its place', async () => {
    const { platform } = await start({ account: { email: 'rahul@zero-x.live', plan: 'pro', source: 'device', expires_at: null } })
    const row = within(sidebar()).getByRole('button', { name: 'Account: rahul, Pro plan' })
    expect(row.textContent).toContain('R')
    expect(row.textContent).toContain('Pro')

    const openMenu = () => {
      fireEvent.click(within(sidebar()).getByRole('button', { name: /^Account:/ }))
      return screen.getByRole('menu', { name: 'Account' })
    }
    let menu = openMenu()
    expect(within(menu).getByText('rahul@zero-x.live')).toBeTruthy()
    expect(within(menu).getByText('Pro')).toBeTruthy()
    fireEvent.keyDown(menu, { key: 'Escape' })
    const external: Array<[RegExp, string]> = [
      [/Plans and pricing/, LINKS.pricing],
      [/API keys/, LINKS.apiKeys],
      [/Download Neuron/, LINKS.download],
      [/Help and support/, LINKS.help],
    ]
    for (const [name, url] of external) {
      fireEvent.click(within(openMenu()).getByRole('menuitem', { name }))
      expect(platform.opened.at(-1)).toBe(url)
    }
    fireEvent.click(within(openMenu()).getByRole('menuitem', { name: /Usage and plan/ }))
    expect(screen.getByRole('heading', { name: 'Usage and plan' })).toBeTruthy()
    fireEvent.click(within(openMenu()).getByRole('menuitem', { name: /What's new/ }))
    expect(screen.getByRole('heading', { name: 'About' })).toBeTruthy()
    menu = openMenu()
    fireEvent.click(within(menu).getByRole('menuitem', { name: /Settings/ }))
    expect(screen.getByRole('heading', { name: 'Account' })).toBeTruthy()
  })

  test('sign out asks first, then returns to the sign-in screen', async () => {
    const { engine, platform } = await start()
    fireEvent.click(within(sidebar()).getByRole('button', { name: /^Account:/ }))
    fireEvent.click(screen.getByRole('menuitem', { name: 'Sign out' }))
    const dialog = screen.getByRole('alertdialog', { name: 'Sign out of Zero-X?' })
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))
    expect(sent(engine, 'slash')).toEqual([])

    fireEvent.click(within(sidebar()).getByRole('button', { name: /^Account:/ }))
    fireEvent.click(screen.getByRole('menuitem', { name: 'Sign out' }))
    fireEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Sign out' }))
    expect(sent(engine, 'slash').at(-1)).toEqual({ kind: 'command', type: 'slash', command: 'logout', args: '' })
    // Once the engine forgot the sign-in, a fresh engine starts on the sign-in screen.
    expect(platform.restarts).toEqual([])
    await emit(engine, { type: 'turn_finished' })
    await waitFor(() => expect(platform.restarts).toEqual(['session-1']))
  })

  test('on the Max plan there is no plans link; signed out, the row signs in', async () => {
    await start({ account: { email: 'max@example.com', plan: 'max', source: 'device', expires_at: null } })
    fireEvent.click(within(sidebar()).getByRole('button', { name: /^Account:/ }))
    expect(screen.queryByRole('menuitem', { name: /Plans and pricing/ })).toBeNull()
    cleanup()
    const { engine } = await start({ sign_in_required: true, signed_in: false, account: null, session_id: '' })
    fireEvent.click(within(sidebar()).getByRole('button', { name: 'Sign in to Zero-X' }))
    expect(sent(engine, 'slash').at(-1)).toEqual({ kind: 'command', type: 'slash', command: 'login', args: '' })
  })

  test('the menu works by keyboard and gives focus back', async () => {
    await start()
    const row = within(sidebar()).getByRole('button', { name: /^Account:/ })
    row.focus()
    fireEvent.click(row)
    const menu = screen.getByRole('menu', { name: 'Account' })
    const items = within(menu).getAllByRole('menuitem')
    await waitFor(() => expect(document.activeElement).toBe(items[0]))
    fireEvent.keyDown(menu, { key: 'ArrowDown' })
    expect(document.activeElement).toBe(items[1])
    fireEvent.keyDown(menu, { key: 'End' })
    expect(document.activeElement).toBe(items.at(-1)!)
    fireEvent.keyDown(menu, { key: 'ArrowDown' })
    expect(document.activeElement).toBe(items[0])
    await expectAccessible()
    fireEvent.keyDown(menu, { key: 'Escape' })
    expect(screen.queryByRole('menu')).toBeNull()
    expect(document.activeElement).toBe(within(sidebar()).getByRole('button', { name: /^Account:/ }))
  })

  test('the usage page: bars, the reset time and credits', () => {
    const account = accountView(true, null, {
      email: 'rahul@zero-x.live',
      name: 'Rahul S',
      plan: 'pro',
      limits: { requests_per_day: 200, tokens_per_day: 1_000_000 },
      usage_today: { requests: 190, tokens: 120_000 },
      resets_at: '2026-10-24T00:00:00Z',
      credits_usd: 4.5,
    })
    const opened: string[] = []
    render(<UsagePanel account={account} onOpenLink={(url) => opened.push(url)} />)
    const bars = screen.getAllByRole('progressbar')
    expect(bars.map((bar) => [bar.getAttribute('aria-label'), bar.getAttribute('aria-valuenow')])).toEqual([
      ['Requests today', '95'],
      ['Tokens today', '12'],
    ])
    expect(screen.getByText(/190 of 200/)).toBeTruthy()
    expect(screen.getByText(/^Resets /)).toBeTruthy()
    expect(screen.getByText('Credit balance: $4.50')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: /Top up credits/ }))
    expect(opened).toEqual([LINKS.pricing])
  })
})

describe('D9: folder trust', () => {
  test('an untrusted folder blocks prompts until it is allowed', async () => {
    const { engine, platform } = await start({ workspace_trusted: false })
    const dialog = screen.getByRole('dialog', { name: 'Before Neuron works here' })
    expect(within(dialog).getByText('/work/app')).toBeTruthy()
    expect(within(dialog).getByText(/open, change and run files/)).toBeTruthy()
    expect((screen.getByLabelText('Message Neuron') as HTMLTextAreaElement).disabled).toBe(true)
    await expectAccessible()
    // Focus stays in the dialog.
    const allow = within(dialog).getByRole('button', { name: 'Allow Neuron in this folder' })
    await waitFor(() => expect(document.activeElement).toBe(allow))
    fireEvent.keyDown(dialog, { key: 'Tab' })
    expect(dialog.contains(document.activeElement)).toBe(true)

    fireEvent.click(allow)
    expect(sent(engine, 'trust_workspace')).toHaveLength(1)
    expect(sent(engine, 'submit')).toEqual([])
    await emit(engine, { type: 'workspace_trusted', cwd: '/work/app', restart_required: true })
    // The folder's own settings load in a fresh engine.
    await waitFor(() => expect(platform.restarts).toEqual([null]))
  })

  test('or open a different folder', async () => {
    const { platform } = await start({ workspace_trusted: false })
    fireEvent.click(screen.getByRole('button', { name: 'Open a different folder' }))
    await waitFor(() => expect(platform.projectOpens).toEqual([['/work/picked', null]]))
  })
})

describe('D10: first run, and gateway errors as cards', () => {
  test('welcome, sign in, then the folder', async () => {
    const engine = new FakeEngine({ sign_in_required: true, signed_in: false, account: null, session_id: '' })
    const platform = fakePlatform(engine, { settings: { firstRunDone: false } })
    render(<App platform={platform} />)
    const dialog = await screen.findByRole('dialog', { name: 'Welcome to Neuron' })
    await expectAccessible()
    fireEvent.click(within(dialog).getByRole('button', { name: 'Get started' }))
    expect(within(dialog).getByRole('heading', { name: 'Sign in' })).toBeTruthy()
    fireEvent.click(within(dialog).getByRole('button', { name: 'Sign in' }))
    expect(sent(engine, 'slash').at(-1)).toMatchObject({ command: 'login' })
    // Signed in: the engine's status says so.
    await emit(engine, engine.status({ sign_in_required: false, signed_in: true, session_id: 'session-1' }))
    await waitFor(() => expect(within(dialog).getByRole('heading', { name: 'Which folder should Neuron work in?' })).toBeTruthy())
    fireEvent.click(within(dialog).getByRole('button', { name: 'Open folder…' }))
    await waitFor(() => expect(platform.projectOpens).toEqual([['/work/picked', null]]))
    await waitFor(() => expect(screen.queryByRole('dialog', { name: /Welcome|Which folder/ })).toBeNull())
    expect((platform.saved.get('settings') as { firstRunDone: boolean }).firstRunDone).toBe(true)
  })

  test("sign-in when Zero-X can't be reached", async () => {
    const { engine, platform } = await start({ sign_in_required: true, signed_in: false, account: null, session_id: '' })
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))
    await emit(
      engine,
      { type: 'error', message: 'This Zero-X gateway does not support device sign-in yet. Use `neuron auth login --with-token`.' },
      { type: 'turn_finished' },
    )
    const card = screen.getByRole('alert', { name: "Can't reach Zero-X right now" })
    fireEvent.click(within(card).getByRole('button', { name: /Use my own provider key/ }))
    expect(platform.opened.at(-1)).toBe(LINKS.providerKeys)
    fireEvent.click(within(card).getByRole('button', { name: 'Retry' }))
    expect(sent(engine, 'slash').filter((message) => message.command === 'login')).toHaveLength(2)
  })

  const cases: Array<[string, Record<string, unknown>, string]> = [
    ['401', { message: 'Session expired', status: 401 }, 'Sign in again'],
    ['403 upgrade_required', { message: 'Pro only', code: 'upgrade_required', upgrade_url: 'https://zero-x.live/pricing?plan=pro' }, 'This needs a paid plan'],
    ['429 insufficient_quota', { message: 'Daily quota reached', code: 'insufficient_quota', resets_at: '2026-10-24T00:00:00Z' }, "You've used today's quota"],
    ['503', { message: 'Busy', status: 503, retry_after_seconds: 0 }, 'Zero-X is busy'],
    ['a network error', { message: 'Could not reach Zero-X: connection refused' }, "Can't reach Zero-X right now"],
  ]
  for (const [what, error, title] of cases) {
    test(`${what}: its card, and the prompt stays in the message box`, async () => {
      const { engine, platform } = await start()
      const box = screen.getByLabelText('Message Neuron') as HTMLTextAreaElement
      fireEvent.change(box, { target: { value: 'refactor the parser' } })
      fireEvent.keyDown(box, { key: 'Enter' })
      expect(box.value).toBe('')
      await emit(engine, { type: 'error', ...error } as UiEvent, { type: 'turn_finished' })
      const card = screen.getByRole('alert', { name: title })
      expect(box.value).toBe('refactor the parser')
      if (what === '403 upgrade_required') {
        fireEvent.click(within(card).getByRole('button', { name: /Plans and pricing/ }))
        expect(platform.opened.at(-1)).toBe('https://zero-x.live/pricing?plan=pro')
      }
      if (what === '429 insufficient_quota') expect(within(card).getByText(/It resets at/)).toBeTruthy()
      if (what === '401') {
        fireEvent.click(within(card).getByRole('button', { name: 'Sign in to Zero-X' }))
        expect(sent(engine, 'slash').at(-1)).toMatchObject({ command: 'login' })
      }
      if (what === '503' || what === 'a network error') {
        fireEvent.click(within(card).getByRole('button', { name: /Try again|Retry/ }))
        expect(sent(engine, 'submit').map((message) => message.text)).toEqual(['refactor the parser', 'refactor the parser'])
      }
    })
  }
})

describe('D8, D11, D23: settings, the folder picker, version and updates', () => {
  test('Sessions defaults reach the next engine, and a folder keeps its mode', async () => {
    const { platform } = await start()
    fireEvent.click(within(sidebar()).getByRole('button', { name: 'Settings' }))
    fireEvent.click(screen.getByRole('button', { name: 'Sessions' }))
    fireEvent.change(screen.getByLabelText('Default model'), { target: { value: 'opus' } })
    fireEvent.change(screen.getByLabelText('Default permission mode'), { target: { value: 'read-only' } })
    fireEvent.click(screen.getByRole('button', { name: 'Close settings' }))
    // This folder is left in full access.
    fireEvent.click(screen.getByRole('button', { name: 'Permission mode' }))
    fireEvent.click(screen.getByRole('option', { name: /Full access/ }))
    fireEvent.click(within(sidebar()).getByRole('button', { name: 'Projects' }))
    fireEvent.click(await screen.findByRole('button', { name: /website/ }))
    await waitFor(() => expect(platform.projectOpens).toEqual([['/work/website', null]]))
    expect(platform.launches.at(-1)).toEqual({ model: 'opus', permissionMode: 'read-only' })
    const saved = platform.saved.get('settings') as { modeByFolder: Record<string, string> }
    expect(saved.modeByFolder['/work/app']).toBe('danger-full-access')
  })

  test('Open folder… uses the system picker', async () => {
    const { platform } = await start()
    fireEvent.click(within(sidebar()).getByRole('button', { name: 'Projects' }))
    fireEvent.click(screen.getByRole('button', { name: 'Open folder…' }))
    await waitFor(() => expect(platform.projectOpens).toEqual([['/work/picked', null]]))
    platform.picked = null
    fireEvent.click(within(sidebar()).getByRole('button', { name: 'Projects' }))
    fireEvent.click(screen.getByRole('button', { name: 'Open folder…' }))
    await Promise.resolve()
    expect(platform.projectOpens).toHaveLength(1)
  })

  test('About: the versions copy on click; a newer release shows a notice', async () => {
    const copied: string[] = []
    Object.defineProperty(navigator, 'clipboard', { value: { writeText: async (text: string) => void copied.push(text) }, configurable: true })
    const { platform } = await start({}, { update: { version: '0.2.1', notes: null } })
    const notice = await within(sidebar()).findByRole('status')
    expect(notice.textContent).toContain('Neuron 0.2.1 is available.')
    fireEvent.click(within(notice).getByRole('button', { name: /Download/ }))
    expect(platform.opened.at(-1)).toBe(LINKS.download)

    fireEvent.click(within(sidebar()).getByRole('button', { name: 'Settings' }))
    fireEvent.click(screen.getByRole('button', { name: 'About' }))
    const version = await screen.findByRole('button', { name: /Neuron Desktop 0\.2\.0 · engine 6\.3\.0/ })
    fireEvent.click(version)
    await waitFor(() => expect(copied).toEqual(['Neuron Desktop 0.2.0 · engine 6.3.0']))
    expect(screen.getByText('Copied')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Check for updates' }))
    await waitFor(() => expect(screen.getByText('Neuron 0.2.1 is available.', { selector: 'span' })).toBeTruthy())
    await expectAccessible()
  })

  test('the settings page and the sidebar pass axe', async () => {
    const { engine } = await start()
    await emit(engine, { type: 'sessions', current: 'session-1', sessions: threeProjects() } as UiEvent)
    await expectAccessible()
    fireEvent.click(within(sidebar()).getByRole('button', { name: 'Settings' }))
    for (const section of ['Account', 'Usage and plan', 'Appearance', 'Sessions', 'About']) {
      fireEvent.click(within(screen.getByRole('navigation', { name: 'Settings sections' })).getByRole('button', { name: section }))
      await expectAccessible()
    }
    // Escape goes back to the session.
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(screen.getByLabelText('Message Neuron')).toBeTruthy()
  })
})
