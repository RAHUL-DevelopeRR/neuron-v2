import { forwardRef, useState, type ReactNode } from 'react'

import { ago, filterRows, groupRows, type Badge, type GroupBy, type SessionRow } from '../sessions'
import { triage, type Activity, type ChatRow } from '../chats'
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

const ACTIVITY_MARK: Record<Activity, string> = { needs_you: '!', running: '●', idle: '○', stopped: '■' }
const ACTIVITY_LABEL: Record<Activity, string> = { needs_you: 'needs you', running: 'working', idle: 'idle', stopped: 'stopped' }

/** What a chat's row says to a screen reader beyond its title. */
function chatStatus(row: ChatRow): string {
  if (row.activity === 'idle' && row.unread) return 'finished, not seen yet'
  return row.stopNote ? `${ACTIVITY_LABEL[row.activity]}: ${row.stopNote}` : ACTIVITY_LABEL[row.activity]
}

function ChatButton({ row, active, rail, onOpen }: { row: ChatRow; active: boolean; rail: boolean; onOpen: (id: string) => void }) {
  const status = chatStatus(row)
  return (
    <button
      data-chat-id={row.id}
      className={`chat-row activity-${row.activity}${active ? ' current' : ''}${row.unread ? ' unread' : ''}`}
      aria-current={active ? 'true' : undefined}
      aria-label={rail ? `${row.title}, ${status}` : undefined}
      title={rail ? `${row.title} (${status})` : row.stopNote ?? row.title}
      onClick={() => onOpen(row.id)}
    >
      <span className="chat-dot" aria-hidden>
        {row.unread && row.activity !== 'needs_you' && row.activity !== 'running' ? '•' : ACTIVITY_MARK[row.activity]}
      </span>
      {!rail && (
        <>
          <span className="session-title">{row.title}</span>
          {row.worktree && row.branch && <span className="chat-tag muted small">{row.branch.replace(/^neuron\//, '')}</span>}
          <span className="sr-only">, {status}</span>
        </>
      )}
    </button>
  )
}

const CHAT_GROUPS: Array<['needsYou' | 'running' | 'doneUnread' | 'idle', string]> = [
  ['needsYou', 'Needs you'],
  ['running', 'Working'],
  ['doneUnread', 'Finished'],
  ['idle', 'Idle'],
]

/** The open chats in four groups: waiting for the user, working, finished out of view, and the rest. */
function ChatGroups({
  chats,
  active,
  rail,
  onOpen,
  onClose,
}: {
  chats: ChatRow[]
  active: string | null
  rail: boolean
  onOpen: (id: string) => void
  onClose?: (id: string) => void
}) {
  const groups = triage(chats)
  return (
    <div className="chat-groups">
      {CHAT_GROUPS.map(([key, label]) =>
        groups[key].length === 0 ? null : (
          <section key={key} className={`chat-group group-${key}`} aria-label={`${label}, ${groups[key].length}`}>
            {!rail && <h2 className="chat-group-head">{label}</h2>}
            <ul>
              {groups[key].map((row) => (
                <li key={row.id} className="chat-item">
                  <ChatButton row={row} active={row.id === active} rail={rail} onOpen={onOpen} />
                  {onClose && !rail && (
                    <button className="icon-button chat-close" aria-label={`Close chat: ${row.title}`} title="Close this chat" onClick={() => onClose(row.id)}>
                      ✕
                    </button>
                  )}
                </li>
              ))}
            </ul>
          </section>
        ),
      )}
    </div>
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
  // L4: multi-chat. With `chats`, the open chats are listed in groups above the history.
  chats?: ChatRow[]
  activeChat?: string | null
  onOpenChat?: (id: string) => void
  needsYouCount?: number
  onJumpWaiting?: () => void
  onNewWorktreeChat?: () => void
  /** Closes a chat from its row (a chat with a copy of its own asks first). */
  onCloseChat?: (id: string) => void
}>(function Sidebar(
  { rail, onToggleRail, rows, open, groupBy, onGroupBy, busy, historyAvailable, page, onNewSession, onOpenSession, onOpenProject, onNavigate, onSettings, onSearch, footer, chats, activeChat = null, onOpenChat, needsYouCount = 0, onJumpWaiting, onNewWorktreeChat, onCloseChat },
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
        {needsYouCount > 0 && onJumpWaiting && (
          <button
            className="needs-badge"
            onClick={onJumpWaiting}
            aria-label={`${needsYouCount} ${needsYouCount === 1 ? 'chat needs' : 'chats need'} you. Go to the first`}
            title="Go to the chat that needs you (Ctrl+J)"
          >
            {needsYouCount}
          </button>
        )}
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
        {onNewWorktreeChat && <NavButton icon="⎇" label="New chat in a copy" shortcut="Ctrl+Shift+N" rail={rail} onClick={onNewWorktreeChat} />}
        <NavButton icon="⌕" label="Search" rail={rail} onClick={onSearch} />
        <NavButton icon="▣" label="Projects" rail={rail} active={page === 'projects'} onClick={() => onNavigate('projects')} />
        <NavButton icon="⧉" label="Extensions" rail={rail} active={page === 'extensions'} onClick={() => onNavigate('extensions')} />
        {rail && <NavButton icon="⚙" label="Settings" shortcut="Ctrl+," rail onClick={onSettings} active={page === 'settings'} />}
      </div>
      {chats && chats.length > 0 && <ChatGroups chats={chats} active={activeChat} rail={rail} onOpen={onOpenChat ?? (() => {})} onClose={onCloseChat} />}
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
