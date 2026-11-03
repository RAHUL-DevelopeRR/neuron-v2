import { useMemo, useState } from 'react'

import type { SessionSummary } from '@neuron/client'

import type { Project } from '../platform'
import { PixelLogo } from './PixelLogo'

const DAY = 24 * 60 * 60 * 1000

/** Chats by when they last changed: Today, Yesterday, This week, Older. */
export function groupChats(sessions: SessionSummary[], now = Date.now()): Array<[string, SessionSummary[]]> {
  const startOfToday = new Date(now)
  startOfToday.setHours(0, 0, 0, 0)
  const today = startOfToday.getTime()
  const groups = new Map<string, SessionSummary[]>()
  for (const session of sessions) {
    const label =
      session.updated_at_ms >= today
        ? 'Today'
        : session.updated_at_ms >= today - DAY
          ? 'Yesterday'
          : session.updated_at_ms >= today - 6 * DAY
            ? 'This week'
            : 'Older'
    groups.set(label, [...(groups.get(label) ?? []), session])
  }
  return [...groups]
}

export function ago(ms: number, now = Date.now()): string {
  const seconds = Math.max(0, Math.round((now - ms) / 1000))
  if (seconds < 60) return 'now'
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`
  if (seconds < 86_400) return `${Math.floor(seconds / 3600)}h`
  return `${Math.floor(seconds / 86_400)}d`
}

/** The project, a new chat, and the project's chats to search and resume. */
export function Sidebar({
  project,
  sessions,
  currentId,
  busy,
  historyAvailable,
  canOpenProject,
  onNewChat,
  onResume,
  onOpenProject,
  onSettings,
}: {
  project: Project | null
  sessions: SessionSummary[] | null
  currentId: string | null
  busy: boolean
  historyAvailable: boolean
  canOpenProject: boolean
  onNewChat: () => void
  onResume: (id: string) => void
  onOpenProject: () => void
  onSettings: () => void
}) {
  const [query, setQuery] = useState('')
  const shown = useMemo(() => {
    const needle = query.trim().toLowerCase()
    const list = (sessions ?? []).filter((session) => session.message_count > 0 || session.id === currentId)
    return needle ? list.filter((session) => (session.title ?? session.id).toLowerCase().includes(needle)) : list
  }, [sessions, query, currentId])

  return (
    <nav className="sidebar" aria-label="Chats">
      <div className="sidebar-head">
        <PixelLogo pixel={3} />
        <button className="icon-button" title="Settings" aria-label="Settings" onClick={onSettings}>
          ⚙
        </button>
      </div>
      <button
        className="project-button"
        onClick={onOpenProject}
        disabled={!canOpenProject}
        title={project?.path ?? 'No project'}
        aria-label="Project"
      >
        <span className="project-name">{project?.name ?? 'No project'}</span>
        {canOpenProject && <span className="muted">Switch…</span>}
      </button>
      <button className="new-chat" onClick={onNewChat} disabled={busy}>
        + New chat <kbd>Ctrl+N</kbd>
      </button>
      {historyAvailable ? (
        <>
          <input
            className="search"
            type="search"
            aria-label="Search chats"
            placeholder="Search chats"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
          <div className="chat-list">
            {sessions === null && <div className="muted pad">Loading chats…</div>}
            {sessions !== null && shown.length === 0 && (
              <div className="muted pad">{query ? 'No chat matches.' : 'No chats in this project yet.'}</div>
            )}
            {groupChats(shown).map(([label, group]) => (
              <section key={label}>
                <h3 className="chat-group">{label}</h3>
                <ul>
                  {group.map((session) => (
                    <li key={session.id}>
                      <button
                        className={`chat-row${session.id === currentId ? ' current' : ''}`}
                        aria-current={session.id === currentId ? 'true' : undefined}
                        disabled={busy && session.id !== currentId}
                        onClick={() => session.id !== currentId && onResume(session.id)}
                        title={session.title ?? session.id}
                      >
                        <span className="chat-title">{session.title ?? 'Untitled chat'}</span>
                        <span className="chat-meta">
                          {ago(session.updated_at_ms)} · {session.message_count} msgs
                          {session.branch ? ` · ⎇ ${session.branch}` : ''}
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              </section>
            ))}
          </div>
        </>
      ) : (
        <div className="muted pad">This engine has no chat history; update Neuron for it.</div>
      )}
    </nav>
  )
}
