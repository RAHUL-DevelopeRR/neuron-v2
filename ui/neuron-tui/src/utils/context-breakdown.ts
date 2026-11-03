/**
 * How full the context window is, and what fills it, from the engine's
 * `status`: the status bar's meter and the context popover's rows.
 *
 * Engines with feature `status_bar` send `status.context`, the window split
 * into the system prompt, tool schemas, project instructions, conversation
 * and tool outputs; the fill is what a request sends over what it may use
 * (`context_window - output_reserve`). Older engines send only
 * `context_percent`, the conversation's share of what was left after the
 * system prompt and tools: it is shown clamped to 0–100 and marked `~`.
 */

import { clampPercent, DEFAULT_COMPACT_THRESHOLD_PERCENT } from '../components/context-meter'

import type { ContextBreakdown, StatusEvent } from '../engine/protocol'

export interface ContextUsage {
  /** 0–100. */
  percent: number
  /** Read from the old `context_percent`, not the breakdown. */
  estimated: boolean
  /** Where auto-compaction starts, as a share of the usable window (0–100). */
  thresholdPercent: number
  /** What a request may use (`context_window - output_reserve`), when known. */
  usable: number | null
  /** What a request sends now, when known. */
  used: number | null
  breakdown: ContextBreakdown | null
}

/** The usable window of a breakdown: the window less the reply's reserve. */
export function usableTokens(context: ContextBreakdown): number {
  return Math.max(0, context.context_window - context.output_reserve)
}

/** The parts of a breakdown summed: what a request sends. */
export function sentTokens(context: ContextBreakdown): number {
  return (
    context.system_tokens +
    context.tool_schema_tokens +
    context.instruction_tokens +
    context.conversation_tokens +
    context.tool_output_tokens
  )
}

/** The fill for the meter, or `null` before the engine has a session. */
export function contextUsage(status: StatusEvent | null): ContextUsage | null {
  if (!status) return null
  const context = status.context
  if (context) {
    const usable = usableTokens(context)
    const used = context.used_tokens > 0 ? context.used_tokens : sentTokens(context)
    const percent = usable > 0 ? clampPercent((used / usable) * 100) : used > 0 ? 100 : 0
    const thresholdPercent =
      usable > 0 && context.threshold_tokens > 0
        ? clampPercent((context.threshold_tokens / usable) * 100)
        : DEFAULT_COMPACT_THRESHOLD_PERCENT
    return { percent, estimated: false, thresholdPercent, usable, used, breakdown: context }
  }
  if (!status.session_id) return null
  return {
    percent: clampPercent(status.context_percent),
    estimated: true,
    thresholdPercent: DEFAULT_COMPACT_THRESHOLD_PERCENT,
    usable: null,
    used: null,
    breakdown: null,
  }
}

export interface BreakdownRow {
  label: string
  tokens: number | null
  /** Share of the usable window (0–100). */
  percent: number
  /** Files behind the row (the instruction files), when there are any. */
  detail: string | null
  /** The `Free` row. */
  free?: boolean
}

/** At most this many instruction files are named on their row. */
const MAX_NAMED_FILES = 2

function shortFileName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean)
  return parts.slice(-2).join('/') || path
}

/**
 * The popover's rows: system prompt, tools, project instructions,
 * conversation, tool outputs and what is free. With a breakdown they add up
 * to the usable window (the free row never goes below zero). Without one
 * there is a single estimated conversation row.
 */
export function breakdownRows(usage: ContextUsage): BreakdownRow[] {
  const context = usage.breakdown
  if (!context || usage.usable === null) {
    return [{ label: 'Conversation', tokens: null, percent: usage.percent, detail: null }]
  }
  const usable = usage.usable
  const share = (tokens: number) => (usable > 0 ? (tokens / usable) * 100 : 0)
  const files = context.instruction_files
  const named = files.slice(0, MAX_NAMED_FILES).map(shortFileName)
  const detail =
    files.length === 0
      ? null
      : files.length > MAX_NAMED_FILES
        ? `${named.join(', ')} +${files.length - MAX_NAMED_FILES}`
        : named.join(', ')
  const rows: BreakdownRow[] = [
    { label: 'System prompt', tokens: context.system_tokens, percent: share(context.system_tokens), detail: null },
    {
      label: `Tools (${context.tool_count})`,
      tokens: context.tool_schema_tokens,
      percent: share(context.tool_schema_tokens),
      detail: null,
    },
    {
      label: 'Project instructions & memory',
      tokens: context.instruction_tokens,
      percent: share(context.instruction_tokens),
      detail,
    },
    {
      label: 'Conversation',
      tokens: context.conversation_tokens,
      percent: share(context.conversation_tokens),
      detail: null,
    },
    {
      label: 'Tool outputs',
      tokens: context.tool_output_tokens,
      percent: share(context.tool_output_tokens),
      detail: null,
    },
  ]
  const free = Math.max(0, usable - sentTokens(context))
  rows.push({ label: 'Free', tokens: free, percent: share(free), detail: null, free: true })
  return rows
}
