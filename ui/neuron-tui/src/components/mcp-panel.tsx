/**
 * `/mcp`: the MCP servers and connectors, in the composer's place (engine
 * feature `mcp_toggle`), in two tabs.
 *
 * Installed: each server with its switch, transport, scope and what it is
 * doing. A server that waits for the user carries a badge after its name:
 * `[blocked]`, `[needs approval]`, `[needs connect]`, `[unpinned]`.
 *
 *   ↑/↓ move   space/enter on/off   a approve   c connect   l sign in   x remove   tab Add   esc close
 *
 * A switch is saved in the user settings (`disabledMcpServers`); the servers
 * restart from the next turn. A server the connector policy requires shows a
 * lock and its switch does nothing. A workspace server that is new or changed
 * (`needs approval`) starts once `a` approves it. A hosted connector not
 * linked to the Zero-X account yet shows `needs connect`; `c` opens the page
 * that links it. A remote server that needs a sign-in says so, and `l` runs
 * `/mcp login <server>`. `x` twice removes a server with its secrets. A stdio
 * server that is neither a catalog connector nor pinned ends its row with
 * `unpinned` (`/mcp pin <server>` records its version and hashes).
 *
 * Add (engine feature `mcp_catalog`): search the connector catalog, pick a
 * connector, fill its variables (secret ones are masked and go to the OS
 * keychain through the engine), name it and pick where it is saved (user,
 * project or local), then Enter adds it (utils/mcp-add-form.ts).
 */

import {
  TextAttributes,
  decodePasteBytes,
  stripAnsiSequences,
  type KeyEvent,
  type PasteEvent,
} from '@opentui/core'
import { useKeyboard, usePaste } from '@opentui/react'
import React, { useCallback, useEffect, useState } from 'react'

import { truncateToWidth, visibleWindow } from './model-picker'
import {
  addConnector,
  approveMcpServer,
  catalogAvailable,
  connectMcpServer,
  removeMcpServer,
  serverRemovable,
  showMcpTab,
  signInMcpServer,
  toggleMcpServer,
} from '../engine/mcp-panel'
import { useTheme } from '../hooks/use-theme'
import {
  serverConnectUrl,
  serverEnabled,
  serverLocked,
  serverNeedsApproval,
  serverNeedsSignIn,
  serverBadges,
  serverSummary,
  useMcpPanelStore,
} from '../state/mcp-panel-store'
import { displayWidth, padToDisplayWidth } from '../utils/display-width'
import { getGlyphMode, type GlyphMode } from '../utils/glyphs'
import {
  ADD_TAB_START,
  addTabKey,
  filterConnectors,
  formFields,
  maskSecret,
  pasteIntoAddTab,
  type AddTabState,
} from '../utils/mcp-add-form'
import { BORDER_CHARS } from '../utils/ui-constants'

import type { ConnectorInfo, McpServerInfo } from '../engine/protocol'

export type McpKeyAction =
  | { type: 'move'; delta: number }
  | { type: 'toggle' }
  | { type: 'connect' }
  | { type: 'approve' }
  | { type: 'sign-in' }
  | { type: 'remove' }
  | { type: 'tab' }
  | { type: 'close' }
  | { type: 'none' }

/** What a key does on the Installed tab. */
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
    case 'c':
      return { type: 'connect' }
    case 'a':
      return { type: 'approve' }
    case 'l':
      return { type: 'sign-in' }
    case 'x':
      return { type: 'remove' }
    case 'tab':
      return { type: 'tab' }
    default:
      return { type: 'none' }
  }
}

/** The row's mark: on, off, sent, or locked on by the connector policy. */
export function serverMark(
  server: McpServerInfo,
  pending: string | null,
  mode: GlyphMode = getGlyphMode(),
): string {
  let mark: string
  if (pending === server.name) mark = '[…]'
  else if (serverLocked(server)) mark = mode === 'console' ? '[req]' : '[🔒]'
  else mark = serverEnabled(server) ? '[on]' : '[off]'
  return padToDisplayWidth(mark, 5)
}

/** The keys the Installed tab offers for `server`, for its footer. */
export function installedHints(
  server: McpServerInfo | undefined,
  confirmRemove: boolean,
  canAdd: boolean = catalogAvailable(),
): string {
  if (!server) return canAdd ? 'tab Add · esc close' : 'esc close'
  if (confirmRemove) return `x again removes ${server.name} · any other key keeps it`
  const hints = ['↑↓ move']
  if (!serverLocked(server)) hints.push('space on/off')
  if (serverNeedsApproval(server)) hints.push('a approve')
  if (serverConnectUrl(server)) hints.push('c connect')
  if (serverNeedsSignIn(server)) hints.push('l sign in')
  if (serverRemovable(server)) hints.push('x remove')
  if (canAdd) hints.push('tab Add')
  hints.push('esc close')
  return hints.join(' · ')
}

