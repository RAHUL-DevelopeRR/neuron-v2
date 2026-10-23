import { useState, type ReactNode } from 'react'

import { BADGE_LABEL, ago, filterRows, groupRows, type GroupBy, type SessionRow } from '../sessions'
import type { Project, UpdateInfo } from '../platform'

function Page({ title, onClose, children, actions }: { title: string; onClose: () => void; children: ReactNode; actions?: ReactNode }) {
  return (
    <div className="page">
      <header className="page-header">
        <h1>{title}</h1>
        <div className="spacer" />
        {actions}
        <button className="icon-button" aria-label={`Close ${title}`} title="Back to the session (Esc)" onClick={onClose}>
          ✕
        </button>
      </header>
      <div className="page-body">{children}</div>
    </div>
  )
}

/** The folders Neuron worked in, and a native folder picker. */
export function ProjectsPage({
  current,
  recent,
  counts,
  canPickFolder,
  canOpen,
  busy,
  error,
  onPick,
  onOpen,
  onClose,
}: {
  current: Project | null
  recent: Project[]
  /** Sessions per project path. */
  counts: Record<string, number>
  canPickFolder: boolean
  canOpen: boolean
  busy: boolean
  error: string | null
  onPick: () => void
  onOpen: (path: string) => void
  onClose: () => void
}) {
  const [path, setPath] = useState('')
  const list = current && !recent.some((project) => project.path === current.path) ? [current, ...recent] : recent
  return (
    <Page
      title="Projects"
      onClose={onClose}
      actions={
        canPickFolder && (
          <button className="primary" onClick={onPick} disabled={busy}>
            Open folder…
          </button>
        )
      }
    >
      {!canOpen && <p className="muted">This Neuron host serves one project.</p>}
      <ul className="project-list" aria-label="Recent projects">
        {list.map((project) => {
          const isCurrent = project.path === current?.path
          return (
            <li key={project.path}>
              <button
                className={`project-row${isCurrent ? ' current' : ''}`}
                disabled={!canOpen || busy || isCurrent}
                aria-current={isCurrent ? 'true' : undefined}
                onClick={() => onOpen(project.path)}
              >
                <strong>{project.name}</strong>
                <span className="muted small">{project.path}</span>
                <span className="muted small">
                  {isCurrent ? 'Open now' : ''}
                  {counts[project.path] ? `${isCurrent ? ' · ' : ''}${counts[project.path]} sessions` : ''}
                </span>
              </button>
            </li>
          )
        })}
      </ul>
      {canOpen && !canPickFolder && (
        <form
          className="path-form"
          onSubmit={(event) => {
            event.preventDefault()
            if (path.trim()) onOpen(path.trim())
          }}
        >
          <input aria-label="Project folder" placeholder="C:\\path\\to\\project or /home/you/project" value={path} onChange={(event) => setPath(event.target.value)} />
          <button type="submit" className="primary" disabled={busy || !path.trim()}>
            Open
          </button>
        </form>
      )}
      {error && (
        <div className="error-line" role="alert">
          {error}
        </div>
      )}
    </Page>
  )
}

/** Every session on this device, searchable, in one table. */
export function AllSessionsPage({
  rows,
  groupBy,
  busy,
  onOpen,
  onClose,
}: {
  rows: SessionRow[]
  groupBy: GroupBy
  busy: boolean
  onOpen: (row: SessionRow) => void
  onClose: () => void
}) {
  const [query, setQuery] = useState('')
  const groups = groupRows(filterRows(rows, query), groupBy, null)
  return (
    <Page title="All sessions" onClose={onClose}>
      <input className="search" type="search" aria-label="Search all sessions" placeholder="Search by title" value={query} onChange={(event) => setQuery(event.target.value)} />
      {groups.length === 0 && <p className="muted">{query ? 'No session title matches.' : 'No sessions yet.'}</p>}
      {groups.map((group) => (
        <section key={group.key} aria-label={group.label} className="all-sessions-group">
          <h2>{group.label}</h2>
          <ul>
            {group.rows.map((row) => (
              <li key={row.id}>
                <button className={`all-session-row${row.current ? ' current' : ''}`} disabled={busy || row.current} onClick={() => onOpen(row)}>
                  <span className="session-title">{row.title}</span>
                  <span className="muted small">
                    {row.project.name} · {ago(row.updatedAtMs)} · {row.messageCount} messages{row.branch ? ` · ⎇ ${row.branch}` : ''}
                    {row.badge ? ` · ${BADGE_LABEL[row.badge]}` : ''}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      ))}
    </Page>
  )
}

export function ExtensionsPage({ children, onClose }: { children: ReactNode; onClose: () => void }) {
  return (
    <Page title="Extensions" onClose={onClose}>
      {children}
    </Page>
  )
}

/** "Neuron 0.2.1 is available", above the account row. */
export function UpdateNotice({ update, rail, onDownload, onDismiss }: { update: UpdateInfo; rail: boolean; onDownload: () => void; onDismiss: () => void }) {
  if (rail) {
    return (
      <button className="update-dot" aria-label={`Neuron ${update.version} is available. Download`} title={`Neuron ${update.version} is available`} onClick={onDownload}>
        ⬆
      </button>
    )
  }
  return (
    <div className="update-notice" role="status">
      <span>Neuron {update.version} is available.</span>
      <button className="link-button" onClick={onDownload}>
        Download <span aria-hidden>↗</span>
      </button>
      <button className="icon-button" aria-label="Dismiss the update notice" onClick={onDismiss}>
        ✕
      </button>
    </div>
  )
}
