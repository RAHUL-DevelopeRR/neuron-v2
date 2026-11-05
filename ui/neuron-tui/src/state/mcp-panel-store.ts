/**
 * The MCP servers panel (`/mcp` while the engine lists the `mcp_toggle`
 * feature): open or not, the servers of the last `extensions` event, and the
 * server whose switch was just sent. engine/mcp-panel.ts feeds it.
 */

import { create } from 'zustand'

import type { McpServerInfo } from '../engine/protocol'

export interface McpPanelState {
  open: boolean
  /** `null` until the engine answered `list_extensions`. */
  servers: McpServerInfo[] | null
  /** The server whose switch was sent and not answered yet. */
  pending: string | null
  openPanel: () => void
  close: () => void
  setServers: (servers: McpServerInfo[]) => void
  setPending: (name: string | null) => void
  reset: () => void
}

export const useMcpPanelStore = create<McpPanelState>()((set) => ({
  open: false,
  servers: null,
  pending: null,
  openPanel: () => set({ open: true, servers: null, pending: null }),
  close: () => set({ open: false, pending: null }),
  setServers: (servers) => set({ servers, pending: null }),
  setPending: (pending) => set({ pending }),
  reset: () => set({ open: false, servers: null, pending: null }),
}))

/** A server is on unless the engine says otherwise (older engines leave it out). */
export const serverEnabled = (server: McpServerInfo): boolean => server.enabled !== false

/** The connector policy requires the server: its switch is locked on. */
export const serverLocked = (server: McpServerInfo): boolean => server.required === true

/** `ready · 4 tools`, `off`, `failed: why`, ... for a row. */
export function serverSummary(server: McpServerInfo): string {
  if (!serverEnabled(server)) return 'off'
  const tools =
    server.tools.length > 0
      ? ` · ${server.tools.length} ${server.tools.length === 1 ? 'tool' : 'tools'}`
      : ''
  const status = server.status.replace('_', ' ')
  const required = serverLocked(server) ? ' · required' : ''
  const unpinned = server.unpinned ? ' · unpinned' : ''
  return server.error
    ? `${status}: ${server.error}`
    : `${status}${tools}${required}${unpinned}`
}