const TabBar = ({ tab, canAdd }: { tab: 'installed' | 'add'; canAdd: boolean }) => {
  const theme = useTheme()
  const label = (name: 'installed' | 'add', text: string) => (
    <span
      fg={tab === name ? theme.primary : theme.muted}
      attributes={tab === name ? TextAttributes.BOLD : undefined}
    >
      {tab === name ? `[${text}]` : ` ${text} `}
    </span>
  )
  return (
    <text style={{ wrapMode: 'none' }}>
      {label('installed', 'Installed')}
      {canAdd ? <span fg={theme.muted}> </span> : null}
      {canAdd ? label('add', 'Add') : null}
    </text>
  )
}

const InstalledTab = ({
  servers,
  pending,
  focus,
  confirmRemove,
  rowWidth,
  maxVisibleRows,
  glyphMode,
  canAdd,
}: {
  servers: McpServerInfo[] | null
  pending: string | null
  focus: number
  confirmRemove: string | null
  rowWidth: number
  maxVisibleRows: number
  glyphMode: GlyphMode
  canAdd: boolean
}) => {
  const theme = useTheme()
  if (servers === null) {
    return <text style={{ fg: theme.muted }}>Reading the MCP servers…</text>
  }
  if (servers.length === 0) {
    return (
      <text style={{ wrapMode: 'word', fg: theme.muted }}>
        {canAdd
          ? 'No MCP servers are configured. Tab opens the connector catalog.'
          : 'No MCP servers are configured. Add them under mcpServers in the settings.'}
      </text>
    )
  }
  const [start, end] = visibleWindow(servers.length, focus, maxVisibleRows)
  return (
    <>
      {servers.slice(start, end).map((server, offset) => {
        const focused = start + offset === focus
        const on = serverEnabled(server)
        const mark = serverMark(server, pending, glyphMode)
        const label = truncateToWidth(
          `${server.name}  ${server.transport} · ${server.scope}`,
          Math.max(6, Math.floor(rowWidth / 2)),
        )
        const removing = confirmRemove === server.name
        const badges = removing ? '' : serverBadges(server).map((badge) => ` [${badge}]`).join('')
        const summary = removing ? '  remove? press x again' : `  ${serverSummary(server)}`
        const attention = removing || server.error !== null || serverNeedsApproval(server)
        return (
          <text key={server.name} style={{ wrapMode: 'none' }}>
            <span fg={focused ? theme.primary : theme.muted}>{focused ? '› ' : '  '}</span>
            <span fg={on ? theme.success : theme.muted}>{`${mark} `}</span>
            <span fg={theme.foreground} attributes={focused ? TextAttributes.BOLD : undefined}>
              {label}
            </span>
            {badges ? (
              <span fg={theme.warning} attributes={TextAttributes.BOLD}>
                {badges}
              </span>
            ) : null}
            <span fg={attention ? theme.warning : theme.muted}>
              {truncateToWidth(
                summary,
                Math.max(4, rowWidth - 8 - displayWidth(label) - displayWidth(badges)),
              )}
            </span>
          </text>
        )
      })}
    </>
  )
}

/** What the focused field of the Add form explains under it. */
function fieldHelp(state: AddTabState, connector: ConnectorInfo): string | null {
  const field = formFields(connector)[state.field]
  if (field?.kind === 'scope') {
    return 'user: every folder · project: this repository, shared · local: this folder, not committed'
  }
  if (field?.kind !== 'variable') return null
  if (field.variable.help_url) return `Get it at ${field.variable.help_url}`
  return field.variable.secret ? 'Kept in the OS keychain, never in a settings file.' : null
}

