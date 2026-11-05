/**
 * The MCP servers panel's link to the engine (feature `mcp_toggle`):
 * `list_extensions` fills it, `set_mcp_server_enabled` flips a server (the
 * engine saves it in the user settings, says so in a notice and lists the
 * servers again). `/mcp` with arguments (`/mcp show docs`, `/mcp enable docs`)
 * still goes to the engine as a slash command.
 */

import { getNeuronEngineClient, runExchange } from './neuron-engine-client'
import { useChatStore } from '../state/chat-store'
import { serverEnabled, serverLocked, useMcpPanelStore } from '../state/mcp-panel-store'
import { getSystemMessage } from '../utils/message-history'

import type { McpServerInfo, UiEvent } from './protocol'

let lineCount = 0

function showLine(text: string): void {
  lineCount += 1
  const message = { ...getSystemMessage(text), id: `sys-mcp-${Date.now()}-${lineCount}` }
  useChatStore.getState().setMessages((prev) => [...prev, message])
}

/** Takes `extensions` while the panel is open; `true` when it did. */
export function handleMcpPanelEvent(event: UiEvent): boolean {
  if (event.type !== 'extensions') return false
  const store = useMcpPanelStore.getState()
  if (!store.open) return false
  store.setServers(event.mcp_servers)
  return true
}

/** `/mcp`: opens the panel and asks the engine for the servers. */
export function openMcpPanel(): void {
  const engine = getNeuronEngineClient()
  if (engine.connection !== 'ready') {
    showLine('The Neuron engine is not running.')
    return
  }
  useMcpPanelStore.getState().openPanel()
  try {
    engine.send({ type: 'list_extensions' })
  } catch (error) {
    useMcpPanelStore.getState().close()
    showLine(error instanceof Error ? error.message : String(error))
  }
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
    showLine(error instanceof Error ? error.message : String(error))
  }
}

/** `/mcp <args>`: the engine's own report or switch, answered in the chat. */
export function forwardMcpSlash(args: string): void {
  const engine = getNeuronEngineClient()
  if (engine.connection !== 'ready') {
    showLine('The Neuron engine is not running.')
    return
  }
  void runExchange(engine, () => engine.slash('mcp', args.trim()), () => {}).catch(
    (error: unknown) => showLine(error instanceof Error ? error.message : String(error)),
  )
}

/** A new engine is starting. */
export function resetMcpPanel(): void {
  useMcpPanelStore.getState().reset()
}
