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
import { ExtensionsPanel } from '../src/components/ExtensionsPanel'
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
    // Not disabled, so keyboard focus stays; clicks are ignored while it waits.
    expect(pending.disabled).toBe(false)
    expect(pending.getAttribute('aria-disabled')).toBe('true')
    expect(pending.getAttribute('aria-busy')).toBe('true')
    fireEvent.click(pending)
    expect(sent(engine, 'set_mcp_server_enabled').length).toBe(1)
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
  }, 30_000)

  test('a switch that cannot be sent (engine down) shows the reason instead of hanging', async () => {
    const data = { mcpServers: [{ name: 'docs', scope: 'user', transport: 'stdio', status: 'ready', enabled: true, tools: [], error: null }], plugins: [], skills: [], agents: [] }
    for (const failure of [
      () => {
        throw new Error('The engine is not running.')
      },
      () => Promise.reject(new Error('The engine is not running.')),
    ]) {
      const view = render(<ExtensionsPanel extensions={data as never} onRefresh={() => {}} onSetEnabled={failure} />)
      fireEvent.click(screen.getByRole('switch', { name: 'docs' }))
      expect((await screen.findByRole('alert')).textContent).toContain('The engine is not running.')
      const again = screen.getByRole('switch', { name: 'docs' })
      expect(again.getAttribute('aria-disabled')).toBeNull()
      expect(again.getAttribute('aria-busy')).toBeNull()
      view.unmount()
    }
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

/**
 * What the engine sends from 7.0.0-alpha.5 (engine.rs's `extensions_event`): servers that wait for
 * approval, are refused by a deny rule or the connector policy, or are hosted connectors not linked
 * yet; `required` on the ones the policy keeps on; transport `zerox` (older engines: `managed_proxy`).
 */
describe('MCP rows on an alpha.5 engine', () => {
  // The reasons are the engine's own wording (a blocked server's is the policy's).
  const APPROVAL_WHY = 'a workspace server: run /mcp approve docs to start it'
  const REFUSED_WHY = "your organization's connector policy does not allow shell-tools"
  const UNLINKED_WHY = 'not linked to your Zero-X account; connect it at https://zero-x.live/connect/github (/mcp connect github)'
  // A reason that does not say how to approve (an older engine's, or a policy's), which the hint is for.
  const WAITING_WHY = 'a workspace server that is new or changed'
  const approval = server('docs', { scope: 'project', status: 'needs_approval', tools: [], error: APPROVAL_WHY })
  const waiting = server('cache', { scope: 'project', status: 'needs_approval', tools: [], error: WAITING_WHY })
  const refused = server('shell-tools', { scope: 'managed', status: 'blocked', tools: [], error: REFUSED_WHY })
  const unlinked = server('github', {
    transport: 'zerox',
    status: 'needs_connect',
    tools: [],
    error: UNLINKED_WHY,
    connect_url: 'https://zero-x.live/connect/github',
  })

  const panel = (servers: Array<Record<string, unknown>>, onSetEnabled?: (name: string, enabled: boolean) => void) =>
    render(<ExtensionsPanel extensions={{ mcpServers: servers, plugins: [], skills: [], agents: [] } as never} onRefresh={() => {}} onSetEnabled={onSetEnabled} />)
  const rowOf = (name: string) => screen.getByText(name, { selector: '.extension-name' }).closest('li') as HTMLElement
  const statusOf = (name: string) => within(rowOf(name)).getByText(/./, { selector: '.extension-status' }).textContent

  test('the new statuses read as words, not as their wire names', () => {
    panel([approval, refused, unlinked])
    expect(statusOf('docs')).toBe('needs approval')
    expect(statusOf('shell-tools')).toBe('blocked')
    expect(statusOf('github')).toBe('not connected')
    for (const wire of ['needs_approval', 'needs_connect']) expect(document.body.textContent).not.toContain(wire)
  })

  test('a status this build does not know is shown as the engine sent it', () => {
    panel([server('later', { status: 'quarantined', tools: [] })])
    expect(statusOf('later')).toBe('quarantined')
  })

  test("the engine's reason is the row's detail, visible without a click and shown once", () => {
    panel([approval, refused, unlinked])
    for (const [name, reason] of [
      ['docs', APPROVAL_WHY],
      ['shell-tools', REFUSED_WHY],
      ['github', UNLINKED_WHY],
    ]) {
      expect(within(rowOf(name)).getByText(reason)).toBeTruthy()
      // The head has nothing more to show, so a click adds nothing and hides nothing.
      fireEvent.click(within(rowOf(name)).getAllByRole('button')[0]!)
      expect(within(rowOf(name)).getAllByText(reason)).toHaveLength(1)
    }
  })

  test('a refused server reads as an error; one that only waits does not', () => {
    panel([approval, refused, unlinked])
    expect(within(rowOf('shell-tools')).getByText(REFUSED_WHY).className).toContain('error-line')
    expect(within(rowOf('docs')).getByText(APPROVAL_WHY).className).not.toContain('error-line')
    expect(within(rowOf('github')).getByText(UNLINKED_WHY).className).not.toContain('error-line')
  })

  test('a failed server still shows its error only when its row is opened', () => {
    panel([server('ghost', { status: 'failed', tools: [], error: 'spawn failed: not found' })])
    expect(screen.queryAllByText('spawn failed: not found').length).toBe(0)
    fireEvent.click(within(rowOf('ghost')).getAllByRole('button')[0]!)
    expect(screen.getByText('spawn failed: not found')).toBeTruthy()
  })

  test('a status without a reason shows no detail line', () => {
    panel([server('docs', { status: 'blocked', tools: [], error: null })])
    expect(statusOf('docs')).toBe('blocked')
    expect(rowOf('docs').querySelectorAll('.extension-detail').length).toBe(0)
  })

  test('a server that needs approval says how to approve it, as a quiet hint with no command of its own', () => {
    const calls: string[] = []
    panel([approval, waiting, refused, unlinked, server('ok')], (name, enabled) => calls.push(`${name}:${enabled}`))
    const hint = within(rowOf('cache')).getByText('Approve it with /mcp approve cache in the chat')
    expect(hint.className).toContain('muted')
    // Text only: no button or link to press, so nothing new is sent to the engine.
    expect(hint.closest('button, a')?.tagName ?? null).toBeNull()
    fireEvent.click(hint)
    expect(calls).toEqual([])
    expect(within(rowOf('cache')).queryAllByRole('button', { name: /approve/i }).length).toBe(0)
    // Only that row gets it, and it names its own server: not the one whose reason says it, nor other statuses.
    expect(screen.getAllByText(/Approve it with/)).toHaveLength(1)
    for (const name of ['docs', 'shell-tools', 'github', 'ok']) expect(within(rowOf(name)).queryAllByText(/Approve it with/).length).toBe(0)
  })

  test('the hint names the server it is about', () => {
    panel([server('team_db-2', { status: 'needs_approval', tools: [], error: null })])
    expect(screen.getByText('Approve it with /mcp approve team_db-2 in the chat')).toBeTruthy()
  })

  test("the hint is left out when the engine's own reason already says how to approve", () => {
    panel([approval])
    // The engine's reason has the command, once; the hint would say it a second time.
    expect(within(rowOf('docs')).getByText(APPROVAL_WHY)).toBeTruthy()
    expect(screen.queryAllByText(/Approve it with/).length).toBe(0)
    expect((rowOf('docs').textContent?.match(/\/mcp approve/g) ?? []).length).toBe(1)
    cleanup()
    // Whichever server it names, and wherever in the reason it is.
    panel([server('docs', { status: 'needs_approval', tools: [], error: 'run /mcp approve to start it' })])
    expect(screen.queryAllByText(/Approve it with/).length).toBe(0)
  })

  test('the hint stays when the reason does not say how to approve', () => {
    for (const error of [WAITING_WHY, null, '', 'see /mcp list for the servers']) {
      panel([server('docs', { status: 'needs_approval', tools: [], error })])
      expect(within(rowOf('docs')).getAllByText('Approve it with /mcp approve docs in the chat')).toHaveLength(1)
      cleanup()
    }
  })

  test('a server that waits for approval but was turned off reads off, with no reason and no hint', () => {
    // Whether or not its reason says how to approve: it is the switch that hides the hint.
    for (const [row, why] of [
      [waiting, WAITING_WHY],
      [approval, APPROVAL_WHY],
    ] as const) {
      panel([{ ...row, enabled: false }])
      expect(statusOf(row.name)).toBe('off')
      expect(screen.queryAllByText(/Approve it with/).length).toBe(0)
      expect(screen.queryAllByText(why).length).toBe(0)
      cleanup()
    }
  })

  test('a hosted connector is "hosted", whichever name the engine gives its transport', () => {
    panel([
      server('new-engine', { transport: 'zerox', scope: 'managed' }),
      server('old-engine', { transport: 'managed_proxy', scope: 'user' }),
      server('local', { transport: 'stdio' }),
      server('remote', { transport: 'http' }),
    ])
    const meta = (name: string) => rowOf(name).querySelector('.extension-head .muted')?.textContent
    expect(meta('new-engine')).toBe('managed · hosted · 1 tool')
    expect(meta('old-engine')).toBe('user · hosted · 1 tool')
    expect(meta('local')).toBe('user · stdio · 1 tool')
    expect(meta('remote')).toBe('user · http · 1 tool')
    for (const wire of ['zerox', 'managed_proxy']) expect(document.body.textContent).not.toContain(wire)
  })

  test('a required server has a locked "Required" marker and no On/Off switch, and the marker does nothing', () => {
    const calls: string[] = []
    panel([server('policy-docs', { scope: 'managed', transport: 'zerox', required: true }), server('docs'), server('plain', { required: false })], (name, enabled) =>
      calls.push(`${name}:${enabled}`),
    )
    expect(screen.queryAllByRole('switch', { name: 'policy-docs' }).length).toBe(0)
    expect(within(rowOf('policy-docs')).queryAllByText(/^(On|Off)$/).length).toBe(0)
    // The others keep theirs, and only the required row is marked.
    expect(screen.getByRole('switch', { name: 'docs' })).toBeTruthy()
    expect(screen.getByRole('switch', { name: 'plain' })).toBeTruthy()
    expect(screen.getAllByText('Required')).toHaveLength(1)

    const marker = within(rowOf('policy-docs')).getByText('Required')
    expect(marker.getAttribute('title')).toMatch(/policy/i)
    // Not a control: nothing to press, focus or switch.
    expect(marker.closest('button, a, [role="switch"], [tabindex]')?.tagName ?? null).toBeNull()
    fireEvent.click(marker)
    fireEvent.keyDown(marker, { key: 'Enter' })
    expect(calls).toEqual([])
    // It still reads as the row's own status and count.
    expect(statusOf('policy-docs')).toBe('ready')
  })

  test('a required server is marked even when the engine has no switches', () => {
    panel([server('policy-docs', { required: true }), server('docs')])
    expect(screen.queryAllByRole('switch').length).toBe(0)
    expect(within(rowOf('policy-docs')).getByText('Required')).toBeTruthy()
    expect(within(rowOf('docs')).queryAllByText('Required').length).toBe(0)
  })

  test('on the Extensions page, from the engine to the row', async () => {
    const { engine } = await start({}, undefined, FEATURES_WITH_TOGGLE)
    openExtensions()
    await emit(
      engine,
      extensions(approval, waiting, refused, unlinked, server('policy-docs', { scope: 'managed', transport: 'zerox', required: true }), server('legacy', { transport: 'managed_proxy' })),
    )
    const rows = within(screen.getByRole('list', { name: 'MCP servers' }))
    expect(rows.getAllByText('needs approval')).toHaveLength(2)
    expect(rows.getByText('blocked')).toBeTruthy()
    expect(rows.getByText('not connected')).toBeTruthy()
    expect(rows.getByText(APPROVAL_WHY)).toBeTruthy()
    expect(rows.getByText(WAITING_WHY)).toBeTruthy()
    // The hint only where the reason does not already say how.
    expect(rows.getByText('Approve it with /mcp approve cache in the chat')).toBeTruthy()
    expect(rows.queryAllByText('Approve it with /mcp approve docs in the chat').length).toBe(0)
    expect(rows.getByText('Required')).toBeTruthy()
    expect(rows.queryAllByRole('switch', { name: 'policy-docs' }).length).toBe(0)
    expect(rows.getByRole('switch', { name: 'legacy' })).toBeTruthy()
    expect(rows.getAllByText(/hosted/)).toHaveLength(3)
    // Reading the rows asks nothing new of the engine.
    const kinds = new Set(engine.commands().map((command) => (command as { type: string }).type))
    expect(kinds.has('approve_mcp_server')).toBe(false)
    expect(sent(engine, 'slash')).toEqual([])
    await expectAccessible()
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

  test('a clipboard that refuses says so', async () => {
    render(
      <EngineProblemCard
        problem={engineProblemFor('[engine:failed] x')!}
        actions={{ onCopyDiagnostics: () => Promise.reject(new Error('denied')), onOpenLink: () => {} }}
      />,
    )
    fireEvent.click(screen.getByRole('button', { name: 'Copy diagnostics' }))
    expect(await screen.findByText('Could not copy')).toBeTruthy()
  })

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

  test('Settings says so when the clipboard refuses', async () => {
    Object.defineProperty(navigator, 'clipboard', { value: { writeText: () => Promise.reject(new Error('denied')) }, configurable: true })
    const { engine } = await start({}, { manualUpdates: true })
    await act(async () => {
      for (const listener of engine.closeListeners) listener({ code: 1, error: 'The Neuron engine stopped (exit code 1).' })
    })
    fireEvent.click(within(screen.getByRole('navigation', { name: 'Neuron' })).getByRole('button', { name: 'Settings' }))
    fireEvent.click(screen.getByRole('button', { name: 'Diagnostics' }))
    fireEvent.click(screen.getByRole('button', { name: 'Copy diagnostics' }))
    expect(await screen.findByText('Could not copy')).toBeTruthy()
  })

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

  test('the whole report is scrubbed, the host part included', () => {
    const secrets = ['hunter2', 'user:pass', 'AKIAABCDEFGHIJKLMNOP', 'rahul', 'topsecretvalue', 'abc123tok']
    const text = buildDiagnostics({
      appVersion: '1.0.0',
      hostReport: [
        'Engine path: C:\\Users\\rahul\\AppData\\neuron.exe',
        'Other: /Users/rahul/bin/neuron and /home/rahul/.local/neuron',
        'password: hunter2',
        'Proxy: https://user:pass@proxy.example.com/x',
        'aws=AKIAABCDEFGHIJKLMNOP',
        'api_key=topsecretvalue',
        'Authorization: Bearer abc123tokenvalue',
        'token=abc123tok',
      ].join('\n'),
      engine: { version: '6.3.0', protocol: 2, features: [] },
      connection: 'ready',
      lastError: 'secret: topsecretvalue at /home/rahul/work',
      updates: 'none',
    })
    for (const secret of secrets) expect(text).not.toContain(secret)
    expect(text).toContain('~\\AppData\\neuron.exe')
    expect(text).toContain('~/bin/neuron')
    expect(text).toContain('proxy.example.com/x')
    expect(text).toContain('Engine version: 6.3.0')
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
