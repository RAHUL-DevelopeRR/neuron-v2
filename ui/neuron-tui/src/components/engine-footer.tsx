/**
 * One line under the input from the engine's last `status` and `usage`:
 *
 *   claude-sonnet-4-6 · workspace-write · main +1 ~2 ?3 · ↑12.4K ↓2.1K · ctx 4% · Zero-X pro · you@example.com
 *
 * Parts that don't fit the width are dropped, least important first (see
 * {@link FooterPart.priority}): the model and the Zero-X plan stay longest,
 * the account's email goes first.
 */

import React from 'react'

import { useTheme } from '../hooks/use-theme'
import { needsSignIn, useEngineStore, type EngineStoreState } from '../state/engine-store'
import { formatTokenCount } from '../utils/format-token-count'

import type { GitStatus, StatusEvent } from '../engine/protocol'

export type FooterTone = 'normal' | 'muted' | 'warning'
export interface FooterPart {
  text: string
  tone: FooterTone
  /** Lower stays longer when the line is too narrow. */
  priority: number
}

const SEPARATOR = ' · '

/** Which parts stay when they don't all fit: lowest first. */
const PRIORITY = {
  model: 0,
  account: 1,
  mode: 2,
  untrusted: 3,
  context: 4,
  git: 5,
  tokens: 6,
  email: 7,
} as const

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

/**
 * The Zero-X login is usable: signed in, and the engine does not wait for a
 * sign-in. A stored login the gateway revoked still reads as signed in (it
 * has not expired), but the engine starts signed out with it.
 */
export function hasUsableLogin(status: StatusEvent): boolean {
  return status.signed_in === true && !needsSignIn(status)
}

export function footerParts(
  status: StatusEvent | null,
  usage: EngineStoreState['usage'],
): FooterPart[] {
  if (!status) return []
  const parts: FooterPart[] = [
    { text: status.model, tone: 'normal', priority: PRIORITY.model },
  ]
  parts.push({
    text: status.plan_mode ? `${status.permission_mode} · plan` : status.permission_mode,
    tone: 'muted',
    priority: PRIORITY.mode,
  })
  if (!status.workspace_trusted) {
    parts.push({ text: 'untrusted folder', tone: 'warning', priority: PRIORITY.untrusted })
  }
  if (status.git) {
    parts.push({ text: formatGit(status.git), tone: 'muted', priority: PRIORITY.git })
  }
  if (usage) {
    const total = usage.cumulative
    const input = total.input + total.cache_read + total.cache_write
    const mark = usage.estimated ? '~' : ''
    parts.push({
      text: `↑${mark}${formatTokenCount(input)} ↓${mark}${formatTokenCount(total.output)}`,
      tone: 'muted',
      priority: PRIORITY.tokens,
    })
  }
  if (status.session_id) {
    parts.push({
      text: `ctx ${status.context_percent}%`,
      tone: 'muted',
      priority: PRIORITY.context,
    })
  }
  const account = status.account
  if (hasUsableLogin(status)) {
    const plan = account?.plan
    parts.push({
      text: plan ? `Zero-X ${plan}` : 'Zero-X',
      tone: 'muted',
      priority: PRIORITY.account,
    })
    if (account?.email) {
      parts.push({ text: account.email, tone: 'muted', priority: PRIORITY.email })
    }
  } else if (account) {
    parts.push({
      text: 'Zero-X session expired: /login',
      tone: 'warning',
      priority: PRIORITY.account,
    })
  }
  return parts
}

/**
 * The parts that fit in `width` columns with their separators, in their
 * order: the most important ones first, then each less important one that
 * still fits.
 */
export function fitParts(parts: FooterPart[], width: number): FooterPart[] {
  const byImportance = parts
    .map((part, index) => ({ part, index }))
    .sort((a, b) => a.part.priority - b.part.priority || a.index - b.index)
  const kept = new Set<number>()
  let used = 0
  for (const { part, index } of byImportance) {
    const cost = (kept.size > 0 ? SEPARATOR.length : 0) + part.text.length
    if (used + cost > width) continue
    kept.add(index)
    used += cost
  }
  return parts.filter((_, index) => kept.has(index))
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
