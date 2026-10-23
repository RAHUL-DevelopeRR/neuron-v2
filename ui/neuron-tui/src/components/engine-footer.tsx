/**
 * One line under the input from the engine's last `status` and `usage`:
 *
 *   claude-sonnet-4-6 · workspace-write · main +1 ~2 ?3 · ↑12.4K ↓2.1K · ctx 4% · Zero-X pro · you@example.com
 *
 * Parts that don't fit the width are dropped, least important first (see
 * {@link FooterPart.priority}): the model and the Zero-X plan stay longest,
 * then the mode, an untrusted folder and the signed-in email; the token
 * counters go first. A long model id is cut short ("…") before any part but
 * the token counters is dropped.
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
  /** The fewest columns it may be cut to, with "…"; unset when it is never cut. */
  minWidth?: number
}

const SEPARATOR = ' · '
const ELLIPSIS = '…'

/** Which parts stay when they don't all fit: lowest first. */
const PRIORITY = {
  model: 0,
  account: 1,
  mode: 2,
  untrusted: 3,
  email: 4,
  context: 5,
  git: 6,
  tokens: 7,
} as const

/**
 * Parts at least this important have the model id cut short to fit them;
 * less important ones (the token counters) only take the room that is left.
 */
const CUT_MODEL_FOR = PRIORITY.git

/** A model id is not cut below this: the provider and the start of its name. */
export const MODEL_MIN_WIDTH = 16

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
    {
      text: status.model,
      tone: 'normal',
      priority: PRIORITY.model,
      minWidth: Math.min(status.model.length, MODEL_MIN_WIDTH),
    },
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

/** `text` in `width` columns: unchanged when it fits, else cut with "…". */
function cut(text: string, width: number): string {
  if (text.length <= width) return text
  return width <= 1 ? ELLIPSIS.slice(0, width) : text.slice(0, width - 1) + ELLIPSIS
}

/**
 * The parts that fit in `width` columns with their separators, in their
 * order: the most important ones first, then each less important one that
 * still fits. A part down to the email that does not fit has the cuttable
 * parts (the model id) cut short for it, down to their minimum.
 */
export function fitParts(parts: FooterPart[], width: number): FooterPart[] {
  const byImportance = parts
    .map((part, index) => ({ part, index }))
    .sort((a, b) => a.part.priority - b.part.priority || a.index - b.index)
  /** The width each kept part is drawn in. */
  const kept = new Map<number, number>()
  let used = 0
  const room = (index: number) => {
    const part = parts[index]!
    return (kept.get(index) ?? part.text.length) - (part.minWidth ?? part.text.length)
  }
  for (const { part, index } of byImportance) {
    const separator = kept.size > 0 ? SEPARATOR.length : 0
    const full = part.text.length
    let over = used + separator + full - width
    if (over <= 0) {
      kept.set(index, full)
      used += separator + full
      continue
    }
    if (part.priority > CUT_MODEL_FOR) continue
    // The part itself (a model id alone) and the kept cuttable parts.
    const own = full - (part.minWidth ?? full)
    const others = [...kept.keys()].filter((other) => room(other) > 0)
    const available = own + others.reduce((sum, other) => sum + room(other), 0)
    if (available < over) continue
    for (const other of others) {
      if (over <= 0) break
      const take = Math.min(room(other), over)
      kept.set(other, kept.get(other)! - take)
      used -= take
      over -= take
    }
    const drawn = full - Math.max(0, over)
    kept.set(index, drawn)
    used += separator + drawn
  }
  return parts
    .map((part, index) => ({ part, index }))
    .filter(({ index }) => kept.has(index))
    .map(({ part, index }) => {
      const drawn = kept.get(index)!
      return drawn < part.text.length ? { ...part, text: cut(part.text, drawn) } : part
    })
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
