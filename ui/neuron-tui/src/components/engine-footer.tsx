/**
 * One line under the input from the engine's last `status` and `usage`:
 *
 *   claude-sonnet-4-6 · workspace-write · main +1 ~2 ?3 · ↑12.4K ↓2.1K · ctx 4% · Zero-X pro
 *
 * Parts that don't fit the width are dropped from the end.
 */

import React from 'react'

import { useTheme } from '../hooks/use-theme'
import { useEngineStore, type EngineStoreState } from '../state/engine-store'
import { formatTokenCount } from '../utils/format-token-count'

import type { GitStatus, StatusEvent } from '../engine/protocol'

export type FooterTone = 'normal' | 'muted' | 'warning'
export interface FooterPart {
  text: string
  tone: FooterTone
}

const SEPARATOR = ' · '

/** `main +1 ~2 ?3 !1`: staged, unstaged, untracked, conflicted (non-zero only). */
export function formatGit(git: GitStatus): string {
  const counts = [
    ['+', git.staged],
    ['~', git.unstaged],
    ['?', git.untracked],
    ['!', git.conflicted],
  ] as const
  const changes = counts
    .filter(([, count]) => count > 0)
    .map(([mark, count]) => `${mark}${count}`)
  return [git.branch ?? 'detached', ...changes].join(' ')
}

export function footerParts(
  status: StatusEvent | null,
  usage: EngineStoreState['usage'],
): FooterPart[] {
  if (!status) return []
  const parts: FooterPart[] = [{ text: status.model, tone: 'normal' }]
  parts.push({
    text: status.plan_mode ? `${status.permission_mode} · plan` : status.permission_mode,
    tone: 'muted',
  })
  if (!status.workspace_trusted) parts.push({ text: 'untrusted folder', tone: 'warning' })
  if (status.git) parts.push({ text: formatGit(status.git), tone: 'muted' })
  if (usage) {
    const total = usage.cumulative
    const input = total.input + total.cache_read + total.cache_write
    const mark = usage.estimated ? '~' : ''
    parts.push({
      text: `↑${mark}${formatTokenCount(input)} ↓${mark}${formatTokenCount(total.output)}`,
      tone: 'muted',
    })
  }
  if (status.session_id) parts.push({ text: `ctx ${status.context_percent}%`, tone: 'muted' })
  if (status.signed_in) {
    const plan = status.account?.plan
    parts.push({ text: plan ? `Zero-X ${plan}` : 'Zero-X', tone: 'muted' })
  } else if (status.account) {
    parts.push({ text: 'Zero-X session expired: /login', tone: 'warning' })
  }
  return parts
}

/** The leading parts that fit in `width` columns, with their separators. */
export function fitParts(parts: FooterPart[], width: number): FooterPart[] {
  const kept: FooterPart[] = []
  let used = 0
  for (const part of parts) {
    const cost = (kept.length > 0 ? SEPARATOR.length : 0) + part.text.length
    if (used + cost > width) break
    kept.push(part)
    used += cost
  }
  return kept
}

export const EngineFooter = ({ width }: { width: number }) => {
  const theme = useTheme()
  const status = useEngineStore((state) => state.status)
  const usage = useEngineStore((state) => state.usage)
  const parts = fitParts(footerParts(status, usage), Math.max(0, width - 2))
  if (parts.length === 0) return null

  const color = (tone: FooterTone) =>
    tone === 'warning' ? theme.warning : tone === 'muted' ? theme.muted : theme.foreground

  return (
    <box style={{ paddingLeft: 1, paddingRight: 1, flexShrink: 0 }}>
      <text style={{ wrapMode: 'none' }}>
        {parts.map((part, index) => (
          <span key={index} fg={color(part.tone)}>
            {index > 0 ? SEPARATOR : ''}
            {part.text}
          </span>
        ))}
      </text>
    </box>
  )
}
