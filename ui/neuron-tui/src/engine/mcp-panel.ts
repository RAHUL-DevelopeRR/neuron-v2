/**
 * The MCP servers panel's link to the engine (feature `mcp_toggle`):
 * `list_extensions` fills it, `set_mcp_server_enabled` flips a server (the
 * engine saves it in the user settings, says so in a notice and lists the
 * servers again). With feature `mcp_catalog` the panel also approves a
 * workspace server (`approve_mcp_server`), removes one (`remove_mcp_server`)
 * and adds a catalog connector (`list_connector_catalog`, `add_mcp_server`);
 * each change is answered with a notice and the servers again. `/mcp` with
 * arguments (`/mcp show docs`, `/mcp login docs`) still goes to the engine as
 * a slash command.
 *
 * Every `extensions` event is kept (state/mcp-panel-store.ts `known`), panel
 * open or not: the UI asks for one when the engine is ready, and the engine
 * sends one after each MCP change and once its servers finished starting.
 */

import { getNeuronEngineClient, runExchange, type NeuronEngineClient } from './neuron-engine-client'
import { FEATURE_EXTENSIONS, FEATURE_MCP_CATALOG } from './protocol'
import { useChatStore } from '../state/chat-store'
import { hasEngineFeature, useEngineStore } from '../state/engine-store'
import {
  serverConnectUrl,
  serverEnabled,
  serverLocked,
  serverNeedsApproval,
  useMcpPanelStore,
  type McpPanelTab,
} from '../state/mcp-panel-store'
import { getSystemMessage } from '../utils/message-history'
import { safeOpen } from '../utils/open-url'

import type { ConnectorInfo, EngineCommand, McpServerInfo, ReadyEvent, UiEvent } from './protocol'

let lineCount = 0

function showLine(text: string): void {
  lineCount += 1
  const message = { ...getSystemMessage(text), id: `sys-mcp-${Date.now()}-${lineCount}` }
  useChatStore.getState().setMessages((prev) => [...prev, message])
}

const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error))

/**
 * Takes `extensions` (kept for the chip and the tool cells; listed while the
 * panel is open) and, while it is open, `connector_catalog`. `true` when it
 * took the event.
 */
export function handleMcpPanelEvent(event: UiEvent): boolean {
  const store = useMcpPanelStore.getState()
  if (event.type === 'extensions') {
    if (store.open) store.setServers(event.mcp_servers)
    else store.setKnown(event.mcp_servers)
    return true
  }
  if (event.type === 'connector_catalog') {
    if (!store.open) return false
    store.setCatalog({ version: event.version, connectors: event.connectors })
    return true
  }
  return false
}

/**
 * Asks a ready engine for its servers, so the status bar's chip has them
 * before the panel is opened. `false` when the engine does not list them.
 */
export function requestExtensions(
  engine: Pick<NeuronEngineClient, 'send'>,
  ready: Pick<ReadyEvent, 'features'>,
): boolean {
  if (!ready.features?.includes(FEATURE_EXTENSIONS)) return false
  try {
    engine.send({ type: 'list_extensions' })
    return true
  } catch {
    // The engine died right after `ready`: its exit is reported elsewhere.
    return false
  }
}

/** Sends `command` to a ready engine; says why in the chat when it can't. */
function sendToEngine(command: EngineCommand): boolean {
  const engine = getNeuronEngineClient()
  if (engine.connection !== 'ready') {
    showLine('The Neuron engine is not running.')
    return false
  }
  try {
    engine.send(command)
    return true
  } catch (error) {
    showLine(errorText(error))
    return false
  }
}

/** `/mcp`: opens the panel on `tab` and asks the engine for the servers (and the catalog). */
export function openMcpPanel(tab: McpPanelTab = 'installed'): void {
  const engine = getNeuronEngineClient()
  if (engine.connection !== 'ready') {
    showLine('The Neuron engine is not running.')
    return
  }
  useMcpPanelStore.getState().openPanel(tab === 'add' && catalogAvailable() ? 'add' : 'installed')
  try {
    engine.send({ type: 'list_extensions' })
    if (useMcpPanelStore.getState().tab === 'add') searchConnectorCatalog('')
  } catch (error) {
    useMcpPanelStore.getState().close()
    showLine(errorText(error))
  }
}

/** The engine has the connector catalog and the approve, add and remove commands. */
export const catalogAvailable = (): boolean =>
  hasEngineFeature(useEngineStore.getState(), FEATURE_MCP_CATALOG)

/** Switches the panel's tab; the Add tab asks for the catalog the first time. */
export function showMcpTab(tab: McpPanelTab): void {
  const store = useMcpPanelStore.getState()
  if (tab === 'add' && !catalogAvailable()) {
    showLine('This engine has no connector catalog. Update Neuron to add connectors here.')
    return
  }
  store.setTab(tab)
  if (tab === 'add' && store.catalog === null) searchConnectorCatalog('')
}

/** The catalog's connectors matching every word of `query` (all when empty). */
export function searchConnectorCatalog(query: string): void {
  if (!catalogAvailable()) return
  sendToEngine({ type: 'list_connector_catalog', query: query.trim() })
}

/**
 * Space or Enter on a row: turns the server off when it is on, on when it is
 * off. A server the connector policy requires stays on.
 */
