/**
 * The MCP servers panel (`/mcp` while the engine lists the `mcp_toggle`
 * feature) and what the rest of the UI knows about the servers.
 *
 * `known` is the last `extensions` event, kept whether the panel is open or
 * not: the status bar's connectors chip and the tool cells of `mcp__` calls
 * read it. While the panel is open, `servers` is what it lists (`null` until
 * the engine answered), `tab` which of its tabs shows (the servers, or the
 * connector catalog to add one from), and `pending` the server whose change
 * was just sent. engine/mcp-panel.ts feeds it.
 */

import { create } from 'zustand'

import type { ConnectorInfo, McpServerInfo } from '../engine/protocol'

export type McpPanelTab = 'installed' | 'add'

export interface McpPanelState {
  open: boolean
  tab: McpPanelTab
  /** `null` until the engine answered `list_extensions`. */
  servers: McpServerInfo[] | null
  /** The servers of the last `extensions` event, panel open or not. */
  known: McpServerInfo[] | null
  /** The server whose change was sent and not answered yet. */
  pending: string | null
  /** The catalog's connectors for the Add tab; `null` until the engine answered. */
  catalog: { version: string; connectors: ConnectorInfo[] } | null
  openPanel: (tab?: McpPanelTab) => void
  close: () => void
  setTab: (tab: McpPanelTab) => void
  setServers: (servers: McpServerInfo[]) => void
  setKnown: (servers: McpServerInfo[]) => void
  setPending: (name: string | null) => void
  setCatalog: (catalog: { version: string; connectors: ConnectorInfo[] } | null) => void
  reset: () => void
}

export const useMcpPanelStore = create<McpPanelState>()((set) => ({
  open: false,
  tab: 'installed',
  servers: null,
  known: null,
  pending: null,
  catalog: null,
  openPanel: (tab = 'installed') =>
    set({ open: true, tab, servers: null, pending: null, catalog: null }),
  close: () => set({ open: false, pending: null }),
  setTab: (tab) => set({ tab }),
  setServers: (servers) => set({ servers, known: servers, pending: null }),
  setKnown: (known) => set({ known }),
  setPending: (pending) => set({ pending }),
  setCatalog: (catalog) => set({ catalog }),
  reset: () =>
    set({ open: false, tab: 'installed', servers: null, known: null, pending: null, catalog: null }),
}))

/** A server is on unless the engine says otherwise (older engines leave it out). */
export const serverEnabled = (server: McpServerInfo): boolean => server.enabled !== false

/** The connector policy requires the server: its switch is locked on. */
export const serverLocked = (server: McpServerInfo): boolean => server.required === true

/** A new or changed workspace server, waiting for the user to approve it. */
export const serverNeedsApproval = (server: McpServerInfo): boolean =>
  server.status === 'needs_approval'

/**
 * A remote server that needs a sign-in: its error names `neuron mcp login`
 * (or says it needs signing in).
 */
export function serverNeedsSignIn(server: McpServerInfo): boolean {
  if (server.transport !== 'http' && server.transport !== 'sse') return false
  return /mcp login|sign[ -]?in/i.test(server.error ?? '')
}

/**
 * The page that links a hosted connector the Zero-X account has not linked yet
 * (status `needs_connect`); `null` for any other server.
 */
export function serverConnectUrl(server: McpServerInfo): string | null {
  return server.status === 'needs_connect' && server.connect_url ? server.connect_url : null
}

/** A server in a state that needs the user (failed, blocked, waiting for approval or a link). */
export function serverNeedsAttention(server: McpServerInfo): boolean {
  if (!serverEnabled(server)) return false
  return ['failed', 'blocked', 'needs_approval', 'needs_connect', 'unsupported'].includes(
    server.status,
  )
}

/** Statuses a row shows as a badge of their own, by the badge's words. */
const BADGE_STATUSES: Readonly<Record<string, string>> = {
  blocked: 'blocked',
  needs_approval: 'needs approval',
  needs_connect: 'needs connect',
}

/**
 * The badges on a row, drawn apart from its summary so they stand out:
 * `blocked`, `needs approval`, `needs connect` (a server that is on) and
 * `unpinned`.
 */
export function serverBadges(server: McpServerInfo): string[] {
  const badges: string[] = []
  const status = serverEnabled(server) ? BADGE_STATUSES[server.status] : undefined
  if (status) badges.push(status)
  if (server.unpinned) badges.push('unpinned')
  return badges
}

/**
 * `ready · 4 tools`, `off`, `failed: why`, ... for a row, less what its
 * badges say ({@link serverBadges}): a `needs approval` row says only why.
 */
export function serverSummary(server: McpServerInfo): string {
  if (!serverEnabled(server)) return 'off'
  const tools =
    server.tools.length > 0
      ? ` · ${server.tools.length} ${server.tools.length === 1 ? 'tool' : 'tools'}`
      : ''
  const badged = BADGE_STATUSES[server.status] !== undefined
  const status = badged ? '' : server.status.replace('_', ' ')
  const required = serverLocked(server) ? ' · required' : ''
  if (server.error) return badged ? server.error : `${status}: ${server.error}`
  return `${status}${tools}${required}`.replace(/^ · /, '')
}

/** What the status bar's connectors chip says about the servers. */
export interface ConnectorsChip {
  /** Servers that are up. */
  ready: number
  /** Servers that are turned on. */
  total: number
  /** One needs the user (failed, blocked, waiting for approval or a link). */
  warning: boolean
  /** The connector policy requires one of them. */
  locked: boolean
}

/** The chip for the known servers; `null` before any are known or with none turned on. */
export function connectorsChip(servers: McpServerInfo[] | null): ConnectorsChip | null {
  if (!servers) return null
  const on = servers.filter(serverEnabled)
  if (on.length === 0) return null
  return {
    ready: on.filter((server) => server.status === 'ready').length,
    total: on.length,
    warning: on.some(serverNeedsAttention),
    locked: on.some(serverLocked),
  }
}