const AddForm = ({
  state,
  connector,
  rowWidth,
  glyphMode,
}: {
  state: AddTabState
  connector: ConnectorInfo
  rowWidth: number
  glyphMode: GlyphMode
}) => {
  const theme = useTheme()
  const fields = formFields(connector)
  const labelWidth = Math.min(
    32,
    Math.max(
      12,
      ...fields.map((field) =>
        field.kind === 'variable' ? displayWidth(field.variable.label) + 1 : 11,
      ),
    ),
  )
  const help = fieldHelp(state, connector)
  return (
    <>
      <text style={{ wrapMode: 'none', fg: theme.foreground }}>
        {truncateToWidth(
          `Add ${connector.name} (${connector.transport}${connector.oauth ? ', signs in with OAuth' : ''})`,
          rowWidth,
        )}
      </text>
      {fields.map((field, index) => {
        const focused = index === state.field
        let label: string
        let value: string
        if (field.kind === 'variable') {
          label = `${field.variable.label}${field.variable.required ? '*' : ''}`
          const typed = state.form.values[field.variable.name] ?? ''
          value = field.variable.secret
            ? maskSecret(typed, glyphMode === 'console' ? '*' : '•')
            : typed
          if (!value && field.variable.default) value = `(${field.variable.default})`
        } else if (field.kind === 'name') {
          label = 'Server name'
          value = state.form.name || `(${connector.id})`
        } else {
          label = 'Scope'
          value = (['user', 'project', 'local'] as const)
            .map((scope) => (scope === state.form.scope ? `[${scope}]` : ` ${scope} `))
            .join(' ')
        }
        const cursor = focused && field.kind !== 'scope' ? '▏' : ''
        return (
          <text key={`${field.kind}-${index}`} style={{ wrapMode: 'none' }}>
            <span fg={focused ? theme.primary : theme.muted}>{focused ? '› ' : '  '}</span>
            <span fg={theme.foreground} attributes={focused ? TextAttributes.BOLD : undefined}>
              {padToDisplayWidth(truncateToWidth(label, labelWidth), labelWidth + 1)}
            </span>
            <span fg={theme.foreground}>
              {truncateToWidth(`${value}${cursor}`, Math.max(4, rowWidth - labelWidth - 3))}
            </span>
          </text>
        )
      })}
      {help ? (
        <text style={{ wrapMode: 'none', fg: theme.muted }}>{truncateToWidth(help, rowWidth)}</text>
      ) : null}
    </>
  )
}

const AddTab = ({
  state,
  connectors,
  rowWidth,
  maxVisibleRows,
  glyphMode,
}: {
  state: AddTabState
  connectors: ConnectorInfo[] | null
  rowWidth: number
  maxVisibleRows: number
  glyphMode: GlyphMode
}) => {
  const theme = useTheme()
  if (connectors === null) {
    return <text style={{ fg: theme.muted }}>Reading the connector catalog…</text>
  }
  const connector = connectors.find((entry) => entry.id === state.connectorId)
  if (state.stage === 'form' && connector) {
    return <AddForm state={state} connector={connector} rowWidth={rowWidth} glyphMode={glyphMode} />
  }
  const matches = filterConnectors(connectors, state.query)
  const [start, end] = visibleWindow(matches.length, state.focus, maxVisibleRows)
  return (
    <>
      <text style={{ wrapMode: 'none' }}>
        <span fg={theme.muted}>Search: </span>
        <span fg={theme.foreground}>{`${state.query}▏`}</span>
      </text>
      {matches.length === 0 ? (
        <text style={{ fg: theme.muted }}>No connector matches.</text>
      ) : (
        matches.slice(start, end).map((entry, offset) => {
          const focused = start + offset === state.focus
          const added = entry.added.length > 0 ? ' · added' : ''
          const label = truncateToWidth(entry.name, Math.max(6, Math.floor(rowWidth / 3)))
          const detail = `  ${entry.publisher}${entry.official ? ' (official)' : ''} · ${entry.licence}${added} · ${entry.description}`
          return (
            <text key={entry.id} style={{ wrapMode: 'none' }}>
              <span fg={focused ? theme.primary : theme.muted}>{focused ? '› ' : '  '}</span>
              <span fg={theme.foreground} attributes={focused ? TextAttributes.BOLD : undefined}>
                {label}
              </span>
              <span fg={theme.muted}>
                {truncateToWidth(detail, Math.max(4, rowWidth - 2 - displayWidth(label)))}
              </span>
            </text>
          )
        })
      )}
    </>
  )
}

