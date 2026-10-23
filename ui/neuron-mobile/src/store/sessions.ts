/**
 * The Sessions tab: every shared session on every paired computer, grouped by
 * project, with the badges the desktop sidebar uses (running, needs you,
 * failed, unread). Pure, so it is tested without the app.
 */

import type { Presence, ShareStatus } from '../relay/contract'
import type { HostView } from '../relay/phone'

export type Badge = 'running' | 'needs_me' | 'failed' | 'unread' | null

export interface SessionRow {
  key: string
  hostId: string
  shareId: string
  computer: string
  title: string
  branch: string | null
  status: ShareStatus
  badge: Badge
  updatedAt: number
}

export interface ProjectGroup {
  project: string
  rows: SessionRow[]
}

export const shareKey = (hostId: string, shareId: string) => `${hostId}/${shareId}`

function badgeOf(presence: Presence, viewed: number | undefined): Badge {
  if (presence.status === 'needs_me') return 'needs_me'
  if (presence.status === 'running') return 'running'
  if (presence.status === 'failed') return 'failed'
  // Idle: unread when something happened after the phone last looked.
  return viewed === undefined || presence.last_seq > viewed ? 'unread' : null
}

const ORDER: Record<string, number> = { needs_me: 0, running: 1, failed: 2, unread: 3 }

/** Groups the shared sessions by project: projects with something that needs the user first, then by recency. */
export function groupSessions(hosts: HostView[], viewed: Record<string, number>): ProjectGroup[] {
  const groups = new Map<string, SessionRow[]>()
  for (const host of hosts) {
    if (!host.online) continue
    for (const share of host.shares) {
      const presence = share.presence
      if (!presence) continue
      const key = shareKey(host.hostId, share.shareId)
      const row: SessionRow = {
        key,
        hostId: host.hostId,
        shareId: share.shareId,
        computer: host.name,
        title: presence.title || 'New session',
        branch: presence.branch,
        status: presence.status,
        badge: badgeOf(presence, viewed[key]),
        updatedAt: presence.updated_at_ms || share.at,
      }
      const rows = groups.get(presence.project) ?? []
      rows.push(row)
      groups.set(presence.project, rows)
    }
  }
  const rank = (row: SessionRow) => (row.badge ? ORDER[row.badge] : 4)
  return [...groups.entries()]
    .map(([project, rows]) => ({ project, rows: rows.sort((a, b) => rank(a) - rank(b) || b.updatedAt - a.updatedAt) }))
    .sort((a, b) => rank(a.rows[0]) - rank(b.rows[0]) || b.rows[0].updatedAt - a.rows[0].updatedAt || a.project.localeCompare(b.project))
}

/** "just now", "5 min ago", "3 h ago", or a date. */
export function ago(at: number, now = Date.now()): string {
  const seconds = Math.max(0, Math.round((now - at) / 1000))
  if (seconds < 45) return 'just now'
  const minutes = Math.round(seconds / 60)
  if (minutes < 60) return `${minutes} min ago`
  const hours = Math.round(minutes / 60)
  if (hours < 24) return `${hours} h ago`
  return new Date(at).toLocaleDateString()
}
