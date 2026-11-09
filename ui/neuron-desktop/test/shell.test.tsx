/**
 * WP3, the desktop shell: the sidebar across projects with badges, the account
 * row and menu, usage, settings, the folder-trust gate, first run, gateway
 * error cards, the folder picker, the version and update notice, and the
 * keyboard and screen-reader basics on those surfaces.
 */
import { afterEach, describe, expect, test } from 'bun:test'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import axe from 'axe-core'
import { StrictMode } from 'react'

import {
  accountView,
  trustChangedKeysLine,
  trustCopy,
  type AccountDetails,
  type ClientMessage,
  type ErrorDetail,
  type TrustPreviewEvent,
  type UiEvent,
} from '@neuron/client'

import { App } from '../src/App'
import { UsagePanel } from '../src/components/SettingsPage'
import { TrustModal } from '../src/components/TrustModal'
import { LINKS } from '../src/links'
import { ALL_FEATURES, FakeEngine, fakePlatform, type FakePlatformOptions } from './fake-platform'

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

/** An `account` event's account, as the engine sends it. */
function accountDetails(over: Partial<AccountDetails> = {}): AccountDetails {
  return {
    email: 'rahul@zero-x.live',
    name: 'Rahul S',
    plan: 'pro',
    plan_name: 'Pro',
    source: 'device',
    expires_at: null,
    limits: { requests: 200, tokens: 1_000_000 },
    usage_today: { requests: 74, tokens: 120_000 },
    resets_at: '2026-10-24T00:00:00Z',
    credit_balance_cents: 450,
    suspended: false,
    dashboard_url: 'https://zero-x.live/dashboard',
    pricing_url: 'https://zero-x.live/pricing?from=account',
    usage_updated_at: 1_792_800_000,
    usage_error: null,
    ...over,
  }
}

const detail = (over: Partial<ErrorDetail>): ErrorDetail => ({
  code: 'unknown',
  status: null,
  upgrade_url: null,
  resets_at: null,
  retry_after_seconds: null,
  ...over,
})


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
    expect(sent(engine, 'list_sessions').at(-1)).toEqual({ kind: 'command', type: 'list_sessions', scope: 'all' })
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
    // The terminal UI runs neuron-1; another desktop window has neuron-2 open; neuron-3 failed in the terminal.
    base[1] = { ...base[1], run_status: 'running', client: 'terminal' } as (typeof base)[number]
    base[2] = { ...base[2], run_status: 'open', client: 'desktop' } as (typeof base)[number]
    base[3] = { ...base[3], failed: true } as (typeof base)[number]
    await emit(engine, { type: 'sessions', current: 'session-1', sessions: base } as UiEvent)
    const row = (title: RegExp) => within(sidebar()).getByRole('button', { name: title })
    expect(row(/neuron task 1/).textContent).toContain('running in the terminal')
    expect(row(/neuron task 2/).textContent).toContain('open in another window')
    expect(row(/neuron task 3/).textContent).toContain('failed')

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
    // Ctrl+K opens the command palette; searching sessions is one of its commands.
    fireEvent.keyDown(window, { key: 'b', ctrlKey: true, shiftKey: true })
    fireEvent.keyDown(window, { key: 'k', ctrlKey: true })
    const palette = await screen.findByRole('dialog', { name: 'Command palette' })
    fireEvent.change(within(palette).getByRole('combobox'), { target: { value: 'search sessions' } })
    fireEvent.keyDown(within(palette).getByRole('combobox'), { key: 'Enter' })
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
    const account = accountView(true, null, accountDetails({ usage_today: { requests: 190, tokens: 120_000 } }))
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
    expect(opened).toEqual(['https://zero-x.live/pricing?from=account'])
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
    ['401', { message: 'Session expired', detail: detail({ code: 'invalid_session', status: 401 }) }, 'Sign in again'],
    [
      '403 upgrade_required',
      { message: 'Pro only', detail: detail({ code: 'upgrade_required', status: 403, upgrade_url: 'https://zero-x.live/pricing?plan=pro' }) },
      'This needs a paid plan',
    ],
    [
      '429 insufficient_quota',
      { message: 'Daily quota reached', detail: detail({ code: 'insufficient_quota', status: 429, resets_at: '2026-10-24T00:00:00Z' }) },
      "You've used today's quota",
    ],
    ['503', { message: 'Busy', detail: detail({ code: 'service_unavailable', status: 503, retry_after_seconds: 0 }) }, 'Zero-X is busy'],
    ['a network error', { message: 'Could not reach Zero-X: connection refused', detail: detail({ code: 'unreachable' }) }, "Can't reach Zero-X right now"],
    ["an older engine's wording", { message: 'Could not reach Zero-X: connection refused' }, "Can't reach Zero-X right now"],
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
      if (what === '503' || what === 'a network error' || what === "an older engine's wording") {
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

  test('an unsigned build sends the user to the releases page, and shows no false update notice', async () => {
    const { platform } = await start({}, { manualUpdates: true })
    await waitFor(() => expect(platform.checks).toBe(0))
    expect(within(sidebar()).queryByRole('status')).toBeNull()
    fireEvent.click(within(sidebar()).getByRole('button', { name: 'Settings' }))
    fireEvent.click(screen.getByRole('button', { name: 'About' }))
    fireEvent.click(screen.getByRole('button', { name: 'Check for updates' }))
    await waitFor(() => expect(screen.getByText(/new version is on the releases page/i)).toBeTruthy())
    fireEvent.click(screen.getByRole('button', { name: /Releases page/ }))
    expect(platform.opened.at(-1)).toBe(LINKS.releases)
    expect(platform.checks).toBe(0)
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
  }, 30_000)
})

