import { useCallback, useEffect, useState } from 'react'

import type { DirEntry, Platform } from '../platform'

const HIDDEN = new Set(['.git', 'node_modules', 'target', '.neuron'])

/** The workspace as a tree, read on demand. `refreshKey` re-reads open folders (after the agent wrote files). */
export function FileTree({
  platform,
  refreshKey,
  changed,
  selected,
  onOpen,
}: {
  platform: Platform
  refreshKey: number
  changed: Set<string>
  selected: string | null
  onOpen: (path: string) => void
}) {
  const [children, setChildren] = useState<Record<string, DirEntry[]>>({})
  const [expanded, setExpanded] = useState<Set<string>>(new Set(['.']))
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(
    async (path: string) => {
      try {
        const listing = await platform.list(path)
        setChildren((current) => ({ ...current, [path]: listing.entries }))
        setError(null)
      } catch (failure) {
        setError(failure instanceof Error ? failure.message : String(failure))
      }
    },
    [platform],
  )

  useEffect(() => {
    for (const path of expanded) void load(path)
    // Re-read what is open when the agent may have changed it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshKey, load])

  const toggle = (entry: DirEntry) => {
    const next = new Set(expanded)
    if (next.has(entry.path)) next.delete(entry.path)
    else {
      next.add(entry.path)
      if (!children[entry.path]) void load(entry.path)
    }
    setExpanded(next)
  }

  const render = (path: string, depth: number) =>
    (children[path] ?? []).map((entry) => {
      const open = expanded.has(entry.path)
      const isChanged = changed.has(entry.path) || [...changed].some((p) => p.startsWith(`${entry.path}/`))
      return (
        <li key={entry.path}>
          <button
            className={`tree-row${selected === entry.path ? ' selected' : ''}${isChanged ? ' changed' : ''}${HIDDEN.has(entry.name) ? ' dim' : ''}`}
            style={{ paddingLeft: 8 + depth * 12 }}
            onClick={() => (entry.dir ? toggle(entry) : onOpen(entry.path))}
            title={entry.path}
          >
            <span className="tree-icon" aria-hidden>
              {entry.dir ? (open ? '▾' : '▸') : '·'}
            </span>
            {entry.name}
          </button>
          {entry.dir && open && <ul>{render(entry.path, depth + 1)}</ul>}
        </li>
      )
    })

  return (
    <nav className="file-tree" aria-label="Files">
      <div className="pane-title">
        Explorer
        <button className="icon-button" title="Refresh" onClick={() => expanded.forEach((path) => void load(path))}>
          ↻
        </button>
      </div>
      {error && <div className="error-line small">{error}</div>}
      <ul>{render('.', 0)}</ul>
    </nav>
  )
}
