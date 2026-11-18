import { useEffect, useState } from 'react'

import type { DefinitionInfo, Extensions, McpServerInfo } from '@neuron/client'

const STATUS_LABEL: Record<string, string> = {
  ready: 'ready',
  starting: 'starting',
  failed: 'failed',
  pending: 'no tools',
  unsupported: 'unsupported',
  not_started: 'not started',
  disabled: 'off',
  needs_approval: 'needs approval',
  blocked: 'blocked',
  needs_connect: 'not connected',
}

/**
 * Statuses whose `error` is not a failure but the reason the server is not running: it waits for the
 * user, or a deny rule or the connector policy refuses it. The row shows the reason at once; a failed
 * server's error stays behind a click.
 */
const REASON_STATUSES: ReadonlySet<string> = new Set(['needs_approval', 'blocked', 'needs_connect'])

/** Hosted connectors through Zero-X (older engines said `managed_proxy`); a person reads them as hosted. */
const HOSTED_TRANSPORTS: ReadonlySet<string> = new Set(['zerox', 'managed_proxy'])
const transportLabel = (transport: string) => (HOSTED_TRANSPORTS.has(transport) ? 'hosted' : transport)

/** How a server that waits for approval is approved. The engine's own reason usually says it already. */
const APPROVE_COMMAND = '/mcp approve'

/** A padlock (U+1F512) followed by the text variation selector (U+FE0E), which asks for a plain glyph like the window's others. */
const LOCK = String.fromCodePoint(0x1f512, 0xfe0e)

/** How often the panel asks again while a server is still starting. */
const STARTING_REFRESH_MS = 2_000

/** Whether the engine reports this server as turned off. */
const isOff = (server: McpServerInfo) => server.enabled === false || server.status === 'disabled'

/** How long a switch waits for the engine's fresh list before it shows what the list says. */
const PENDING_MS = 6_000

interface Toggle {
  /** The state asked for while the engine has not confirmed it; `undefined` when idle. */
  pending: boolean | undefined
  onToggle: (enabled: boolean) => void
  /** Why the last switch did not go through (the engine is down), shown inline. */
  error?: string | null
}

function McpServerRow({ server, toggle }: { server: McpServerInfo; toggle: Toggle | null }) {
  const [open, setOpen] = useState(false)
  const off = isOff(server)
  // Why the server is not running, where that is what the row has to say: shown at once, not behind a click.
  const reason = !off && REASON_STATUSES.has(server.status) ? server.error : null
  const toolList = server.tools.length ? server.tools.join(', ') : null
  const detailError = reason ? null : server.error
  const detail = detailError ?? toolList
  // Said once: the hint is for a reason that does not already tell how to approve.
  const approveHint = !off && server.status === 'needs_approval' && !server.error?.includes(APPROVE_COMMAND)
  return (
    <li className="extension-row">
      <button className="tree-row extension-head" onClick={() => detail && setOpen(!open)} aria-expanded={detail ? open : undefined}>
        <span className={`extension-status status-${off ? 'disabled' : server.status}`}>{off ? 'off' : (STATUS_LABEL[server.status] ?? server.status)}</span>
        <span className="extension-name">{server.name}</span>
        <span className="muted small">
          {server.scope} · {transportLabel(server.transport)}
          {server.status === 'ready' && !off && ` · ${server.tools.length} tool${server.tools.length === 1 ? '' : 's'}`}
        </span>
      </button>
      {server.required === true ? (
        // The connector policy keeps it on: a marker, not a control, so nothing here can be pressed.
        <span className="extension-required muted small" title="Your connector policy requires this server, so it stays on">
          <span aria-hidden>{LOCK}</span> Required<span className="sr-only">: stays on</span>
        </span>
      ) : (
        toggle && (
          <button
            role="switch"
            className="extension-toggle"
            aria-label={server.name}
            aria-checked={toggle.pending === undefined ? !off : toggle.pending}
            aria-busy={toggle.pending !== undefined || undefined}
            // Not `disabled`: keyboard focus stays on the switch while it waits, and clicks are ignored.
            aria-disabled={toggle.pending !== undefined || undefined}
            onClick={() => {
              if (toggle.pending === undefined) toggle.onToggle(off)
            }}
          >
            <span className="switch-track" aria-hidden>
              <span className="switch-thumb" />
            </span>
            <span className="muted small">{toggle.pending === undefined ? (off ? 'Off' : 'On') : toggle.pending ? 'Turning on…' : 'Turning off…'}</span>
          </button>
        )
      )}
      {toggle?.error && (
        <div className="error-line small extension-detail" role="alert">
          {toggle.error}
        </div>
      )}
      {reason && <div className={`${server.status === 'blocked' ? 'error-line' : 'muted'} small extension-detail`}>{reason}</div>}
      {approveHint && (
        // Typed in the chat, which sends it to the engine as a slash command: the panel has no command of its own for it.
        <div className="muted small extension-detail">
          Approve it with <code>{`${APPROVE_COMMAND} ${server.name}`}</code> in the chat
        </div>
      )}
      {open && detail && <div className={detailError ? 'error-line small extension-detail' : 'muted small extension-detail'}>{detail}</div>}
    </li>
  )
}

