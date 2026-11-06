/**
 * L5, extensions and resilience: the MCP on/off switch, which engine runs, problem
 * cards for an engine that is missing, blocked, denied, crashed or did not restart,
 * and the diagnostics report.
 */
import { afterEach, describe, expect, test } from 'bun:test'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import axe from 'axe-core'

import type { ClientMessage, UiEvent } from '@neuron/client'

import { App } from '../src/App'
import { EngineProblemCard } from '../src/components/ProblemCard'
import { buildDiagnostics, engineProblemFor, parseEngineProblem, type EngineProblemCode } from '../src/engineProblems'
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

type Command = Extract<ClientMessage, { kind: 'command' }> & { enabled?: boolean; name?: string }
const sent = (engine: FakeEngine, type: string) =>
  engine.commands().filter((message): message is Command => message.kind === 'command' && message.type === type)

async function expectAccessible(root: Element = document.body) {
  const result = await axe.run(root, { rules: { 'color-contrast': { enabled: false } } })
  const serious = result.violations.filter((violation) => violation.impact === 'serious' || violation.impact === 'critical')
  expect(serious.map((violation) => `${violation.id}: ${violation.nodes.map((node) => node.target.join(' ')).join(', ')}`)).toEqual([])
}

const FEATURES_WITH_TOGGLE = [
  'capabilities',
  'sessions',
  'trust_workspace',
  'todos',
  'subagents',
  'extensions',
  'account',
  'error_details',
  'session_scope',
  'trust_preview',
  'mcp_toggle',
]

const server = (name: string, over: Record<string, unknown> = {}) => ({
  name,
  scope: 'user',
  transport: 'stdio',
  status: 'ready',
  tools: ['lookup'],
  error: null,
  ...over,
})

const extensions = (...servers: Array<Record<string, unknown>>) =>
  ({ type: 'extensions', mcp_servers: servers, plugins: [], skills: [], agents: [] }) as unknown as UiEvent

const openExtensions = () => fireEvent.click(screen.getByRole('button', { name: 'Extensions' }))

describe('MCP on/off', () => {
  test('the switch sends the command and stays pending until the fresh list arrives', async () => {
    const { engine } = await start({}, undefined, FEATURES_WITH_TOGGLE)
    openExtensions()
    await emit(engine, extensions(server('docs'), server('ghost', { status: 'disabled', enabled: false, tools: [] })))
    const docs = screen.getByRole('switch', { name: 'docs' })
    expect(docs.getAttribute('aria-checked')).toBe('true')
    expect(screen.getByRole('switch', { name: 'ghost' }).getAttribute('aria-checked')).toBe('false')
    expect(screen.getByText('off', { selector: '.extension-status' })).toBeTruthy()

    fireEvent.click(docs)
    expect(sent(engine, 'set_mcp_server_enabled')).toEqual([{ kind: 'command', type: 'set_mcp_server_enabled', name: 'docs', enabled: false }])
    const pending = screen.getByRole('switch', { name: 'docs' }) as HTMLButtonElement
    expect(pending.disabled).toBe(true)
    expect(pending.getAttribute('aria-busy')).toBe('true')
    expect(screen.getByText(/Turning off/)).toBeTruthy()
    expect(screen.getByText(/from your next message/)).toBeTruthy()

    await emit(engine, extensions(server('docs', { status: 'disabled', enabled: false, tools: [] }), server('ghost', { status: 'disabled', enabled: false, tools: [] })))
    const after = screen.getByRole('switch', { name: 'docs' }) as HTMLButtonElement
    expect(after.disabled).toBe(false)
    expect(after.getAttribute('aria-checked')).toBe('false')
    expect(after.getAttribute('aria-busy')).toBeNull()

    fireEvent.click(after)
    expect(sent(engine, 'set_mcp_server_enabled').at(-1)).toMatchObject({ name: 'docs', enabled: true })
    await expectAccessible()
  })

  test('a server whose enabled flag is false reads as off even if its status is stale', async () => {
    const { engine } = await start({}, undefined, FEATURES_WITH_TOGGLE)
    openExtensions()
    await emit(engine, extensions(server('docs', { enabled: false })))
    expect(screen.getByRole('switch', { name: 'docs' }).getAttribute('aria-checked')).toBe('false')
  })

  test('without the feature there is no switch', async () => {
    const { engine } = await start()
    openExtensions()
    await emit(engine, extensions(server('docs')))
    expect(screen.getByRole('list', { name: 'MCP servers' })).toBeTruthy()
    expect(screen.queryByRole('switch')).toBeNull()
  })
})

