import { beforeEach, describe, expect, test } from 'bun:test'

import { handleMcpPanelEvent } from '../../engine/mcp-panel'
import { findGitViewCommand } from '../../commands/git-view-commands'
import { FEATURE_MCP_TOGGLE } from '../../engine/protocol'
import { serverEnabled, serverSummary, useMcpPanelStore } from '../../state/mcp-panel-store'
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
    expect(resolveMcpKey({ name: 'x', ctrl: false, meta: false })).toEqual({ type: 'none' })
  })

  test('rows say whether a server is on and what it does', () => {
    expect(serverSummary(server())).toBe('ready · 2 tools')
    expect(serverSummary(server({ enabled: false, status: 'disabled' }))).toBe('off')
    expect(
      serverSummary(server({ status: 'failed', tools: [], error: 'run `neuron mcp login docs`' })),
    ).toBe('failed: run `neuron mcp login docs`')
    // Older engines leave `enabled` out: the server is on.
    const { enabled: _enabled, ...legacy } = server()
    expect(serverEnabled(legacy)).toBe(true)
  })

  test('extensions fill the panel only while it is open', () => {
    const event = {
      type: 'extensions' as const,
      mcp_servers: [server()],
      plugins: [],
      skills: [],
      agents: [],
    }
    expect(handleMcpPanelEvent(event)).toBe(false)
    useMcpPanelStore.getState().openPanel()
    useMcpPanelStore.getState().setPending('docs')
    expect(handleMcpPanelEvent(event)).toBe(true)
    expect(useMcpPanelStore.getState().servers).toEqual([server()])
    expect(useMcpPanelStore.getState().pending).toBeNull()
  })

  test('/mcp is the UI’s own only when the engine has the switch', () => {
    expect(findGitViewCommand('mcp', [])).toBeUndefined()
    expect(findGitViewCommand('mcp', [FEATURE_MCP_TOGGLE])?.name).toBe('mcp')
  })
})