/** Whether `query` (lowercase) is in any of `fields`. */
const matches = (query: string, ...fields: Array<string | null>) => !query || fields.some((field) => field?.toLowerCase().includes(query))

function Definitions({ title, items, empty, query }: { title: string; items: DefinitionInfo[]; empty: string; query: string }) {
  const shown = items.filter((item) => matches(query, item.name, item.description))
  return (
    <>
      <div className="pane-subtitle">
        {title} <span className="muted">{items.filter((item) => item.active).length}</span>
      </div>
      {items.length === 0 ? (
        <div className="muted pad">{empty}</div>
      ) : shown.length === 0 ? (
        <div className="muted pad">None match.</div>
      ) : (
        <ul className="extensions" aria-label={title}>
          {shown.map((item, index) => (
            <li key={`${index}:${item.name}`} className={`extension-row definition${item.active ? '' : ' shadowed'}`} title={item.active ? undefined : 'Another definition with this name wins'}>
              <span className="extension-name">{item.name}</span>
              <span className="muted small"> · {item.source || 'unknown'}</span>
              {item.description && <div className="muted small extension-description">{item.description}</div>}
            </li>
          ))}
        </ul>
      )}
    </>
  )
}

/**
 * What the engine can use beyond its own tools: MCP servers with their live status,
 * plugins, skills and agent definitions. Read-only; settings.json and the skill and
 * agent folders are where they are added.
 */