describe('which engine runs', () => {
  const openDiagnostics = () => {
    fireEvent.click(within(screen.getByRole('navigation', { name: 'Neuron' })).getByRole('button', { name: 'Settings' }))
    fireEvent.click(screen.getByRole('button', { name: 'Diagnostics' }))
  }

  test('the bundled engine: its path, version, protocol and features, and no warning', async () => {
    await start()
    openDiagnostics()
    await screen.findByText('/app/neuron')
    expect(screen.getByText('Bundled with Neuron')).toBeTruthy()
    expect(screen.getByText('6.3.0')).toBeTruthy()
    expect(screen.getByText('1', { selector: 'dd' })).toBeTruthy()
    expect(screen.getByText(/mcp_toggle|extensions/, { selector: 'dd' })).toBeTruthy()
    expect(screen.queryByRole('alert')).toBeNull()
    await expectAccessible()
  })

  test('an engine from NEURON_BIN is a warning with its path', async () => {
    await start({}, { engine: { source: 'env', path: 'D:/dev/neuron.exe', bundledPath: '/app/neuron', bundledPresent: true } })
    openDiagnostics()
    const warning = await screen.findByRole('alert')
    expect(warning.textContent).toContain('Using the engine from NEURON_BIN: D:/dev/neuron.exe')
    await expectAccessible()
  })

  test('an engine found on the PATH is a warning too; a browser build shows none', async () => {
    await start({}, { engine: { source: 'path', path: '/usr/bin/neuron', bundledPath: '/app/neuron', bundledPresent: false } })
    openDiagnostics()
    expect((await screen.findByRole('alert')).textContent).toContain('Using the engine found on your PATH: /usr/bin/neuron')
    cleanup()
    await start({}, { engine: null })
    openDiagnostics()
    expect(screen.queryByRole('alert')).toBeNull()
    expect(screen.getByText(/runs on test/)).toBeTruthy()
  })
})

const CASES: Array<[EngineProblemCode, string, string]> = [
  ['missing', '[engine:missing] The Neuron engine was not found. Reinstall Neuron.', "Neuron's engine is missing"],
  [
    'quarantined',
    '[engine:quarantined] Windows security blocked the Neuron engine. Restore it from Protection history, or reinstall Neuron.',
    'Security software blocked the engine',
  ],
  ['denied', '[engine:denied] The system would not let Neuron run the engine (permission denied).', 'The engine is not allowed to run'],
  [
    'crashed',
    '[engine:crashed] The Neuron engine stopped unexpectedly (exit code 3). Last message: out of memory',
    'The engine stopped unexpectedly',
  ],
  ['failed', '[engine:failed] Could not start the Neuron engine: invalid input', 'The engine could not start'],
]

describe('engine problems', () => {
  test('the prefix is parsed; plain errors are not problems', () => {
    expect(parseEngineProblem('[engine:crashed] The Neuron engine stopped unexpectedly (exit code 3). Last message: out of memory')).toEqual({
      code: 'crashed',
      message: 'The Neuron engine stopped unexpectedly (exit code 3). Last message: out of memory',
      exitCode: '3',
      lastMessage: 'out of memory',
    })
    expect(parseEngineProblem('[engine:quarantined] Windows security blocked it.')?.code).toBe('quarantined')
    expect(parseEngineProblem('The Neuron engine stopped (exit code 1).')).toBeNull()
    expect(parseEngineProblem('[engine:nonsense] x')).toBeNull()
    expect(parseEngineProblem(null)).toBeNull()
    expect(parseEngineProblem('')).toBeNull()
  })

  test('a failed restart is its own problem, unless the host named a better reason', () => {
    expect(engineProblemFor('Could not start x', { restart: true })).toMatchObject({ code: 'restart_failed' })
    expect(engineProblemFor('[engine:failed] Could not start', { restart: true })).toMatchObject({ code: 'restart_failed' })
    expect(engineProblemFor('[engine:quarantined] blocked', { restart: true })).toMatchObject({ code: 'quarantined' })
    expect(engineProblemFor('Could not start x')).toBeNull()
  })

  for (const [code, raw, title] of CASES) {
    test(`a card for ${code}: what happened and what to do`, async () => {
      const problem = parseEngineProblem(raw)!
      const calls: string[] = []
      render(
        <EngineProblemCard
          problem={problem}
          actions={{ onRestart: () => calls.push('restart'), onCopyDiagnostics: async () => void calls.push('copy'), onOpenLink: (url) => calls.push(url) }}
        />,
      )
      const card = screen.getByRole('alert', { name: title })
      expect(card.className).toContain(`problem-${code}`)
      if (code !== 'crashed') expect(card.textContent).toContain(problem.message.slice(0, 30))
      if (code === 'crashed') {
        expect(card.textContent).toContain('exit code 3')
        expect(card.textContent).toContain('out of memory')
      }
      fireEvent.click(within(card).getByRole('button', { name: 'Try again' }))
      fireEvent.click(within(card).getByRole('button', { name: 'Copy diagnostics' }))
      fireEvent.click(within(card).getByRole('button', { name: /Open help/ }))
      await waitFor(() => expect(calls).toEqual(['restart', 'copy', LINKS.help]))
      expect(await within(card).findByText('Copied')).toBeTruthy()
      await expectAccessible()
    })
  }

  test('a restart that failed names the restart', async () => {
    render(
      <EngineProblemCard
        problem={engineProblemFor('Could not start the Neuron engine: boom', { restart: true })!}
        actions={{ onCopyDiagnostics: async () => {}, onOpenLink: () => {} }}
      />,
    )
    const card = screen.getByRole('alert', { name: 'The engine did not restart' })
    expect(card.textContent).toContain('boom')
    // No restart handler: no "Try again".
    expect(within(card).queryByRole('button', { name: 'Try again' })).toBeNull()
    await expectAccessible()
  })

  test('an engine that exits with a coded error shows the card in the session, and Try again restarts it', async () => {
    const { engine, platform } = await start()
    await act(async () => {
      for (const listener of engine.closeListeners)
        listener({ code: 3, error: '[engine:crashed] The Neuron engine stopped unexpectedly (exit code 3). Last message: out of memory' })
    })
    const card = screen.getByRole('alert', { name: 'The engine stopped unexpectedly' })
    expect(card.textContent).toContain('out of memory')
    fireEvent.click(within(card).getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(platform.restarts).toEqual(['session-1']))
  })

  test('a restart that fails shows the card', async () => {
    const { engine, platform } = await start()
    platform.restart = async () => {
      throw new Error('[engine:quarantined] Windows security blocked the Neuron engine. Restore it from Protection history, or reinstall Neuron.')
    }
    await act(async () => {
      for (const listener of engine.closeListeners) listener({ code: 1, error: 'The Neuron engine stopped (exit code 1).' })
    })
    fireEvent.click(screen.getByRole('button', { name: 'Restart it' }))
    expect(await screen.findByRole('alert', { name: 'Security software blocked the engine' })).toBeTruthy()
  })
})

