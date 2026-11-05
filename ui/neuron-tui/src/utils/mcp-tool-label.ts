/**
 * The connector behind an MCP tool call, for its cell in the transcript:
 * `mcp__github__merge_pull_request` (or `MCPTool` naming it in
 * `qualifiedName`) is drawn as "github · merge_pull_request" with the tier the
 * connector catalog pins for that tool (`destructive`), when the engine sent
 * one (`McpServerInfo.tool_tiers`).
 */

import type { McpServerInfo } from '../engine/protocol'

export interface McpToolCallLabel {
  /** The server (connector) name as configured. */
  server: string
  /** The tool's own name, without the `mcp__<server>__` prefix. */
  tool: string
  /** `read`, `write`, `destructive` or `external`; `null` when not known. */
  tier: string | null
}

/** A name as the engine writes it into tool names (`normalize_name_for_mcp`). */
export function normalizeMcpName(name: string): string {
  return name.replace(/[^A-Za-z0-9_-]/g, '_')
}

/**
 * The server and tool an MCP call goes to; `null` for any other tool. With the
 * known servers, the longest one whose prefix matches wins (a server name may
 * hold `__`); without them, the name is split at its first `__`.
 */
export function mcpToolCallLabel(
  toolName: string,
  input: unknown,
  servers: McpServerInfo[] | null,
): McpToolCallLabel | null {
  let qualified = toolName
  if (toolName === 'MCPTool') {
    const named =
      input && typeof input === 'object' ? (input as Record<string, unknown>).qualifiedName : undefined
    if (typeof named !== 'string') return null
    qualified = named
  }
  if (!qualified.startsWith('mcp__')) return null
  const rest = qualified.slice('mcp__'.length)
  const match = (servers ?? [])
    .map((server) => ({ server, prefix: `${normalizeMcpName(server.name)}__` }))
    .filter(({ prefix }) => rest.startsWith(prefix) && rest.length > prefix.length)
    .sort((a, b) => b.prefix.length - a.prefix.length)[0]
  if (match) {
    const tool = rest.slice(match.prefix.length)
    const tiers = match.server.tool_tiers ?? {}
    const tier =
      tiers[tool] ??
      Object.entries(tiers).find(([name]) => normalizeMcpName(name) === tool)?.[1] ??
      null
    return { server: match.server.name, tool, tier }
  }
  const split = rest.indexOf('__')
  if (split <= 0 || split + 2 >= rest.length) return null
  return { server: rest.slice(0, split), tool: rest.slice(split + 2), tier: null }
}

/** The cell's title: "github · merge_pull_request". */
export function mcpToolTitle(label: McpToolCallLabel): string {
  return `${label.server} · ${label.tool}`
}
