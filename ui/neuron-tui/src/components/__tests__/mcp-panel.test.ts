import { beforeEach, describe, expect, test } from 'bun:test'

import { connectMcpServer, handleMcpPanelEvent, toggleMcpServer } from '../../engine/mcp-panel'
import { findGitViewCommand } from '../../commands/git-view-commands'
import { FEATURE_MCP_TOGGLE } from '../../engine/protocol'
import { useChatStore } from '../../state/chat-store'
import {
  serverConnectUrl,
  serverEnabled,
  serverBadges,
  serverLocked,
  serverSummary,
  useMcpPanelStore,
} from '../../state/mcp-panel-store'
import { resolveMcpKey } from '../mcp-panel'

import type { McpServerInfo } from '../../engine/protocol'

const server = (overrides: Partial<McpServerInfo> = {}): McpServerInfo => ({
  name: 'docs',
  scope: 'user',
  transport: 'http',
  status: 'ready',
  tools: ['search', 'fetch'],
  error: null,
  enabled: true,
  ...overrides,
})

describe('the MCP servers panel', () => {
  beforeEach(() => useMcpPanelStore.getState().reset())

  test('keys move, toggle and close', () => {
    expect(resolveMcpKey({ name: 'up', ctrl: false, meta: false })).toEqual({ type: 'move', delta: -1 })
    expect(resolveMcpKey({ name: 'down', ctrl: false, meta: false })).toEqual({ type: 'move', delta: 1 })
    expect(resolveMcpKey({ name: 'space', ctrl: false, meta: false })).toEqual({ type: 'toggle' })
    expect(resolveMcpKey({ name: 'return', ctrl: false, meta: false })).toEqual({ type: 'toggle' })
    expect(resolveMcpKey({ name: 'escape', ctrl: false, meta: false })).toEqual({ type: 'close' })
    expect(resolveMcpKey({ name: 'c', ctrl: true, meta: false })).toEqual({ type: 'close' })
    expect(resolveMcpKey({ name: 'z', ctrl: false, meta: false })).toEqual({ type: 'none' })
    expect(resolveMcpKey({ name: 'c', ctrl: false, meta: false })).toEqual({ type: 'connect' })
  })

  test('rows say whether a server is on and what it does', () => {
    expect(serverSummary(server())).toBe('ready · 2 tools')
    expect(serverSummary(server({ enabled: false, status: 'disabled' }))).toBe('off')
    expect(
      serverSummary(server({ status: 'failed', tools: [], error: 'run `neuron mcp login docs`' })),
    ).toBe('failed: run `neuron mcp login docs`')
    // Servers that are configured but not started say why.
    expect(
      serverSummary(
        server({
          status: 'needs_approval',
          scope: 'project',
          tools: [],
          error: 'a workspace server: run /mcp approve docs to start it',
        }),
      ),
    ).toBe('a workspace server: run /mcp approve docs to start it')
    expect(
      serverSummary(
        server({ status: 'blocked', tools: [], error: "refused by the permissions.deny rule 'mcp__docs'" }),
      ),
    ).toBe("refused by the permissions.deny rule 'mcp__docs'")
    // Older engines leave `enabled` out: the server is on.
    const { enabled: _enabled, ...legacy } = server()
    expect(serverEnabled(legacy)).toBe(true)
  })

  test('blocked, needs approval, needs connect and unpinned are badges of their own', () => {
    expect(serverBadges(server({ status: 'needs_approval', error: 'a workspace server' }))).toEqual([
      'needs approval',
    ])
    expect(serverBadges(server({ status: 'blocked', error: 'denied' }))).toEqual(['blocked'])
    expect(
      serverBadges(server({ status: 'needs_approval', transport: 'stdio', unpinned: true })),
    ).toEqual(['needs approval', 'unpinned'])
    // A server that is off says only that; a ready or failed one has none.
    expect(serverBadges(server({ enabled: false, status: 'blocked' }))).toEqual([])
    expect(serverBadges(server())).toEqual([])
    expect(serverBadges(server({ status: 'failed', error: 'x' }))).toEqual([])
    // Without an error, the rest of the summary still shows.
    expect(serverSummary(server({ status: 'needs_approval' }))).toBe('2 tools')
  })

  test('a server whose version and hashes are not recorded says unpinned', () => {
    const unpinned = server({ transport: 'stdio', unpinned: true })
    expect(serverSummary(unpinned)).toBe('ready · 2 tools')
    expect(serverBadges(unpinned)).toEqual(['unpinned'])
    // Older engines leave it out, and so does a pinned or catalog server.
    expect(serverBadges(server({ transport: 'stdio' }))).toEqual([])
  })

  test('a server the connector policy requires is locked on', () => {
    const required = server({ name: 'codebase-memory', scope: 'managed', required: true })
    expect(serverLocked(required)).toBe(true)
    expect(serverLocked(server())).toBe(false)
    expect(serverSummary(required)).toBe('ready · 2 tools · required')
    expect(
      serverSummary(
        server({
          status: 'blocked',
          tools: [],
          error: 'refused by managed settings: mcpPolicy.deny lists "docs"',
        }),
      ),
    ).toBe('refused by managed settings: mcpPolicy.deny lists "docs"')

    // Its switch sends nothing and says why.
    const before = useChatStore.getState().messages.length
    toggleMcpServer(required)
    expect(useMcpPanelStore.getState().pending).toBeNull()
    const messages = useChatStore.getState().messages
    expect(messages.length).toBe(before + 1)
    expect(JSON.stringify(messages[messages.length - 1])).toContain(
      'codebase-memory is required by the connector policy',
    )
  })

  test('extensions fill the panel only while it is open; the chip keeps them either way', () => {
    const event = {
      type: 'extensions' as const,
      mcp_servers: [server()],
      plugins: [],
      skills: [],
      agents: [],
    }
    expect(handleMcpPanelEvent(event)).toBe(true)
    expect(useMcpPanelStore.getState().servers).toBeNull()
    expect(useMcpPanelStore.getState().known).toEqual([server()])
    useMcpPanelStore.getState().openPanel()
    useMcpPanelStore.getState().setPending('docs')
    expect(handleMcpPanelEvent(event)).toBe(true)
    expect(useMcpPanelStore.getState().servers).toEqual([server()])
    expect(useMcpPanelStore.getState().pending).toBeNull()
  })

  test('a hosted connector that is not linked opens the page that links it', async () => {
    const connectUrl = 'https://zero-x.live/dashboard#connectors'
    const hosted = server({
      name: 'github',
      scope: 'managed',
      transport: 'zerox',
      status: 'needs_connect',
      tools: [],
      error: `not linked to your Zero-X account; connect it at ${connectUrl} (/mcp connect github)`,
      connect_url: connectUrl,
    })
    expect(serverConnectUrl(hosted)).toBe(connectUrl)
    expect(serverSummary(hosted)).toStartWith('not linked to your Zero-X account')
    expect(serverBadges(hosted)).toEqual(['needs connect'])
    // Linked (or an older engine): nothing to connect.
    expect(serverConnectUrl(server({ transport: 'zerox' }))).toBeNull()
    expect(serverConnectUrl({ ...hosted, connect_url: undefined })).toBeNull()

    const opened: string[] = []
    const before = useChatStore.getState().messages.length
    expect(
      await connectMcpServer(hosted, async (url) => {
        opened.push(url)
        return true
      }),
    ).toBe(true)
    expect(opened).toEqual([connectUrl])
    let messages = useChatStore.getState().messages
    expect(messages.length).toBe(before + 1)
    expect(JSON.stringify(messages[messages.length - 1])).toContain(`Opening ${connectUrl}`)

    // No browser here: the page is still named.
    expect(await connectMcpServer(hosted, async () => false)).toBe(false)
    messages = useChatStore.getState().messages
    expect(JSON.stringify(messages[messages.length - 1])).toContain(
      `Connect github at ${connectUrl}`,
    )

    // Any other server opens nothing.
    opened.length = 0
    expect(
      await connectMcpServer(server(), async (url) => {
        opened.push(url)
        return true
      }),
    ).toBe(false)
    expect(opened).toEqual([])
  })

  test('/mcp is the UI’s own only when the engine has the switch', () => {
    expect(findGitViewCommand('mcp', [])).toBeUndefined()
    expect(findGitViewCommand('mcp', [FEATURE_MCP_TOGGLE])?.name).toBe('mcp')
  })
})
