/**
 * The sidebar's session list: sessions from every project the engine reports,
 * each with one badge, grouped by project, date or status.
 *
 * Engines that list sessions from all projects say which project each one
 * belongs to and whether another process (the terminal UI) has it open. Older
 * engines list only the open project's sessions, so those default to it.
 */

import type { SessionSummary } from '@neuron/client'

import type { Project } from './platform'

/** One badge per row, in this order of precedence. */
export type Badge = 'running' | 'terminal' | 'failed' | 'unread'

export const BADGE_LABEL: Record<Badge, string> = {
  running: 'running',
  terminal: 'open in the terminal',
  failed: 'failed',
  unread: 'unread',
}

export interface SessionRow {
  id: string
  title: string
  updatedAtMs: number
  messageCount: number
  branch: string | null
  project: Project
  current: boolean
  badge: Badge | null
  /** What the badge says to a screen reader and in a tooltip. */
  badgeLabel: string | null
}

export type GroupBy = 'project' | 'date' | 'status'

export interface SessionGroup {
  key: string
  label: string
  /** For project groups: the folder to open. */
  project: Project | null
  rows: SessionRow[]
}

/** What the app remembers between launches: when each session was last looked at, and which failed. */
export interface SeenState {
  seen: Record<string, number>
  failed: Record<string, boolean>
}

const projectName = (path: string) => path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path

/** A session's project: the folder the engine names (feature `session_scope`), else the open one. */
export function projectOf(session: SessionSummary, open: Project | null): Project {
  const path = session.project?.trim()
  if (path) return { name: projectName(path), path }
  return open ?? { name: 'This project', path: '' }
}

/** "running in the terminal", "open in another window": another Neuron has the session. */
function elsewhereLabel(session: SessionSummary): string | null {
  if (session.run_status !== 'running' && session.run_status !== 'open') return null
  const where = session.client === 'terminal' || !session.client ? 'in the terminal' : 'in another window'
  return `${session.run_status} ${where}`
}

const samePath = (a: string, b: string) => a.replace(/[\\/]+$/, '').toLowerCase() === b.replace(/[\\/]+$/, '').toLowerCase()

export function sessionRows(
  sessions: SessionSummary[],
  options: { open: Project | null; currentId: string | null; busy: boolean; seen: SeenState },
): SessionRow[] {
  return sessions
    .filter((session) => session.message_count > 0 || session.id === options.currentId)
    .map((session) => {
      const current = session.id === options.currentId
      const project = projectOf(session, options.open)
      const seenAt = options.seen.seen[session.id]
      const elsewhere = current ? null : elsewhereLabel(session)
      // The engine says when a session's last turn failed; this window also remembers its own.
      const failed = session.failed ?? options.seen.failed[session.id] ?? false
      const badge: Badge | null =
        current && options.busy
          ? 'running'
          : elsewhere
            ? 'terminal'
            : failed
              ? 'failed'
              : !current && seenAt !== undefined && session.updated_at_ms > seenAt
                ? 'unread'
                : null
      return {
        id: session.id,
        title: session.title?.trim() || (session.message_count === 0 ? 'New session' : 'Untitled session'),
        updatedAtMs: session.updated_at_ms,
        messageCount: session.message_count,
        branch: session.git_branch ?? session.branch,
        project,
        current,
        badge,
        badgeLabel: badge === 'terminal' ? elsewhere : badge ? BADGE_LABEL[badge] : null,
      }
    })
}

/** Rows whose title contains `query`, ignoring case. */
export function filterRows(rows: SessionRow[], query: string): SessionRow[] {
  const needle = query.trim().toLowerCase()
  return needle ? rows.filter((row) => row.title.toLowerCase().includes(needle)) : rows
}

const DAY = 24 * 60 * 60 * 1000

function dateLabel(ms: number, now: number): string {
  const start = new Date(now)
  start.setHours(0, 0, 0, 0)
  const today = start.getTime()
  if (ms >= today) return 'Today'
  if (ms >= today - DAY) return 'Yesterday'
  if (ms >= today - 6 * DAY) return 'This week'
  return 'Older'
}

const STATUS_ORDER: Array<[string, string]> = [
  ['active', 'Running'],
  ['failed', 'Failed'],
  ['unread', 'Unread'],
  ['idle', 'Everything else'],
]

/**
 * Groups, newest activity first. By project, the open project is always there
 * (even with no sessions yet), and other projects follow their latest session.
 */
export function groupRows(rows: SessionRow[], by: GroupBy, open: Project | null, now = Date.now()): SessionGroup[] {
  const sorted = [...rows].sort((a, b) => b.updatedAtMs - a.updatedAtMs)
  if (by === 'date') {
    const groups = new Map<string, SessionRow[]>()
    for (const row of sorted) {
      const label = dateLabel(row.updatedAtMs, now)
      groups.set(label, [...(groups.get(label) ?? []), row])
    }
    return [...groups].map(([label, group]) => ({ key: `date:${label}`, label, project: null, rows: group }))
  }
  if (by === 'status') {
    const bucket = (row: SessionRow) =>
      row.badge === 'running' || row.badge === 'terminal' ? 'active' : row.badge === 'failed' ? 'failed' : row.badge === 'unread' ? 'unread' : 'idle'
    return STATUS_ORDER.map(([key, label]) => ({ key: `status:${key}`, label, project: null, rows: sorted.filter((row) => bucket(row) === key) })).filter(
      (group) => group.rows.length > 0,
    )
  }
  const groups: SessionGroup[] = []
  for (const row of sorted) {
    let group = groups.find((entry) => entry.project && samePath(entry.project.path, row.project.path))
    if (!group) {
      group = { key: `project:${row.project.path}`, label: row.project.name, project: row.project, rows: [] }
      groups.push(group)
    }
    group.rows.push(row)
  }
  if (open && !groups.some((group) => group.project && samePath(group.project.path, open.path))) {
    groups.unshift({ key: `project:${open.path}`, label: open.name, project: open, rows: [] })
  }
  return groups
}

/**
 * The seen state after a new session list: sessions seen for the first time
 * are not unread, and the open session is always seen up to now.
 */
export function noteSeen(state: SeenState, sessions: SessionSummary[], currentId: string | null): SeenState {
  let seen = state.seen
  for (const session of sessions) {
    const known = seen[session.id]
    if (known === undefined || (session.id === currentId && known < session.updated_at_ms)) {
      seen = { ...seen, [session.id]: session.updated_at_ms }
    }
  }
  return seen === state.seen ? state : { ...state, seen }
}

export function ago(ms: number, now = Date.now()): string {
  const seconds = Math.max(0, Math.round((now - ms) / 1000))
  if (seconds < 60) return 'now'
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`
  if (seconds < 86_400) return `${Math.floor(seconds / 3600)}h`
  return `${Math.floor(seconds / 86_400)}d`
}