export function ExtensionsPanel({
  extensions,
  onRefresh,
  onSetEnabled,
}: {
  extensions: Extensions | null
  onRefresh: () => void
  /** Turns a server on or off (feature `mcp_toggle`); absent means no switches. */
  onSetEnabled?: (name: string, enabled: boolean) => void | Promise<void>
}) {
  const [filter, setFilter] = useState('')
  // Servers just switched, with the state asked for, until the fresh list shows it.
  const [pending, setPending] = useState<Record<string, boolean>>({})
  const [toggled, setToggled] = useState(false)
  // Servers whose last switch could not be sent, with the reason.
  const [failed, setFailed] = useState<Record<string, string>>({})
  useEffect(() => {
    if (!extensions) return
    const settled = Object.keys(pending).filter((name) => {
      const server = extensions.mcpServers.find((candidate) => candidate.name === name)
      return !server || !isOff(server) === pending[name]
    })
    if (settled.length) setPending((current) => Object.fromEntries(Object.entries(current).filter(([name]) => !settled.includes(name))))
  }, [extensions, pending])
  useEffect(() => {
    if (!Object.keys(pending).length) return
    const timer = setTimeout(() => setPending({}), PENDING_MS)
    return () => clearTimeout(timer)
  }, [pending])
  const setEnabled = (name: string, enabled: boolean) => {
    setFailed(({ [name]: _gone, ...rest }) => rest)
    setPending((current) => ({ ...current, [name]: enabled }))
    setToggled(true)
    // The engine may be down: a throw (or a rejection) ends the wait with a reason instead of leaving the switch pending.
    const fail = (error: unknown) => {
      setPending(({ [name]: _gone, ...rest }) => rest)
      setFailed((current) => ({ ...current, [name]: `Could not switch ${name}: ${error instanceof Error ? error.message : typeof error === 'string' ? error : 'the engine is not running.'}` }))
    }
    try {
      void Promise.resolve(onSetEnabled?.(name, enabled)).catch(fail)
    } catch (error) {
      fail(error)
    }
  }
  useEffect(() => {
    onRefresh()
  }, [onRefresh])

  const starting = extensions?.mcpServers.some((server) => server.status === 'starting') ?? false
  useEffect(() => {
    if (!starting) return
    const timer = setTimeout(onRefresh, STARTING_REFRESH_MS)
    return () => clearTimeout(timer)
  }, [starting, extensions, onRefresh])

  if (!extensions) return <div className="pane-body muted pad">Asking the engine…</div>
  const { mcpServers, plugins, skills, agents } = extensions
  const query = filter.trim().toLowerCase()
  const servers = mcpServers.filter((server) => matches(query, server.name, ...server.tools))
  const shownPlugins = plugins.filter((plugin) => matches(query, plugin.name, plugin.description))
  return (
    <div className="pane-body extensions-pane">
      <input
        className="extensions-filter"
        type="search"
        placeholder="Filter servers, plugins, skills, agents"
        aria-label="Filter extensions"
        value={filter}
        onChange={(event) => setFilter(event.target.value)}
      />
      <div className="pane-subtitle">
        MCP servers <span className="muted">{mcpServers.filter((server) => server.status === 'ready').length}/{mcpServers.length} ready</span>
        <button className="link-button" onClick={onRefresh}>
          Refresh
        </button>
      </div>
      {onSetEnabled && mcpServers.length > 0 && (
        <div className="muted small pad" role="status">
          {toggled ? 'A change applies from your next message.' : 'Turn a server off to stop the engine using it. A change applies from your next message.'}
        </div>
      )}
      {mcpServers.length === 0 ? (
        <div className="muted pad">
          None configured. Add servers under <code>mcpServers</code> in your Neuron settings.json.
        </div>
      ) : servers.length === 0 ? (
        <div className="muted pad">None match.</div>
      ) : (
        <ul className="extensions" aria-label="MCP servers">
          {servers.map((server) => (
            <McpServerRow
              key={server.name}
              server={server}
              toggle={onSetEnabled ? { pending: pending[server.name], error: failed[server.name], onToggle: (enabled) => setEnabled(server.name, enabled) } : null}
            />
          ))}
        </ul>
      )}

      <div className="pane-subtitle">
        Plugins <span className="muted">{plugins.filter((plugin) => plugin.enabled).length} on</span>
      </div>
      {plugins.length === 0 ? (
        <div className="muted pad">No plugins installed.</div>
      ) : shownPlugins.length === 0 ? (
        <div className="muted pad">None match.</div>
      ) : (
        <ul className="extensions" aria-label="Plugins">
          {shownPlugins.map((plugin) => (
            <li key={plugin.id} className={`extension-row definition${plugin.enabled ? '' : ' shadowed'}`}>
              <span className="extension-name">{plugin.name}</span>
              <span className="muted small">
                {' '}
                {plugin.version} · {plugin.kind} · {plugin.enabled ? 'on' : 'off'}
              </span>
              {plugin.description && <div className="muted small extension-description">{plugin.description}</div>}
            </li>
          ))}
        </ul>
      )}

      <Definitions title="Skills" items={skills} query={query} empty="No skills found. Put one in .claw/skills/<name>/SKILL.md." />
      <Definitions title="Agents" items={agents} query={query} empty="No agent definitions found." />
    </div>
  )
}
