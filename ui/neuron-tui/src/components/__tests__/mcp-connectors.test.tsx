import fs from 'fs'
import path from 'path'

import { afterEach, beforeAll, beforeEach, describe, expect, spyOn, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import {
  addConnector,
  approveMcpServer,
  handleMcpPanelEvent,
  removeMcpServer,
  requestExtensions,
} from '../../engine/mcp-panel'
import {
  setNeuronEngineClientForTesting,
  type NeuronEngineClient,
} from '../../engine/neuron-engine-client'
import {
  FEATURE_MCP_CATALOG,
  FEATURE_MCP_TOGGLE,
  parseUiEventLine,
  type ConnectorCatalogEvent,
  type EngineCommand,
  type ExtensionsEvent,
  type McpServerInfo,
} from '../../engine/protocol'
import { initializeThemeStore, useThemeStore } from '../../hooks/use-theme'
import { useChatStore } from '../../state/chat-store'
import { useEngineStore } from '../../state/engine-store'
import { connectorsChip, useMcpPanelStore } from '../../state/mcp-panel-store'
import { logger } from '../../utils/logger'
import {
  ADD_TAB_START,
  addTabKey,
  filterConnectors,
  maskSecret,
  pasteIntoAddTab,
  type AddTabState,
  type PanelKey,
} from '../../utils/mcp-add-form'
import { mcpToolCallLabel, mcpToolTitle } from '../../utils/mcp-tool-label'
import { statusBarLine, statusLineText } from '../../utils/status-bar-parts'
import { STATUS_BAR_STATUS } from '../../testing/status-bar-fixtures'
import { approveTrustedMcpServers, trustWorkspace } from '../../engine/engine-session'
import { useStartGate } from '../../state/start-gate'
import { McpPanel, installedHints, resolveMcpKey, serverMark } from '../mcp-panel'
import { trustScreenKeyAction } from '../trust-screen'
import { ToolBranch } from '../blocks/tool-branch'
import { createMarkdownPalette } from '../../utils/theme-system'

const SECRET = 'example-secret-value-123'

/** The fake engine's samples (rust/crates/neuron-tui/src/fake_engine.rs). */
function fixtureEvent<T extends string>(type: T) {
  const file = path.resolve(
    import.meta.dir,
    '../../../../../rust/crates/neuron-tui/fixtures/protocol-v2-events.jsonl',
  )
  for (const line of fs.readFileSync(file, 'utf8').split(/\r?\n/).filter(Boolean)) {
    const parsed = parseUiEventLine(line)
    if (parsed.ok && parsed.event.type === type) return parsed.event
  }
  throw new Error(`no ${type} sample`)
}

const EXTENSIONS = fixtureEvent('extensions') as ExtensionsEvent
const CATALOG = fixtureEvent('connector_catalog') as ConnectorCatalogEvent
const byName = (name: string) => EXTENSIONS.mcp_servers.find((server) => server.name === name)!

/** A ready engine that records what is sent. */
function recordingEngine(features: string[] = [FEATURE_MCP_TOGGLE, FEATURE_MCP_CATALOG]) {
  const sent: EngineCommand[] = []
  const client = { connection: 'ready', send: (command: EngineCommand) => void sent.push(command) }
  setNeuronEngineClientForTesting(client as unknown as NeuronEngineClient)
  useEngineStore.getState().setReady('test', features)
  return sent
}

const key = (name: string, extra: Partial<PanelKey> = {}): PanelKey => ({
  name,
  sequence: name.length === 1 ? name : undefined,
  ...extra,
})

function type(state: AddTabState, text: string): AddTabState {
  let next = state
  for (const char of text) next = addTabKey(next, key(char, { sequence: char }), CATALOG.connectors).state
  return next
}

beforeAll(() => {
  initializeThemeStore()
})

beforeEach(() => {
  useMcpPanelStore.getState().reset()
  useEngineStore.getState().reset()
  useChatStore.getState().setMessages([])
})

afterEach(() => {
  setNeuronEngineClientForTesting(null)
  useEngineStore.getState().reset()
})

describe('the connectors chip', () => {
  test('counts the servers that are up of those turned on, from the extensions sample', () => {
    const chip = connectorsChip(EXTENSIONS.mcp_servers)
    // github and codebase-memory are up; docs is off; the rest need the user.
    expect(chip).toEqual({ ready: 2, total: 5, warning: true, locked: true })
    expect(connectorsChip(null)).toBeNull()
    expect(connectorsChip([{ ...byName('docs') }])).toBeNull()
    expect(connectorsChip([byName('github')])).toEqual({
      ready: 1,
      total: 1,
      warning: false,
      locked: false,
    })
  })

  test('the status bar draws it amber with a lock, and it gives way when narrow', () => {
    const input = {
      status: STATUS_BAR_STATUS,
      account: null,
      models: null,
      mode: 'unicode' as const,
      connectors: connectorsChip(EXTENSIONS.mcp_servers),
    }
    const wide = statusBarLine(input, 200)!
    const segment = wide.left.find((part) => part.target === 'connectors')!
    expect(segment.spans).toEqual([
      { text: '⧉ 2/5', tone: 'warning' },
      { text: ' 🔒', tone: 'muted' },
      { text: ' mcp', tone: 'muted' },
    ])
    // Between the permission mode and the context meter.
    const targets = wide.left.map((part) => part.target)
    expect(targets.indexOf('connectors')).toBe(targets.indexOf('permission') + 1)
    const healthy = statusBarLine({ ...input, connectors: connectorsChip([byName('github')]) }, 200)!
    expect(healthy.left.find((part) => part.target === 'connectors')!.spans[0]).toEqual({
      text: '⧉ 1/1',
      tone: 'muted',
    })
    const console = statusBarLine({ ...input, mode: 'console' }, 200)!
    expect(statusLineText(console, 200)).toContain('≡ 2/5 § mcp')
    const narrow = statusBarLine(input, 60)!
    expect(narrow.left.map((part) => part.target)).not.toContain('connectors')
  })

  test('extensions are kept for the chip with the panel closed, and listed while open', () => {
    expect(handleMcpPanelEvent(EXTENSIONS)).toBe(true)
    expect(useMcpPanelStore.getState().known).toEqual(EXTENSIONS.mcp_servers)
    expect(useMcpPanelStore.getState().servers).toBeNull()
    // The catalog only matters to an open panel.
    expect(handleMcpPanelEvent(CATALOG)).toBe(false)
    useMcpPanelStore.getState().openPanel('add')
    expect(handleMcpPanelEvent(CATALOG)).toBe(true)
    expect(useMcpPanelStore.getState().catalog?.connectors).toEqual(CATALOG.connectors)
  })

  test('a ready engine with extensions is asked for them', () => {
    const sent: EngineCommand[] = []
    const engine = { send: (command: EngineCommand) => void sent.push(command) }
    expect(requestExtensions(engine, { features: ['extensions'] })).toBe(true)
    expect(requestExtensions(engine, { features: [] })).toBe(false)
    expect(sent).toEqual([{ type: 'list_extensions' }])
  })
})

describe('the Installed tab', () => {
  test('keys: approve, sign in, remove and the tab switch', () => {
    const plain = { ctrl: false, meta: false }
    expect(resolveMcpKey({ name: 'a', ...plain })).toEqual({ type: 'approve' })
    expect(resolveMcpKey({ name: 'l', ...plain })).toEqual({ type: 'sign-in' })
    expect(resolveMcpKey({ name: 'x', ...plain })).toEqual({ type: 'remove' })
    expect(resolveMcpKey({ name: 'tab', ...plain })).toEqual({ type: 'tab' })
    expect(resolveMcpKey({ name: 'z', ...plain })).toEqual({ type: 'none' })
  })

  test('a required server is locked: its mark is a lock and its switch is not offered', () => {
    const required = byName('codebase-memory')
    // Padded to the five columns of `[off]`, the lock taking two.
    expect(serverMark(required, null, 'unicode')).toBe('[🔒] ')
    expect(serverMark(required, null, 'console')).toBe('[req]')
    expect(serverMark(byName('docs'), null, 'unicode')).toBe('[off]')
    expect(installedHints(required, false, true)).not.toContain('space on/off')
    expect(installedHints(required, false, true)).not.toContain('x remove')
    expect(installedHints(byName('github'), false, true)).toContain('space on/off')
  })

  test('each row offers what it needs', () => {
    expect(installedHints(byName('postgres-tools'), false, true)).toContain('a approve')
    expect(installedHints(byName('linear'), false, true)).toContain('c connect')
    expect(installedHints(byName('github'), false, true)).not.toContain('a approve')
    expect(installedHints(byName('github'), true, true)).toBe(
      'x again removes github · any other key keeps it',
    )
    const signIn: McpServerInfo = {
      ...byName('github'),
      status: 'failed',
      error: 'needs a sign-in: run `neuron mcp login github`',
    }
    expect(installedHints(signIn, false, true)).toContain('l sign in')
  })

  test('Approve on a needs_approval row sends approve_mcp_server', () => {
    const sent = recordingEngine()
    expect(approveMcpServer(byName('postgres-tools'))).toBe(true)
    expect(sent).toEqual([{ type: 'approve_mcp_server', name: 'postgres-tools' }])
    expect(useMcpPanelStore.getState().pending).toBe('postgres-tools')
    // A ready server is not waiting for anything.
    useMcpPanelStore.getState().setPending(null)
    expect(approveMcpServer(byName('github'))).toBe(false)
    expect(sent).toHaveLength(1)
  })

  test('Remove sends remove_mcp_server with the scope; a required server stays', () => {
    const sent = recordingEngine()
    expect(removeMcpServer(byName('notes'))).toBe(true)
    expect(sent).toEqual([{ type: 'remove_mcp_server', name: 'notes', scope: 'local' }])
    useMcpPanelStore.getState().setPending(null)
    expect(removeMcpServer(byName('codebase-memory'))).toBe(false)
    expect(sent).toHaveLength(1)
  })
})

describe('the Add tab', () => {
  const github = CATALOG.connectors[0]!

  test('typing searches the catalog; Enter opens the form with its defaults', () => {
    expect(filterConnectors(CATALOG.connectors, 'git hub')).toEqual([github])
    expect(filterConnectors(CATALOG.connectors, 'nothing-like-it')).toEqual([])
    let state = type(ADD_TAB_START, 'git')
    expect(state.query).toBe('git')
    state = addTabKey(state, key('backspace'), CATALOG.connectors).state
    expect(state.query).toBe('gi')
    state = addTabKey(state, key('return'), CATALOG.connectors).state
    expect(state).toMatchObject({ stage: 'form', connectorId: 'github', field: 0 })
    expect(state.form.scope).toBe(github.default_scope as AddTabState['form']['scope'])
    // Esc goes back to the search, a second Esc (with no query) closes the panel.
    state = addTabKey(state, key('escape'), CATALOG.connectors).state
    expect(state.stage).toBe('search')
    state = addTabKey(state, key('escape'), CATALOG.connectors).state
    expect(addTabKey({ ...state, query: '' }, key('escape'), CATALOG.connectors).effect).toEqual({
      type: 'close',
    })
  })

  test('fields take typed and pasted text; the scope cycles; secrets are masked', () => {
    let state = addTabKey(ADD_TAB_START, key('return'), CATALOG.connectors).state
    state = pasteIntoAddTab(state, CATALOG.connectors, `${SECRET}\r\n`)
    expect(state.form.values.GITHUB_PERSONAL_ACCESS_TOKEN).toBe(SECRET)
    state = addTabKey(state, key('down'), CATALOG.connectors).state
    state = type(state, 'gh')
    expect(state.form.name).toBe('gh')
    state = addTabKey(state, key('down'), CATALOG.connectors).state
    state = addTabKey(state, key('right'), CATALOG.connectors).state
    expect(state.form.scope).toBe('project')
    state = addTabKey(state, key('left'), CATALOG.connectors).state
    state = addTabKey(state, key('left'), CATALOG.connectors).state
    expect(state.form.scope).toBe('local')
    expect(maskSecret(SECRET)).toBe('•'.repeat(SECRET.length))
    expect(maskSecret('x'.repeat(40), '*')).toBe(`${'*'.repeat(32)}…`)
    expect(maskSecret('')).toBe('')

    const { effect } = addTabKey(state, key('return'), CATALOG.connectors)
    expect(effect).toEqual({
      type: 'add',
      connector: github,
      form: { name: 'gh', scope: 'local', values: { GITHUB_PERSONAL_ACCESS_TOKEN: SECRET } },
    })
  })

  test('the form is drawn with the secret masked, never in clear', async () => {
    useEngineStore.getState().setReady('test', [FEATURE_MCP_TOGGLE, FEATURE_MCP_CATALOG])
    useMcpPanelStore.getState().openPanel('add')
    useMcpPanelStore.getState().setCatalog({ version: CATALOG.version, connectors: CATALOG.connectors })
    const setup = await createTestRenderer({ width: 100, height: 14, kittyKeyboard: true })
    const root = createRoot(setup.renderer)
    const settle = async () => {
      await setup.renderOnce()
      await new Promise((resolve) => setTimeout(resolve, 20))
      await setup.renderOnce()
    }
    flushSync(() =>
      root.render(<McpPanel servers={EXTENSIONS.mcp_servers} pending={null} width={100} glyphMode="unicode" />),
    )
    try {
      await settle()
      let frame = setup.captureCharFrame()
      expect(frame).toContain('[Add]')
      expect(frame).toContain('Search:')
      expect(frame).toContain('GitHub')
      setup.mockInput.pressEnter()
      await settle()
      await setup.mockInput.pasteBracketedText(SECRET)
      await settle()
      frame = setup.captureCharFrame()
      expect(frame).toContain('Add GitHub (http)')
      expect(frame).toContain('GitHub personal access token*')
      expect(frame).toContain('•'.repeat(SECRET.length))
      expect(frame).not.toContain(SECRET)
      expect(frame).toContain('Get it at https://github.com/settings/personal-access-tokens/new')
    } finally {
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    }
  })

  test('the Installed tab draws a badge for each server that waits for the user', async () => {
    useEngineStore.getState().setReady('test', [FEATURE_MCP_TOGGLE, FEATURE_MCP_CATALOG])
    useMcpPanelStore.getState().openPanel('installed')
    const setup = await createTestRenderer({ width: 140, height: 16 })
    const root = createRoot(setup.renderer)
    flushSync(() =>
      root.render(<McpPanel servers={EXTENSIONS.mcp_servers} pending={null} width={140} glyphMode="console" />),
    )
    try {
      await setup.renderOnce()
      const frame = setup.captureCharFrame()
      const row = (name: string) => frame.split('\n').find((line) => line.includes(`${name}  `)) ?? ''
      expect(row('postgres-tools')).toContain('[needs approval]')
      expect(row('linear')).toContain('[needs connect]')
      expect(row('notes')).toContain('[unpinned]')
      // Said once, as the badge, not again in the summary.
      expect(row('postgres-tools')).not.toContain('needs approval:')
      expect(row('github')).not.toMatch(/\[(blocked|needs|unpinned)/)
    } finally {
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    }
  })

  test('Add sends add_mcp_server with the secrets, and nothing logs them', () => {
    const sent = recordingEngine()
    const logged: unknown[] = []
    const spies = (['debug', 'info', 'warn', 'error', 'fatal'] as const).map((level) =>
      spyOn(logger, level).mockImplementation(((...args: unknown[]) => {
        logged.push(args)
      }) as never),
    )
    try {
      const github = CATALOG.connectors[0]!
      // A required variable left empty is refused, and says which.
      expect(addConnector(github, { name: '', scope: 'user', values: {} })).toBe(false)
      expect(sent).toEqual([])
      expect(JSON.stringify(useChatStore.getState().messages)).toContain(
        'GitHub needs GitHub personal access token',
      )

      expect(
        addConnector(github, {
          name: 'gh',
          scope: 'project',
          values: { GITHUB_PERSONAL_ACCESS_TOKEN: ` ${SECRET} ` },
        }),
      ).toBe(true)
      expect(sent).toEqual([
        {
          type: 'add_mcp_server',
          id: 'github',
          name: 'gh',
          scope: 'project',
          secrets: { GITHUB_PERSONAL_ACCESS_TOKEN: SECRET },
        },
      ])
      expect(useMcpPanelStore.getState()).toMatchObject({ pending: 'gh', tab: 'installed' })
    } finally {
      for (const spy of spies) spy.mockRestore()
    }
    expect(JSON.stringify(logged)).not.toContain(SECRET)
    expect(JSON.stringify(useChatStore.getState().messages)).not.toContain(SECRET)
  })
})

describe('MCP tool cells', () => {
  test('name the connector and the tool, with the tier the catalog pins', () => {
    const servers = EXTENSIONS.mcp_servers
    const merge = mcpToolCallLabel('mcp__github__merge_pull_request', {}, servers)
    expect(merge).toEqual({ server: 'github', tool: 'merge_pull_request', tier: 'destructive' })
    expect(mcpToolTitle(merge!)).toBe('github · merge_pull_request')
    expect(mcpToolCallLabel('mcp__github__get_me', {}, servers)?.tier).toBe('read')
    // Through the MCPTool wrapper, by its qualified name.
    expect(
      mcpToolCallLabel('MCPTool', { qualifiedName: 'mcp__github__get_me' }, servers),
    ).toEqual({ server: 'github', tool: 'get_me', tier: 'read' })
    // A server without tiers, and before any are known.
    expect(mcpToolCallLabel('mcp__codebase-memory__search_graph', {}, servers)).toEqual({
      server: 'codebase-memory',
      tool: 'search_graph',
      tier: null,
    })
    expect(mcpToolCallLabel('mcp__notes__add', {}, null)).toEqual({
      server: 'notes',
      tool: 'add',
      tier: null,
    })
    // A server whose name holds `__` matches as a whole.
    const odd = [{ ...byName('github'), name: 'my__srv', tool_tiers: { go: 'write' } }]
    expect(mcpToolCallLabel('mcp__my__srv__go', {}, odd)).toEqual({
      server: 'my__srv',
      tool: 'go',
      tier: 'write',
    })
    expect(mcpToolCallLabel('read_file', {}, servers)).toBeNull()
    expect(mcpToolCallLabel('MCPTool', {}, servers)).toBeNull()
  })

  // OSC 52 (write the clipboard), OSC 8 (a link) and CSI (clear the screen).
  const HOSTILE = 'mcp__evil\x1b]52;c;aGk=\x07__steal\x1b]8;;https://x.test\x07me\x1b[2J\nnow'

  test('a name the model wrote is cleaned of escape sequences', () => {
    const label = mcpToolCallLabel('MCPTool', { qualifiedName: HOSTILE }, null)
    expect(label).toEqual({ server: 'evil', tool: 'stealme now', tier: null })
    expect(mcpToolTitle(label!)).toBe('evil · stealme now')
    // A configured server's name is cleaned too.
    const odd = [{ ...byName('github'), name: 'gh\x1b[31m', tool_tiers: {} }]
    expect(mcpToolCallLabel('mcp__gh\x1b[31m__go', {}, odd)?.server).toBe('gh')
  })

  test('the tool cell draws the cleaned name', async () => {
    const setup = await createTestRenderer({ width: 90, height: 8 })
    const root = createRoot(setup.renderer)
    flushSync(() =>
      root.render(
        <ToolBranch
          toolBlock={{
            type: 'tool',
            toolCallId: 'call-1',
            toolName: 'MCPTool' as never,
            input: { qualifiedName: HOSTILE, arguments: {} },
          }}
          keyPrefix="test"
          availableWidth={90}
          onToggleCollapsed={() => {}}
          markdownPalette={createMarkdownPalette(useThemeStore.getState().theme)}
        />,
      ),
    )
    try {
      await setup.renderOnce()
      const frame = setup.captureCharFrame()
      expect(frame).toContain('evil · stealme now')
      expect(frame).not.toContain('\x1b')
      expect(frame).not.toContain('52;c;')
      expect(frame).not.toContain('https://x.test')
    } finally {
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    }
  })
})

describe('approving MCP servers on the trust screen', () => {
  const press = (name: string, sequence = '') => ({ name, ctrl: false, sequence })

  test('space or Enter ticks a server row; the choices work below them', () => {
    // Two servers above the two choices: Allow is row 2.
    expect(trustScreenKeyAction(press('space', ' '), 0, 2)).toEqual({ type: 'toggle', index: 0 })
    expect(trustScreenKeyAction(press('return', '\r'), 1, 2)).toEqual({ type: 'toggle', index: 1 })
    expect(trustScreenKeyAction(press('return', '\r'), 2, 2)).toEqual({ type: 'choose', choice: 'allow' })
    expect(trustScreenKeyAction(press('return', '\r'), 3, 2)).toEqual({ type: 'choose', choice: 'quit' })
    expect(trustScreenKeyAction(press('up'), 2, 2)).toEqual({ type: 'select', index: 1 })
    expect(trustScreenKeyAction(press('down'), 3, 2)).toEqual({ type: 'select', index: 3 })
    expect(trustScreenKeyAction(press('1', '1'), 0, 2)).toEqual({ type: 'choose', choice: 'allow' })
    expect(trustScreenKeyAction(press('space', ' '), 2, 2)).toEqual({ type: 'none' })
    // Without servers it is the plain trust screen.
    expect(trustScreenKeyAction(press('down'), 0, 0)).toEqual({ type: 'select', index: 1 })
  })

  test('the ticked servers are approved once the restarted engine is ready', () => {
    const sent = recordingEngine()
    trustWorkspace(['postgres-tools'])
    expect(sent).toEqual([{ type: 'trust_workspace' }])
    expect(useStartGate.getState().trustStep.kind).toBe('saving')
    sent.length = 0
    // An engine without the catalog commands gets none, and they are forgotten.
    expect(approveTrustedMcpServers({ send: (command) => void sent.push(command) }, [FEATURE_MCP_CATALOG])).toBe(true)
    expect(sent).toEqual([{ type: 'approve_mcp_server', name: 'postgres-tools' }])
    expect(approveTrustedMcpServers({ send: (command) => void sent.push(command) }, [FEATURE_MCP_CATALOG])).toBe(false)
    trustWorkspace(['docs'])
    expect(approveTrustedMcpServers({ send: (command) => void sent.push(command) }, [])).toBe(false)
    expect(sent).toHaveLength(2)
    useStartGate.getState().setTrustStep({ kind: 'asking' })
  })
})