describe('protocol v2: the account event and the trust preview', () => {
  test("the account row and menu read the engine's account, and the usage page refreshes it", async () => {
    const { engine, platform } = await start()
    await emit(engine, { type: 'account', signed_in: true, account: accountDetails() })
    const row = within(sidebar()).getByRole('button', { name: 'Account: Rahul S, Pro plan' })
    expect(row.textContent).toContain('RS')
    fireEvent.click(row)
    const menu = screen.getByRole('menu', { name: 'Account' })
    expect(within(menu).getByText('Pro · 37% of today used')).toBeTruthy()
    fireEvent.click(within(menu).getByRole('menuitem', { name: /Plans and pricing/ }))
    expect(platform.opened.at(-1)).toBe('https://zero-x.live/pricing?from=account')

    fireEvent.click(within(sidebar()).getByRole('button', { name: /^Account:/ }))
    fireEvent.click(screen.getByRole('menuitem', { name: /Usage and plan/ }))
    await waitFor(() => expect(sent(engine, 'refresh_account')).toHaveLength(1))
    expect(screen.getAllByRole('progressbar').map((bar) => bar.getAttribute('aria-valuenow'))).toEqual(['37', '12'])
    expect(screen.getByText('Credit balance: $4.50')).toBeTruthy()

    // A suspended account says so; signing out on the engine clears the row.
    await emit(engine, { type: 'account', signed_in: true, account: accountDetails({ suspended: true }) })
    expect(screen.getByRole('alert').textContent).toContain('suspended')
    await emit(engine, { type: 'account', signed_in: false, account: null }, engine.status({ signed_in: false, account: null }))
    expect(within(sidebar()).getByRole('button', { name: 'Sign in to Zero-X' })).toBeTruthy()
  })

  test('the trust dialog lists what the folder turns on, in the shared wording', async () => {
    const { engine } = await start({ workspace_trusted: false })
    await waitFor(() => expect(sent(engine, 'trust_preview')).toHaveLength(1))
    await emit(engine, {
      type: 'trust_preview',
      cwd: '/work/app',
      root: '/work/app',
      is_git_root: true,
      session_only: false,
      trusted: false,
      settings_files: ['/work/app/.neuron/settings.json'],
      hooks: [
        { event: 'PreToolUse', command: 'lint', source: '/work/app/.neuron/settings.json' },
        { event: 'PostToolUse', command: 'fmt', source: '/work/app/.neuron/settings.json' },
      ],
      mcp_servers: [{ name: 'postgres-tools', transport: 'stdio', target: 'pg-mcp', source: '/work/app/.neuron/settings.json' }],
      permission_rules: [{ effect: 'allow', rule: 'Bash(cargo test)', source: '/work/app/.neuron/settings.json' }],
      other_settings: [],
    })
    const dialog = screen.getByRole('dialog', { name: 'Before Neuron works here' })
    expect(within(dialog).getByText('(git root: this folder)')).toBeTruthy()
    const list = within(dialog).getByRole('list', { name: 'What this folder turns on' })
    expect(within(list).getAllByRole('listitem').map((item) => item.textContent)).toEqual([
      '2 hooks that run commands around tool calls',
      '1 MCP server: postgres-tools',
      '1 permission rule from .neuron/settings.json',
    ])
    await expectAccessible()
  })

  test('the home folder is trusted for the next engine only', async () => {
    const { engine, platform } = await start({ workspace_trusted: false, cwd: '/home/rahul' })
    await waitFor(() => expect(sent(engine, 'trust_preview')).toHaveLength(1))
    await emit(engine, {
      type: 'trust_preview',
      cwd: '/home/rahul',
      root: '/home/rahul',
      is_git_root: false,
      session_only: true,
      trusted: false,
      settings_files: [],
      hooks: [],
      mcp_servers: [],
      permission_rules: [],
      other_settings: [],
    })
    const dialog = screen.getByRole('dialog', { name: 'Before Neuron works here' })
    expect(within(dialog).getByText(/does not turn on any hooks/)).toBeTruthy()
    expect(within(dialog).getByText(/for this session only/)).toBeTruthy()
    fireEvent.click(within(dialog).getByRole('button', { name: 'Allow Neuron in this folder' }))
    await emit(engine, { type: 'workspace_trusted', cwd: '/home/rahul', restart_required: true, session_only: true, root: '/home/rahul' })
    await waitFor(() => expect(platform.trustedOnce).toEqual([null]))
  })

  // The trust dialog, and what the engine's `trust_preview` says for /work/app.
  const trustDialog = () => screen.getByRole('dialog', { name: 'Before Neuron works here' })
  const preview = (commands: string[], over: Record<string, unknown> = {}): UiEvent =>
    ({
      type: 'trust_preview',
      cwd: '/work/app',
      root: '/work/app',
      is_git_root: true,
      session_only: false,
      trusted: false,
      settings_files: ['/work/app/.neuron/settings.json'],
      hooks: commands.map((command) => ({ event: 'PreToolUse', command, source: '/work/app/.neuron/settings.json' })),
      mcp_servers: [],
      permission_rules: [],
      other_settings: [],
      ...over,
    }) as UiEvent
  const listed = () =>
    within(within(trustDialog()).getByRole('list', { name: 'What this folder turns on' }))
      .getAllByRole('listitem')
      .map((item) => item.textContent)

  // From 7.0.0-alpha.5 the engine asks for trust again when a trusted folder's hooks, MCP launch specs or
  // allow rules change: the same folder is untrusted once more, and the dialog needs a fresh preview.
  describe('a trusted folder that asks for trust again', () => {
    test('trusted, then untrusted again, asks what the folder turns on a second time', async () => {
      const { engine } = await start({ workspace_trusted: false })
      await waitFor(() => expect(sent(engine, 'trust_preview')).toHaveLength(1))
      await emit(engine, preview(['lint']))
      expect(listed()).toEqual(['1 hook that runs commands around tool calls'])

      // Every status says the folder is untrusted: that is not a new question.
      await emit(engine, engine.status({ workspace_trusted: false }))
      expect(sent(engine, 'trust_preview')).toHaveLength(1)

      // Trusted: the dialog is gone.
      await emit(engine, engine.status({ workspace_trusted: true }))
      expect(screen.queryAllByRole('dialog', { name: 'Before Neuron works here' }).length).toBe(0)
      expect(sent(engine, 'trust_preview')).toHaveLength(1)

      // Its hooks changed and the engine wants trust again: the preview is asked for again, and replaces the old one.
      await emit(engine, engine.status({ workspace_trusted: false }))
      await waitFor(() => expect(sent(engine, 'trust_preview')).toHaveLength(2))
      await emit(engine, preview(['lint', 'deploy'], { changed_since_trust: true, changed_settings: ['hooks'] }))
      expect(listed()).toEqual(['2 hooks that run commands around tool calls'])

      // And once more only, for as long as it stays untrusted.
      await emit(engine, engine.status({ workspace_trusted: false }))
      expect(sent(engine, 'trust_preview')).toHaveLength(2)
    })

    test('allowed, the engine restarted, and a later change asks again', async () => {
      const { engine, platform } = await start({ workspace_trusted: false })
      await waitFor(() => expect(sent(engine, 'trust_preview')).toHaveLength(1))
      await emit(engine, preview(['lint']))
      fireEvent.click(within(trustDialog()).getByRole('button', { name: 'Allow Neuron in this folder' }))
      await emit(engine, { type: 'workspace_trusted', cwd: '/work/app', restart_required: true })
      await waitFor(() => expect(platform.restarts).toEqual([null]))
      // The fresh engine says it is ready and that the folder is trusted.
      await emit(engine, { type: 'ready', protocol: 1, version: '6.3.0', features: ALL_FEATURES }, engine.status({ workspace_trusted: true }))
      expect(screen.queryAllByRole('dialog', { name: 'Before Neuron works here' }).length).toBe(0)
      expect(sent(engine, 'trust_preview')).toHaveLength(1)

      // Later, an MCP launch spec changes.
      await emit(engine, engine.status({ workspace_trusted: false }))
      await waitFor(() => expect(sent(engine, 'trust_preview')).toHaveLength(2))
      await emit(engine, preview([], { mcp_servers: [{ name: 'postgres-tools', transport: 'stdio', target: 'pg-mcp --new', source: '/work/app/.neuron/settings.json' }] }))
      expect(listed()).toEqual(['1 MCP server: postgres-tools'])
    })

    test('a folder that stays untrusted is asked about once', async () => {
      const { engine } = await start({ workspace_trusted: false })
      await waitFor(() => expect(sent(engine, 'trust_preview')).toHaveLength(1))
      for (let round = 0; round < 3; round++) await emit(engine, engine.status({ workspace_trusted: false }))
      expect(sent(engine, 'trust_preview')).toHaveLength(1)
    })

    test('under StrictMode, which runs every effect twice, exactly one preview is asked for', async () => {
      const engine = new FakeEngine({ workspace_trusted: false })
      render(
        <StrictMode>
          <App platform={fakePlatform(engine)} />
        </StrictMode>,
      )
      await screen.findByLabelText('Message Neuron')
      await waitFor(() => expect(sent(engine, 'trust_preview')).toHaveLength(1))
      for (let round = 0; round < 3; round++) await emit(engine, engine.status({ workspace_trusted: false }))
      expect(sent(engine, 'trust_preview')).toHaveLength(1)
    })
  })

  /** The engine process ends, as a crash or a stop does. */
  const exits = (engine: FakeEngine) =>
    act(async () => {
      for (const listener of engine.closeListeners) listener({ code: 1, error: 'The Neuron engine stopped (exit code 1).' })
    })
  /** A fresh engine says it is ready, and what it makes of the folder. */
  const comesBack = (engine: FakeEngine, trusted: boolean) =>
    emit(engine, { type: 'ready', protocol: 1, version: '6.3.0', features: ALL_FEATURES }, engine.status({ workspace_trusted: trusted }))

  // A fresh engine's `ready` clears the preview the dialog had, so an engine that goes away and comes
  // back while the folder stays untrusted has to be asked again, or the dialog opens without its list.
  describe('an engine that restarts while the folder stays untrusted', () => {
    test('one that exits and comes back is asked for the preview again, once', async () => {
      const { engine } = await start({ workspace_trusted: false })
      await waitFor(() => expect(sent(engine, 'trust_preview')).toHaveLength(1))
      await emit(engine, preview(['lint']))
      expect(listed()).toEqual(['1 hook that runs commands around tool calls'])

      await exits(engine)
      expect(sent(engine, 'trust_preview')).toHaveLength(1)

      // The fresh engine has no preview to show, so the dialog is asked to be filled in again.
      await comesBack(engine, false)
      await waitFor(() => expect(sent(engine, 'trust_preview')).toHaveLength(2))
      await emit(engine, preview(['lint']))
      expect(listed()).toEqual(['1 hook that runs commands around tool calls'])

      // And that engine is asked once: more statuses of the same untrusted folder are not new questions.
      for (let round = 0; round < 2; round++) await emit(engine, engine.status({ workspace_trusted: false }))
      expect(sent(engine, 'trust_preview')).toHaveLength(2)
    })

    test('one that restarts through `connecting` is asked for the preview again', async () => {
      const { engine, platform } = await start({ workspace_trusted: false })
      await waitFor(() => expect(sent(engine, 'trust_preview')).toHaveLength(1))
      const controller = await platform.connect()
      await act(async () => controller.apply({ type: 'local/connecting' }))
      expect(sent(engine, 'trust_preview')).toHaveLength(1)
      await comesBack(engine, false)
      await waitFor(() => expect(sent(engine, 'trust_preview')).toHaveLength(2))
    })

    test('a folder that is trusted when the engine comes back is not asked about', async () => {
      const { engine } = await start({ workspace_trusted: false })
      await waitFor(() => expect(sent(engine, 'trust_preview')).toHaveLength(1))
      await emit(engine, engine.status({ workspace_trusted: true }))
      await exits(engine)
      await comesBack(engine, true)
      expect(screen.queryAllByRole('dialog', { name: 'Before Neuron works here' }).length).toBe(0)
      expect(sent(engine, 'trust_preview')).toHaveLength(1)
    })
  })

  // Allow is answered by `workspace_trusted`, and then a restart. From 7.0.0-alpha.5 the engine can refuse
  // it too: when the project's settings changed after the dialog's list was made it trusts nothing and
  // answers with an `error` and a fresh `trust_preview`, and when it cannot record the trust, with an
  // `error` (engine_trust.rs). No restart follows, so the dialog has to be usable again, not stuck on
  // "Starting Neuron with this folder's settings" with both buttons off.
  describe('an Allow the engine did not act on', () => {
    const STALE =
      'The settings in /work/app changed after the trust screen was shown, so nothing was trusted. Read what it asks for now and allow again.'
    const primary = () => trustDialog().querySelector<HTMLButtonElement>('button.primary')!
    /** Whether each of the dialog's buttons is off, in order. */
    const off = () => [...trustDialog().querySelectorAll<HTMLButtonElement>('.trust-buttons button')].map((button) => button.disabled)
    /** An untrusted folder, with the dialog's list when asked for, and Allow pressed on it. */
    async function allowed(withPreview = true) {
      const started = await start({ workspace_trusted: false })
      await waitFor(() => expect(sent(started.engine, 'trust_preview')).toHaveLength(1))
      if (withPreview) await emit(started.engine, preview(['lint']))
      fireEvent.click(primary())
      expect(sent(started.engine, 'trust_workspace')).toHaveLength(1)
      // While it waits both buttons are off and the first says what is happening.
      expect(off()).toEqual([true, true])
      expect(primary().textContent).toBe(trustCopy.restarting)
      return started
    }

    test('a stale Allow, answered with an error and a fresh preview, can be allowed again', async () => {
      const { engine, platform } = await allowed()
      await emit(engine, { type: 'error', message: STALE }, preview(['lint', 'deploy']))
      expect(off()).toEqual([false, false])
      expect(primary().textContent).toBe(trustCopy.allow)
      // The dialog shows what the files say now.
      expect(listed()).toEqual(['2 hooks that run commands around tool calls'])
      expect(platform.restarts).toEqual([])

      // Allowed again, this time it is trusted, and the usual restart follows.
      fireEvent.click(primary())
      expect(sent(engine, 'trust_workspace')).toHaveLength(2)
      expect(off()).toEqual([true, true])
      await emit(engine, { type: 'workspace_trusted', cwd: '/work/app', restart_required: true })
      await waitFor(() => expect(platform.restarts).toEqual([null]))
    })

    test('the preview may come before the error', async () => {
      const { engine } = await allowed()
      await emit(engine, preview(['lint', 'deploy']), { type: 'error', message: STALE })
      expect(off()).toEqual([false, false])
    })

    test('a fresh preview alone ends the wait', async () => {
      const { engine } = await allowed()
      await emit(engine, preview(['lint', 'deploy']))
      expect(off()).toEqual([false, false])
    })

    test('an Allow the engine could not record, answered with an error alone, can be tried again', async () => {
      const { engine } = await allowed()
      await emit(engine, { type: 'error', message: 'could not trust /work/app: permission denied' })
      expect(off()).toEqual([false, false])
      // What the dialog showed is still what it shows.
      expect(listed()).toEqual(['1 hook that runs commands around tool calls'])
      fireEvent.click(primary())
      expect(sent(engine, 'trust_workspace')).toHaveLength(2)
    })

    test('an engine that goes away while Allow waits leaves the dialog usable on the next one', async () => {
      const { engine } = await allowed()
      await exits(engine)
      await comesBack(engine, false)
      expect(off()).toEqual([false, false])
      fireEvent.click(primary())
      expect(sent(engine, 'trust_workspace')).toHaveLength(2)
    })

    test('the first preview, arriving after Allow was pressed, is not an answer to it', async () => {
      const { engine, platform } = await allowed(false)
      // It is the answer to the dialog's own request: nothing was refused, so the wait goes on.
      await emit(engine, preview(['lint']))
      expect(off()).toEqual([true, true])
      await emit(engine, { type: 'workspace_trusted', cwd: '/work/app', restart_required: true })
      await waitFor(() => expect(platform.restarts).toEqual([null]))
    })

    test('an error that was there before Allow does not end the wait', async () => {
      const started = await start({ workspace_trusted: false })
      const { engine, platform } = started
      await waitFor(() => expect(sent(engine, 'trust_preview')).toHaveLength(1))
      await emit(engine, preview(['lint']), { type: 'error', message: 'The gateway could not be reached.' })
      fireEvent.click(primary())
      expect(off()).toEqual([true, true])
      // Something else the engine says meanwhile changes nothing either.
      await emit(engine, engine.status({ workspace_trusted: false }), { type: 'notice', message: 'Checking the folder.' } as UiEvent)
      expect(off()).toEqual([true, true])
      await emit(engine, { type: 'workspace_trusted', cwd: '/work/app', restart_required: true })
      await waitFor(() => expect(platform.restarts).toEqual([null]))
    })
  })

  // Trusted before, and what the folder asks for changed since: the dialog says so in the client's own
  // words (`trustCopy.changed`, then `trustChangedKeysLine`), above what the folder turns on.
  describe('the trust dialog says why a folder is asked again', () => {
    const CHANGED = { changed_since_trust: true, changed_settings: ['hooks', 'env'] }
    /** The dialog for a preview made of `over` (a hook, unless it says otherwise); `null` is no preview yet. */
    const element = (over: Record<string, unknown> | null) => (
      <TrustModal
        folder="/work/app"
        preview={over === null ? null : (preview(['lint'], over) as TrustPreviewEvent)}
        working={false}
        canPickFolder
        onAllow={() => {}}
        onElsewhere={() => {}}
        onOpenLink={() => {}}
      />
    )
    const modal = (over: Record<string, unknown> | null) => render(element(over))
    const follows = (first: Element, second: Element) => (first.compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0

    test("shows the client's wording above what the folder turns on: why, then which settings", () => {
      modal(CHANGED)
      const dialog = trustDialog()
      const why = within(dialog).getByText(trustCopy.changed)
      const keysLine = trustChangedKeysLine(CHANGED)
      const keys = within(dialog).getByText(keysLine)
      expect(keysLine).toContain('hooks, env')
      const heading = within(dialog).getByText(trustCopy.turnsOnHeading)
      const list = within(dialog).getByRole('list', { name: 'What this folder turns on' })
      expect(follows(why, keys)).toBe(true)
      expect(follows(keys, heading)).toBe(true)
      expect(follows(heading, list)).toBe(true)
    })

    test('and above the note that the folder turns nothing on', () => {
      modal({ ...CHANGED, hooks: [] })
      const dialog = trustDialog()
      const why = within(dialog).getByText(trustCopy.changed)
      expect(follows(why, within(dialog).getByText(trustCopy.turnsOnNothing))).toBe(true)
    })

    test('is absent unless the preview says the folder changed since it was trusted', () => {
      for (const over of [null, {}, { changed_since_trust: false }, { changed_since_trust: false, changed_settings: ['hooks'] }, { changed_settings: ['hooks'] }]) {
        modal(over)
        const dialog = trustDialog()
        expect(dialog.textContent).not.toContain(trustCopy.changed)
        expect(dialog.textContent).not.toContain(trustCopy.changedKeys)
        expect(dialog.getAttribute('aria-describedby')).toBe('trust-access')
        cleanup()
      }
    })

    test('says so without a settings line when the engine names no keys', () => {
      for (const over of [{ changed_since_trust: true }, { changed_since_trust: true, changed_settings: [] }]) {
        modal(over)
        const dialog = trustDialog()
        expect(within(dialog).getByText(trustCopy.changed)).toBeTruthy()
        expect(dialog.textContent).not.toContain(trustCopy.changedKeys)
        cleanup()
      }
    })

    test('shows the keys as text, however they are spelled', () => {
      modal({ changed_since_trust: true, changed_settings: ['<b>env</b>', 'a&b'] })
      const dialog = trustDialog()
      expect(dialog.querySelectorAll('b').length).toBe(0)
      expect(dialog.textContent).toContain('<b>env</b>, a&b')
    })

    test('describes the dialog too, so it is read out with the folder access', () => {
      modal(CHANGED)
      const ids = (trustDialog().getAttribute('aria-describedby') ?? '').split(' ')
      expect(ids).toEqual(['trust-access', 'trust-changed'])
      expect(ids.map((id) => document.getElementById(id)?.textContent)).toEqual([trustCopy.access, trustCopy.changed])
    })

    test('leaves nothing for axe to find', async () => {
      modal(CHANGED)
      await expectAccessible()
    })

    // The dialog mounts with no preview: the reducer clears it on `ready` and on `workspace_trusted`, and the
    // engine's answer, with the reason the folder is asked again, comes later. A screen reader reads the dialog
    // as it opens and announces what is added to a live region that was already there, not one added with its text.
    describe('for a screen reader', () => {
      const region = () => trustDialog().querySelector<HTMLElement>('[aria-live]')

      test('what the preview makes is a polite live region, there from the start and empty until the preview arrives', () => {
        const view = modal(null)
        const before = region()
        expect(before?.getAttribute('aria-live')).toBe('polite')
        expect(before?.textContent).toBe('')

        view.rerender(element(CHANGED))
        const after = region()
        // The same element, so the arrival is an addition to it and is announced.
        expect(after === before).toBe(true)
        expect(after?.textContent).toContain(trustCopy.changed)
        expect(after?.textContent).toContain(trustChangedKeysLine(CHANGED))
        expect(after?.textContent).toContain(trustCopy.turnsOnHeading)
        expect(after?.textContent).toContain('1 hook that runs commands around tool calls')
        // Only what the preview makes: what the dialog says on its own is read as it opens.
        expect(after?.textContent).not.toContain(trustCopy.access)
        expect(after?.textContent).not.toContain(trustCopy.caution)
      })

      test('it stays that element when a fresh preview replaces the first, and holds the note that nothing is turned on', () => {
        const view = modal({})
        const first = region()
        expect(first?.textContent).toContain('1 hook that runs commands around tool calls')
        view.rerender(element({ hooks: [], session_only: true }))
        expect(region() === first).toBe(true)
        expect(first?.textContent).toContain(trustCopy.turnsOnNothing)
        expect(first?.textContent).toContain(trustCopy.sessionOnly)
        expect(first?.textContent).not.toContain('1 hook that runs')
      })

      test('the dialog is described only by elements that exist, with the reason or without it', () => {
        for (const over of [null, {}, CHANGED]) {
          modal(over)
          const ids = (trustDialog().getAttribute('aria-describedby') ?? '').split(' ')
          expect(ids.length).toBeGreaterThan(0)
          expect(ids.map((id) => document.getElementById(id) !== null)).toEqual(ids.map(() => true))
          cleanup()
        }
      })

      test('in the app, the dialog opens with the region empty and the engine\'s preview arrives in it', async () => {
        const { engine } = await start({ workspace_trusted: false })
        const before = region()
        expect(before?.textContent).toBe('')
        await waitFor(() => expect(sent(engine, 'trust_preview')).toHaveLength(1))
        await emit(engine, preview(['lint'], CHANGED))
        expect(region() === before).toBe(true)
        expect(before?.textContent).toContain(trustCopy.changed)
        expect(before?.textContent).toContain(trustChangedKeysLine(CHANGED))
      })
    })

    test('in the app, a folder that asks again says what changed, and the first time it does not', async () => {
      const { engine } = await start({ workspace_trusted: false })
      await waitFor(() => expect(sent(engine, 'trust_preview')).toHaveLength(1))
      await emit(engine, preview(['lint']))
      expect(trustDialog().textContent).not.toContain(trustCopy.changed)

      await emit(engine, engine.status({ workspace_trusted: true }))
      await emit(engine, engine.status({ workspace_trusted: false }))
      await waitFor(() => expect(sent(engine, 'trust_preview')).toHaveLength(2))
      await emit(engine, preview(['lint', 'deploy'], CHANGED))
      const dialog = trustDialog()
      expect(within(dialog).getByText(trustCopy.changed)).toBeTruthy()
      expect(within(dialog).getByText(trustChangedKeysLine(CHANGED))).toBeTruthy()
      expect(listed()).toEqual(['2 hooks that run commands around tool calls'])
      await expectAccessible()
    })
  })
})