export function toggleMcpServer(server: McpServerInfo): void {
  const store = useMcpPanelStore.getState()
  if (store.pending) return
  if (serverLocked(server)) {
    showLine(`${server.name} is required by the connector policy and cannot be turned off.`)
    return
  }
  const engine = getNeuronEngineClient()
  if (engine.connection !== 'ready') {
    showLine('The Neuron engine is not running.')
    return
  }
  store.setPending(server.name)
  try {
    engine.send({
      type: 'set_mcp_server_enabled',
      name: server.name,
      enabled: !serverEnabled(server),
    })
  } catch (error) {
    store.setPending(null)
    showLine(errorText(error))
  }
}

/** `a` on a workspace server waiting for approval: approves it as configured now. */
export function approveMcpServer(server: McpServerInfo): boolean {
  const store = useMcpPanelStore.getState()
  if (store.pending) return false
  if (!serverNeedsApproval(server)) {
    showLine(`${server.name} is not waiting for approval.`)
    return false
  }
  if (!catalogAvailable()) {
    forwardMcpSlash(`approve ${server.name}`)
    return true
  }
  store.setPending(server.name)
  if (sendToEngine({ type: 'approve_mcp_server', name: server.name })) return true
  store.setPending(null)
  return false
}

/** A server the panel cannot remove: required by the policy, managed or Neuron's own. */
export function serverRemovable(server: McpServerInfo): boolean {
  return !serverLocked(server) && server.scope !== 'managed' && server.scope !== 'bundled'
}

/**
 * `x` twice on a row: removes the server from the settings of its scope, with
 * its keychain secrets and sign-in. A required, managed or bundled server stays.
 */
export function removeMcpServer(server: McpServerInfo): boolean {
  const store = useMcpPanelStore.getState()
  if (store.pending) return false
  if (!serverRemovable(server)) {
    showLine(
      `${server.name} comes from ${server.scope === 'bundled' ? 'Neuron itself' : 'the connector policy'} and cannot be removed here; turn it off instead.`,
    )
    return false
  }
  if (!catalogAvailable()) {
    forwardMcpSlash(`remove ${server.name}`)
    return true
  }
  store.setPending(server.name)
  if (sendToEngine({ type: 'remove_mcp_server', name: server.name, scope: server.scope })) {
    return true
  }
  store.setPending(null)
  return false
}

/** `l` on a remote server that needs a sign-in: `/mcp login <server>` (opens the browser). */
export function signInMcpServer(server: McpServerInfo): void {
  forwardMcpSlash(`login ${server.name}`)
}

/** Where a connector added from the catalog is saved. */
export type ConnectorScope = 'user' | 'project' | 'local'

export const CONNECTOR_SCOPES: readonly ConnectorScope[] = ['user', 'project', 'local']

/** What the Add tab's form holds for one connector. */
export interface ConnectorForm {
  /** The server name; the connector's id when empty. */
  name: string
  scope: ConnectorScope
  /** What was typed for each variable, by its name. Secrets included: never log it. */
  values: Record<string, string>
}

/** The variables the form still needs: required, with nothing typed and no default. */
export function missingVariables(connector: ConnectorInfo, form: ConnectorForm): string[] {
  return connector.variables
    .filter((variable) => variable.required && !variable.default)
    .filter((variable) => !(form.values[variable.name] ?? '').trim())
    .map((variable) => variable.label || variable.name)
}

/**
 * Enter on the Add form: `add_mcp_server` with what was typed. The secrets go
 * to the engine only (which keeps them in the OS keychain); nothing here logs
 * them, and an error is shown without the command.
 */
export function addConnector(connector: ConnectorInfo, form: ConnectorForm): boolean {
  const store = useMcpPanelStore.getState()
  if (store.pending) return false
  const missing = missingVariables(connector, form)
  if (missing.length > 0) {
    showLine(`${connector.name} needs ${missing.join(', ')}.`)
    return false
  }
  const secrets: Record<string, string> = {}
  for (const variable of connector.variables) {
    const value = (form.values[variable.name] ?? '').trim()
    if (value) secrets[variable.name] = value
  }
  const name = form.name.trim()
  store.setPending(name || connector.id)
  const sent = sendToEngine({
    type: 'add_mcp_server',
    id: connector.id,
    name: name && name !== connector.id ? name : null,
    scope: form.scope,
    secrets,
  })
  if (!sent) {
    store.setPending(null)
    return false
  }
  // The new server shows on the Installed tab once the engine lists it.
  store.setTab('installed')
  return true
}

/**
 * `c` on a hosted connector that needs connecting: opens the Zero-X page that
 * links it (`open` is the browser opener) and says where it is either way.
 * `true` when the browser opened.
 */
export async function connectMcpServer(
  server: McpServerInfo,
  open: (url: string) => Promise<boolean> = safeOpen,
): Promise<boolean> {
  const url = serverConnectUrl(server)
  if (!url) {
    showLine(`${server.name} has nothing to connect.`)
    return false
  }
  const opened = await open(url).catch(() => false)
  showLine(
    opened
      ? `Opening ${url} to connect ${server.name}; it is ready from the next turn.`
      : `Connect ${server.name} at ${url}; it is ready from the next turn.`,
  )
  return opened
}

/** `/mcp <args>`: the engine's own report or switch, answered in the chat. */
export function forwardMcpSlash(args: string): void {
  const engine = getNeuronEngineClient()
  if (engine.connection !== 'ready') {
    showLine('The Neuron engine is not running.')
    return
  }
  void runExchange(engine, () => engine.slash('mcp', args.trim()), () => {}).catch(
    (error: unknown) => showLine(errorText(error)),
  )
}

/** A new engine is starting. */
export function resetMcpPanel(): void {
  useMcpPanelStore.getState().reset()
}
