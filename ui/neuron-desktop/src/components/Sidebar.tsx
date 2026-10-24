import { forwardRef, useState, type ReactNode } from 'react'

import { ago, filterRows, groupRows, type Badge, type GroupBy, type SessionRow } from '../sessions'
import type { Project } from '../platform'
import { PixelLogo } from './PixelLogo'

export type NavTarget = 'projects' | 'extensions' | 'sessions'

const BADGE_MARK: Record<Badge, string> = { running: '●', terminal: '⌨', failed: '✕', unread: '•' }

const GROUP_LABEL: Record<GroupBy, string> = { project: 'Project', date: 'Date', status: 'Status' }

function NavButton({
  icon,
  label,
  shortcut,
  rail,
  active,
  onClick,
  disabled,
}: {
  icon: string
  label: string
  shortcut?: string
  rail: boolean
  active?: boolean
  onClick: () => void
  disabled?: boolean
}) {
  return (
    <button
      className={`nav-item${active ? ' active' : ''}`}
      onClick={onClick}
      disabled={disabled}
      aria-label={rail ? label : undefined}
      aria-current={active ? 'page' : undefined}
      title={rail ? (shortcut ? `${label} (${shortcut})` : label) : undefined}
    >
      <span className="nav-icon" aria-hidden>
        {icon}
      </span>
      {!rail && <span className="nav-label">{label}</span>}
      {!rail && shortcut && <kbd>{shortcut}</kbd>}
    </button>
  )
}

function SessionButton({ row, disabled, onOpen }: { row: SessionRow; disabled: boolean; onOpen: (row: SessionRow) => void }) {
  return (
    <button
      className={`session-row${row.current ? ' current' : ''}${row.badge === 'unread' ? ' unread' : ''}`}
      aria-current={row.current ? 'true' : undefined}
      disabled={disabled}
      onClick={() => !row.current && onOpen(row)}
      title={row.badgeLabel ? `${row.title} (${row.badgeLabel})` : row.title}
    >
      <span className={`session-badge${row.badge ? ` badge-${row.badge}` : ''}`} aria-hidden>
        {row.badge ? BADGE_MARK[row.badge] : ''}
      </span>
      <span className="session-title">{row.title}</span>
      {row.badgeLabel && <span className="sr-only">, {row.badgeLabel}</span>}
      <span className="session-age">{ago(row.updatedAtMs)}</span>
    </button>
  )
}

/** The group-by menu next to "On this device". */
function GroupMenu({ value, onChange }: { value: GroupBy; onChange: (value: GroupBy) => void }) {
  return (
    <label className="group-by">
      <span className="sr-only">Group sessions by</span>
      <select aria-label="Group sessions by" value={value} onChange={(event) => onChange(event.target.value as GroupBy)}>
        {(Object.keys(GROUP_LABEL) as GroupBy[]).map((key) => (
          <option key={key} value={key}>
            {GROUP_LABEL[key]}
          </option>
        ))}
      </select>
    </label>
  )
}

/**
 * Navigation, the sessions on this device grouped by project, and the account
 * row. As an icon rail it keeps the navigation and the account initials.
 */
