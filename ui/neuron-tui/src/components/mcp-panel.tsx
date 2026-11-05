/**
 * `/mcp`: the MCP servers with what each is doing, and a switch per server, in
 * the composer's place (engine feature `mcp_toggle`).
 *
 *   ↑/↓ move   space/enter turn on or off   esc close
 *
 * A switch is saved in the user settings (`disabledMcpServers`); the servers
 * restart from the next turn. A server the connector policy requires shows
 * `[req]` and its switch does nothing. Remote servers that need a sign-in say so in
 * their error (`neuron mcp login <server>` in a terminal).
 */

import { TextAttributes, type KeyEvent } from '@opentui/core'
import { useKeyboard } from '@opentui/react'
import React, { useCallback, useEffect, useState } from 'react'

import { truncateToWidth, visibleWindow } from './model-picker'
import { toggleMcpServer } from '../engine/mcp-panel'
import { useTheme } from '../hooks/use-theme'
import {
  serverEnabled,
  serverLocked,
  serverSummary,
  useMcpPanelStore,
} from '../state/mcp-panel-store'
import { BORDER_CHARS } from '../utils/ui-constants'

import type { McpServerInfo } from '../engine/protocol'

export type McpKeyAction =
  | { type: 'move'; delta: number }
  | { type: 'toggle' }
  | { type: 'close' }
  | { type: 'none' }

/** What a key does in the panel. */
export function resolveMcpKey(key: Pick<KeyEvent, 'name' | 'ctrl' | 'meta'>): McpKeyAction {
  if (key.name === 'escape' || (key.ctrl && !key.meta && key.name === 'c')) {
    return { type: 'close' }
  }
  if (key.ctrl || key.meta) return { type: 'none' }
  switch (key.name) {
    case 'up':
      return { type: 'move', delta: -1 }
    case 'down':
      return { type: 'move', delta: 1 }
    case 'space':
    case 'return':
    case 'enter':
      return { type: 'toggle' }
    default:
      return { type: 'none' }
  }
}

export const McpPanel = ({
  servers,
  pending,
  width,
  maxVisibleRows = 8,
}: {
  servers: McpServerInfo[] | null
  pending: string | null
  width: number
  maxVisibleRows?: number
}) => {
  const theme = useTheme()
  const [focus, setFocus] = useState(0)
  const count = servers?.length ?? 0
  useEffect(() => {
    setFocus((index) => Math.max(0, Math.min(index, count - 1)))
  }, [count])

  useKeyboard(
    useCallback(
      (key: KeyEvent) => {
        const action = resolveMcpKey(key)
        if (action.type === 'none') return
        key.preventDefault?.()
        switch (action.type) {
          case 'close':
            useMcpPanelStore.getState().close()
            return
          case 'move':
            setFocus((index) => Math.max(0, Math.min(count - 1, index + action.delta)))
            return
          case 'toggle': {
            const server = servers?.[focus]
            if (server) toggleMcpServer(server)
            return
          }
        }
      },
      [count, focus, servers],
    ),
  )

  const rowWidth = Math.max(10, Math.max(20, width) - 4 - 6)
  let body: React.ReactNode
  if (servers === null) {
    body = <text style={{ fg: theme.muted }}>Reading the MCP servers…</text>
  } else if (servers.length === 0) {
    body = (
      <text style={{ wrapMode: 'word', fg: theme.muted }}>
        No MCP servers are configured. Add them under mcpServers in the settings.
      </text>
    )
  } else {
    const [start, end] = visibleWindow(servers.length, focus, maxVisibleRows)
    body = servers.slice(start, end).map((server, offset) => {
      const focused = start + offset === focus
      const on = serverEnabled(server)
      const busy = pending === server.name
      const mark = busy ? '[…]' : serverLocked(server) ? '[req]' : on ? '[on] ' : '[off]'
      const label = `${server.name}  ${server.transport}`
      const summary = `  ${serverSummary(server)}`
      return (
        <text key={server.name} style={{ wrapMode: 'none' }}>
          <span fg={focused ? theme.primary : theme.muted}>{focused ? '› ' : '  '}</span>
          <span fg={on ? theme.success : theme.muted}>{`${mark} `}</span>
          <span fg={theme.foreground} attributes={focused ? TextAttributes.BOLD : undefined}>
            {truncateToWidth(label, Math.max(6, Math.floor(rowWidth / 2)))}
          </span>
          <span fg={server.error ? theme.warning : theme.muted}>
            {truncateToWidth(summary, Math.max(4, rowWidth - label.length))}
          </span>
        </text>
      )
    })
  }

  return (
    <box
      title=" MCP servers "
      titleAlignment="center"
      style={{
        width: Math.max(20, width),
        borderStyle: 'single',
        borderColor: theme.border,
        customBorderChars: BORDER_CHARS,
        paddingLeft: 1,
        paddingRight: 1,
        flexDirection: 'column',
      }}
    >
      {body}
      <text style={{ fg: theme.muted }}>
        {count > maxVisibleRows ? `${focus + 1}/${count} · ` : ''}
        {count > 0 ? '↑↓ move · space turn on/off · esc close' : 'esc close'}
      </text>
    </box>
  )
}
