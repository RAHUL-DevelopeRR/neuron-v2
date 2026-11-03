import { useEffect, useState } from 'react'

import type { DefinitionInfo, Extensions, McpServerInfo } from '@neuron/client'

const STATUS_LABEL: Record<string, string> = {
  ready: 'ready',
  starting: 'starting',
  failed: 'failed',
  pending: 'no tools',
  unsupported: 'unsupported',
  not_started: 'not started',
}

/** How often the panel asks again while a server is still starting. */
const STARTING_REFRESH_MS = 2_000

function McpServerRow({ server }: { server: McpServerInfo }) {
  const [open, setOpen] = useState(false)
  const detail = server.error ?? (server.tools.length ? server.tools.join(', ') : null)
  return (
    <li className="extension-row">
      <button className="tree-row extension-head" onClick={() => detail && setOpen(!open)} aria-expanded={detail ? open : undefined}>
        <span className={`extension-status status-${server.status}`}>{STATUS_LABEL[server.status] ?? server.status}</span>
        <span className="extension-name">{server.name}</span>
        <span className="muted small">
          {server.scope} · {server.transport}
          {server.status === 'ready' && ` · ${server.tools.length} tool${server.tools.length === 1 ? '' : 's'}`}
        </span>
      </button>
      {open && detail && <div className={server.error ? 'error-line small extension-detail' : 'muted small extension-detail'}>{detail}</div>}
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
export function ExtensionsPanel({ extensions, onRefresh }: { extensions: Extensions | null; onRefresh: () => void }) {
  const [filter, setFilter] = useState('')
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
      {mcpServers.length === 0 ? (
        <div className="muted pad">
          None configured. Add servers under <code>mcpServers</code> in your Neuron settings.json.
        </div>
      ) : servers.length === 0 ? (
        <div className="muted pad">None match.</div>
      ) : (
        <ul className="extensions" aria-label="MCP servers">
          {servers.map((server) => (
            <McpServerRow key={server.name} server={server} />
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
