/**
 * What a shared session tells the phone's sessions list, from the session as
 * the computer's host holds it. The computer seals this (./share-host.ts); the
 * relay only ever sees the sealed blob.
 */

import { openQuestionItem, type SessionState } from '@neuron/client'

import type { Presence, ShareStatus } from './contract'

const basename = (path: string) => path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path

export function shareStatus(state: SessionState): ShareStatus {
  if (state.permissions.length > 0 || openQuestionItem(state)) return 'needs_me'
  if (state.connection === 'exited') return 'failed'
  if (state.busy || state.sessionStarting) return 'running'
  const lastTurn = [...state.items].reverse().find((item) => item.kind === 'error' || item.kind === 'assistant' || item.kind === 'user')
  return lastTurn?.kind === 'error' ? 'failed' : 'idle'
}

export function presenceOf(state: SessionState, seq: number, fallbackCwd: string, now = Date.now()): Presence {
  const cwd = state.status?.cwd ?? fallbackCwd
  const current = state.sessions?.find((session) => session.id === state.status?.session_id)
  const firstPrompt = state.items.find((item) => item.kind === 'user')
  const title = current?.title ?? (firstPrompt?.kind === 'user' ? firstPrompt.text.replace(/\s+/g, ' ').trim().slice(0, 80) : null)
  return {
    v: 1,
    project: basename(cwd),
    title: title || null,
    branch: state.status?.git?.branch ?? null,
    model: state.status?.model ?? null,
    status: shareStatus(state),
    last_seq: seq,
    updated_at_ms: now,
  }
}