export const Sidebar = forwardRef<HTMLInputElement, {
  rail: boolean
  onToggleRail: () => void
  rows: SessionRow[] | null
  open: Project | null
  groupBy: GroupBy
  onGroupBy: (value: GroupBy) => void
  busy: boolean
  historyAvailable: boolean
  page: NavTarget | 'chat' | 'settings'
  onNewSession: () => void
  onOpenSession: (row: SessionRow) => void
  onOpenProject: (project: Project) => void
  onNavigate: (target: NavTarget) => void
  onSettings: () => void
  onSearch: () => void
  /** The account row, and the update notice above it. */
  footer: ReactNode
}>(function Sidebar(
  { rail, onToggleRail, rows, open, groupBy, onGroupBy, busy, historyAvailable, page, onNewSession, onOpenSession, onOpenProject, onNavigate, onSettings, onSearch, footer },
  searchRef,
) {
  const [query, setQuery] = useState('')
  const [folded, setFolded] = useState<Record<string, boolean>>({})
  const shown = rows ? filterRows(rows, query) : []
  const groups = rows ? groupRows(shown, groupBy, query ? null : open) : []

  return (
    <nav className={`sidebar${rail ? ' rail' : ''}`} aria-label="Neuron">
      <div className="sidebar-head">
        {rail ? <PixelLogo pixel={4} mark /> : <PixelLogo pixel={3} />}
        {!rail && (
          <button className="icon-button" aria-label="Settings" title="Settings (Ctrl+,)" onClick={onSettings}>
            ⚙
          </button>
        )}
        <button
          className="icon-button"
          aria-label={rail ? 'Expand the sidebar' : 'Collapse the sidebar'}
          aria-expanded={!rail}
          title={rail ? 'Expand the sidebar (Ctrl+Shift+B)' : 'Collapse the sidebar (Ctrl+Shift+B)'}
          onClick={onToggleRail}
        >
          {rail ? '»' : '«'}
        </button>
      </div>
      <div className="nav" role="group" aria-label="Go to">
        <NavButton icon="＋" label="New session" shortcut="Ctrl+N" rail={rail} onClick={onNewSession} disabled={busy} />
        <NavButton icon="⌕" label="Search" shortcut="Ctrl+K" rail={rail} onClick={onSearch} />
        <NavButton icon="▣" label="Projects" rail={rail} active={page === 'projects'} onClick={() => onNavigate('projects')} />
        <NavButton icon="⧉" label="Extensions" rail={rail} active={page === 'extensions'} onClick={() => onNavigate('extensions')} />
        {rail && <NavButton icon="⚙" label="Settings" shortcut="Ctrl+," rail onClick={onSettings} active={page === 'settings'} />}
      </div>
      {!rail && (
        <>
          <input
            ref={searchRef}
            className="search"
            type="search"
            aria-label="Search sessions"
            placeholder="Search sessions in every project"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => event.key === 'Escape' && setQuery('')}
          />
          <div className="session-list">
            <div className="section-head">
              <h2 id="on-this-device">On this device</h2>
              {historyAvailable && <GroupMenu value={groupBy} onChange={onGroupBy} />}
            </div>
            {!historyAvailable && <div className="muted pad small">This engine keeps no session history; update Neuron for it.</div>}
            {historyAvailable && rows === null && <div className="muted pad small">Loading sessions…</div>}
            {historyAvailable && rows !== null && query && shown.length === 0 && <div className="muted pad small">No session title matches.</div>}
            <ul aria-labelledby="on-this-device">
              {groups.map((group) => {
                const isFolded = folded[group.key] === true
                const listId = `group-${group.key.replace(/[^a-z0-9]+/gi, '-')}`
                const isOpen = group.project && open && group.project.path === open.path
                return (
                  <li key={group.key} className="session-group">
                    <div className="group-head">
                      <button
                        className="group-toggle"
                        aria-expanded={!isFolded}
                        aria-controls={listId}
                        onClick={() => setFolded({ ...folded, [group.key]: !isFolded })}
                        title={group.project?.path}
                      >
                        <span aria-hidden>{isFolded ? '▸' : '▾'}</span> {group.label}
                        {group.rows.length > 0 && <span className="muted"> {group.rows.length}</span>}
                      </button>
                      {group.project && !isOpen && (
                        <button className="link-button small" disabled={busy} onClick={() => onOpenProject(group.project!)} aria-label={`Open ${group.label}`}>
                          Open
                        </button>
                      )}
                    </div>
                    {!isFolded && (
                      <ul id={listId}>
                        {group.rows.length === 0 && <li className="muted small empty-group">No sessions yet</li>}
                        {group.rows.map((row) => (
                          <li key={row.id}>
                            <SessionButton row={row} disabled={busy && !row.current} onOpen={onOpenSession} />
                          </li>
                        ))}
                      </ul>
                    )}
                  </li>
                )
              })}
            </ul>
          </div>
          <button className={`link-button all-sessions${page === 'sessions' ? ' active' : ''}`} onClick={() => onNavigate('sessions')}>
            All sessions
          </button>
        </>
      )}
      {rail && <div className="spacer" />}
      {footer}
    </nav>
  )
})
