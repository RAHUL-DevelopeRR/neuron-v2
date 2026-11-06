/**
 * Hook commands as the UI shows them (`hook_progress`). While one runs, the
 * status line says so (./live-turn.ts); when one blocks or fails, a line in
 * the chat says which hook, what it said and how long it took. Hooks that
 * pass stay quiet.
 */

import type { HookFinishedEvent } from './protocol'

/** The chat line for a finished hook, or `null` when it passed. */
export function hookFinishedLine(event: HookFinishedEvent): string | null {
  if (event.outcome === 'ok') return null
  const what = {
    blocked: 'blocked',
    error: 'failed',
    cancelled: 'was cancelled',
  }[event.outcome]
  const scope = event.matcher ? ` (${event.matcher})` : ''
  const said = event.message?.trim() ? `: ${event.message.trim()}` : ''
  return `${event.event} hook${scope} ${what}${said} · ${event.command} · ${formatHookDuration(event.duration_ms)}`
}

/** `840 ms`, `1.2 s`. */
export function formatHookDuration(ms: number): string {
  if (ms < 1000) return `${Math.max(0, Math.round(ms))} ms`
  return `${(ms / 1000).toFixed(1)} s`
}