describe('copy diagnostics', () => {
  const secrets = ['dev@example.com', 'rahul@zero-x.live', 'Rahul S', 'sk-live-abcdef1234567890', 'acct_12345']

  test('the report has the versions, engine, connection and update mode, and no account data', async () => {
    const copied: string[] = []
    Object.defineProperty(navigator, 'clipboard', { value: { writeText: async (text: string) => void copied.push(text) }, configurable: true })
    const { engine } = await start({ account: { email: 'dev@example.com', plan: 'pro', source: 'keychain', expires_at: null } }, { manualUpdates: true })
    await emit(engine, {
      type: 'account',
      signed_in: true,
      account: {
        email: 'rahul@zero-x.live',
        name: 'Rahul S',
        plan: 'pro',
        plan_name: 'Pro',
        source: 'device',
        expires_at: null,
        limits: null,
        usage_today: null,
        resets_at: null,
        credit_balance_cents: null,
        suspended: false,
        dashboard_url: null,
        pricing_url: null,
        usage_updated_at: null,
        usage_error: null,
      },
    } as unknown as UiEvent)
    await act(async () => {
      for (const listener of engine.closeListeners)
        listener({ code: 1, error: 'The Neuron engine stopped (exit code 1) for dev@example.com with sk-live-abcdef1234567890 on acct_12345' })
    })
    fireEvent.click(within(screen.getByRole('navigation', { name: 'Neuron' })).getByRole('button', { name: 'Settings' }))
    fireEvent.click(screen.getByRole('button', { name: 'Diagnostics' }))
    fireEvent.click(screen.getByRole('button', { name: 'Copy diagnostics' }))
    await waitFor(() => expect(copied).toHaveLength(1))
    const text = copied[0]
    expect(text).toContain('Neuron Desktop 0.2.0')
    expect(text).toContain('App version: 0.2.0')
    expect(text).toContain('OS: windows x86_64')
    expect(text).toContain('Engine version: 6.3.0')
    expect(text).toContain('Engine protocol: 1')
    expect(text).toContain('Engine features: ')
    expect(text).toContain('Connection: exited')
    expect(text).toContain('Last engine error: The Neuron engine stopped (exit code 1)')
    expect(text).toContain('Updates: manual')
    for (const secret of secrets) expect(text).not.toContain(secret)
    expect(await screen.findByText('Copied')).toBeTruthy()
  })

  test('building the report scrubs emails and token-like strings and never takes an account', () => {
    const text = buildDiagnostics({
      appVersion: '1.0.0',
      hostReport: 'App version: 1.0.0',
      engine: { version: '6.3.0', protocol: 2, features: ['a', 'b'] },
      connection: 'ready',
      lastError: 'Bearer abcdefghijklmnop failed for me@x.io; key=sk-abcdefghijklmnop',
      updates: 'signed',
    })
    expect(text).toContain('Engine features: a, b')
    expect(text).toContain('Updates: signed')
    for (const bad of ['me@x.io', 'abcdefghijklmnop']) expect(text).not.toContain(bad)
    // Without a host report (a browser) it still says what it knows.
    expect(buildDiagnostics({ appVersion: null, hostReport: null, engine: { version: null, protocol: null, features: [] }, connection: 'connecting', lastError: null, updates: 'none' })).toContain(
      'Last engine error: none',
    )
  })
})