export const McpPanel = ({
  servers,
  pending,
  width,
  maxVisibleRows = 8,
  glyphMode = getGlyphMode(),
}: {
  servers: McpServerInfo[] | null
  pending: string | null
  width: number
  maxVisibleRows?: number
  glyphMode?: GlyphMode
}) => {
  const theme = useTheme()
  const tab = useMcpPanelStore((state) => state.tab)
  const catalog = useMcpPanelStore((state) => state.catalog)
  const [focus, setFocus] = useState(0)
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null)
  const [add, setAdd] = useState<AddTabState>(ADD_TAB_START)
  const count = servers?.length ?? 0
  const connectors = catalog?.connectors ?? null
  const canAdd = catalogAvailable()
  useEffect(() => {
    setFocus((index) => Math.max(0, Math.min(index, count - 1)))
  }, [count])
  // A tab opens afresh.
  useEffect(() => {
    setConfirmRemove(null)
    setAdd(ADD_TAB_START)
  }, [tab])

  const handleInstalledKey = useCallback(
    (key: KeyEvent) => {
      const action = resolveMcpKey(key)
      const server = servers?.[focus]
      if (action.type !== 'remove') setConfirmRemove(null)
      if (action.type === 'none') return
      key.preventDefault?.()
      switch (action.type) {
        case 'close':
          useMcpPanelStore.getState().close()
          return
        case 'tab':
          if (canAdd) showMcpTab('add')
          return
        case 'move':
          setFocus((index) => Math.max(0, Math.min(count - 1, index + action.delta)))
          return
        case 'toggle':
          if (server) toggleMcpServer(server)
          return
        case 'connect':
          if (server) void connectMcpServer(server)
          return
        case 'approve':
          if (server) approveMcpServer(server)
          return
        case 'sign-in':
          if (server && serverNeedsSignIn(server)) signInMcpServer(server)
          return
        case 'remove':
          if (!server) return
          // A server the panel can't remove says why at once; another asks twice.
          if (!serverRemovable(server) || confirmRemove === server.name) {
            setConfirmRemove(null)
            removeMcpServer(server)
          } else {
            setConfirmRemove(server.name)
          }
          return
      }
    },
    [canAdd, confirmRemove, count, focus, servers],
  )

  const handleAddKey = useCallback(
    (key: KeyEvent) => {
      const { state, effect } = addTabKey(add, key, connectors ?? [])
      key.preventDefault?.()
      setAdd(state)
      switch (effect.type) {
        case 'close':
          useMcpPanelStore.getState().close()
          return
        case 'installed':
          showMcpTab('installed')
          return
        case 'add':
          addConnector(effect.connector, effect.form)
          return
      }
    },
    [add, connectors],
  )

  useKeyboard(
    useCallback(
      (key: KeyEvent) => (tab === 'add' ? handleAddKey(key) : handleInstalledKey(key)),
      [handleAddKey, handleInstalledKey, tab],
    ),
  )
  usePaste(
    useCallback(
      (event: PasteEvent) => {
        if (tab !== 'add') return
        const text = stripAnsiSequences(decodePasteBytes(event.bytes))
        if (!text) return
        event.preventDefault()
        setAdd((state) => pasteIntoAddTab(state, connectors ?? [], text))
      },
      [connectors, tab],
    ),
  )

  const rowWidth = Math.max(10, Math.max(20, width) - 4 - 6)
  const matches = connectors ? filterConnectors(connectors, add.query) : []
  let footer: string
  if (tab === 'add') {
    footer =
      add.stage === 'form'
        ? 'Enter add · ↑↓ field · ←→ scope · esc back'
        : `${matches.length > maxVisibleRows ? `${add.focus + 1}/${matches.length} · ` : ''}type to search · ↑↓ move · Enter pick · tab Installed · esc close`
  } else {
    const focused = servers?.[focus]
    footer = `${count > maxVisibleRows ? `${focus + 1}/${count} · ` : ''}${installedHints(
      focused,
      focused !== undefined && confirmRemove === focused.name,
      canAdd,
    )}`
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
      <TabBar tab={tab} canAdd={canAdd} />
      {tab === 'add' ? (
        <AddTab
          state={add}
          connectors={connectors}
          rowWidth={rowWidth}
          maxVisibleRows={maxVisibleRows}
          glyphMode={glyphMode}
        />
      ) : (
        <InstalledTab
          servers={servers}
          pending={pending}
          focus={focus}
          confirmRemove={confirmRemove}
          rowWidth={rowWidth}
          maxVisibleRows={maxVisibleRows}
          glyphMode={glyphMode}
          canAdd={canAdd}
        />
      )}
      <text style={{ fg: theme.muted, wrapMode: 'none' }}>
        {truncateToWidth(footer, rowWidth + 4)}
      </text>
    </box>
  )
}
