/**
 * What fills the context window, and the Zero-X account's limits: the panel
 * the status bar's meter opens (click, Alt+C). Esc closes it.
 *
 *   Context · Gemini 2.5 Flash · 1M window
 *   ●●●◑○○○○○○ 35%  of 983K usable · 65.5K kept for the reply
 *     System prompt                   4.1K    0.4%
 *     Tools (23)                     11.8K    1.2%
 *     ...
 *     Free                            679K   69.0%
 *     Auto-compacts at 80% (786K) · /compact now
 *   This session  ↑12.4K ↓2.1K
 *   Account · Free (dev@example.com)
 *     5-hour window   120K / 1M tokens  ●○○○○○○○○○ 12%  resets 14:32 (in 3h 10m)
 *     Today           37 / 100 requests                 resets 05:30
 *     /usage details · /upgrade
 *
 * The account rows are read again when it opens and every minute while it is
 * open (`refresh_account`).
 *
 * In a short terminal (`compact`) it keeps the meter, the free share, the
 * auto-compact line and the account's title, and points at /context for the
 * rest; `maxRows` caps its body either way, so its border, the `esc close`
 * line and the status bar under it stay on screen.
 */

import { type KeyEvent } from '@opentui/core'
import { useKeyboard } from '@opentui/react'
import React, { useCallback, useEffect } from 'react'
import stringWidth from 'string-width'

import { ContextMeter } from './context-meter'
import { useTheme } from '../hooks/use-theme'
import { liveFooterTokens } from '../engine/live-turn'
import { refreshAccount } from '../engine/status-bar-actions'
import { useEngineStore } from '../state/engine-store'
import { useLiveTurnStore } from '../state/live-turn-store'
import { accountRows, accountTitle, sessionTokens, updatedAgo } from '../utils/account-usage'
import { breakdownRows, contextUsage, type BreakdownRow } from '../utils/context-breakdown'
import { formatTokenCount } from '../utils/format-token-count'
import { catalogModel, hasUsableZeroXLogin } from '../utils/status-bar-parts'
import { cleanModelName, distinctModelName } from '../utils/model-name'
import { BORDER_CHARS } from '../utils/ui-constants'

import type { StatusEvent } from '../engine/protocol'

export const ACCOUNT_REFRESH_MS = 60_000

const LABEL_COLUMNS = 30
/** A 10-cell meter and " 12%", so the reset times line up. */
const ACCOUNT_METER_COLUMNS = 14

function pad(text: string, columns: number): string {
  return text + ' '.repeat(Math.max(0, columns - stringWidth(text)))
}

function padStart(text: string, columns: number): string {
  return ' '.repeat(Math.max(0, columns - stringWidth(text))) + text
}

/** One breakdown row as text: label, tokens and share right-aligned, then its files. */
export function breakdownLine(row: BreakdownRow, width: number): string {
  const narrow = width < 60
  const tokens = row.tokens === null ? '' : formatTokenCount(row.tokens)
  const share = `${row.percent.toFixed(1)}%`
  let line = `  ${pad(row.label, LABEL_COLUMNS)}${padStart(tokens, 7)}`
  if (!narrow) line += padStart(share, 8)
  else if (row.tokens === null) line += padStart(`~${Math.round(row.percent)}%`, 6)
  if (row.detail && !narrow) line += `   ${row.detail}`
  return line
}

/** The model by name, without the gateway's route to it (utils/model-name.ts). */
function modelName(status: StatusEvent, models: ReturnType<typeof useEngineStore.getState>['models']): string {
  const model = catalogModel(models, status.model)
  return distinctModelName(
    models?.models,
    model?.id,
    cleanModelName(
      status.model,
      status.model_display_name ?? model?.display_name,
      status.provider ?? model?.provider,
    ),
  )
}

export const ContextPopover = ({
  onClose,
  width,
  now = Date.now(),
  compact = false,
  maxRows,
}: {
  onClose: () => void
  width: number
  now?: number
  /** A short terminal: only the summary lines. */
  compact?: boolean
  /** The most body lines (between the border and `esc close`). */
  maxRows?: number
}) => {
  const theme = useTheme()
  const status = useEngineStore((state) => state.status)
  const models = useEngineStore((state) => state.models)
  const account = useEngineStore((state) => state.account)
  const usage = useEngineStore((state) => state.usage)
  const unsettledInput = useLiveTurnStore((state) => state.unsettledInput)
  const unsettledOutput = useLiveTurnStore((state) => state.unsettledOutput)
  const streamingOutput = useLiveTurnStore((state) => state.streamingOutput)
  const signedIn = status ? hasUsableZeroXLogin(status) : false

  useEffect(() => {
    if (!signedIn) return
    refreshAccount()
    const timer = setInterval(refreshAccount, ACCOUNT_REFRESH_MS)
    return () => clearInterval(timer)
  }, [signedIn])

  useKeyboard(
    useCallback(
      (key: KeyEvent) => {
        if (key.name === 'escape' || (key.ctrl && key.name === 'c') || (key.meta && key.name === 'c')) {
          key.preventDefault?.()
          onClose()
        }
      },
      [onClose],
    ),
  )

  const panelWidth = Math.max(30, width)
  const inner = panelWidth - 4
  const ctx = contextUsage(status)
  const live = liveFooterTokens({ unsettledInput, unsettledOutput, streamingOutput })
  const tokens = sessionTokens(usage, live)

  const lines: React.ReactNode[] = []
  if (status && ctx) {
    const context = ctx.breakdown
    lines.push(
      <text key="title" style={{ wrapMode: 'none', fg: theme.foreground }}>
        {`Context · ${modelName(status, models)}${context ? ` · ${formatTokenCount(context.context_window)} window` : ''}`}
      </text>,
    )
    lines.push(
      <box key="meter" style={{ flexDirection: 'row', overflow: 'hidden' }}>
        <ContextMeter percent={ctx.percent} thresholdPercent={ctx.thresholdPercent} estimated={ctx.estimated} />
        <text style={{ wrapMode: 'none', fg: theme.muted }}>
          {context && ctx.usable !== null
            ? `  of ${formatTokenCount(ctx.usable)} usable · ${formatTokenCount(context.output_reserve)} kept for the reply`
            : '  estimate from an older engine'}
        </text>
      </box>,
    )
    const rows = breakdownRows(ctx)
    const shown = compact && rows.some((row) => row.free) ? rows.filter((row) => row.free) : rows
    for (const row of shown) {
      lines.push(
        <text key={`row-${row.label}`} style={{ wrapMode: 'none', fg: row.free ? theme.success : theme.muted }}>
          {breakdownLine(row, inner)}
        </text>,
      )
    }
    const threshold =
      context && ctx.usable !== null
        ? `  Auto-compacts at ${Math.round(ctx.thresholdPercent)}% (${formatTokenCount(context.threshold_tokens)}) · /compact now`
        : `  Auto-compacts at ${Math.round(ctx.thresholdPercent)}% · /compact now`
    lines.push(
      <text key="threshold" style={{ wrapMode: 'none', fg: theme.muted }}>
        {threshold}
      </text>,
    )
  } else {
    lines.push(
      <text key="none" style={{ wrapMode: 'word', fg: theme.muted }}>
        No session yet: the context fills once the engine opens one.
      </text>,
    )
  }
  if (tokens && !compact) {
    lines.push(
      <text key="session" style={{ wrapMode: 'none', fg: theme.muted }}>
        {`This session  ${tokens}`}
      </text>,
    )
  }
  if (signedIn && account) {
    const ago = updatedAgo(account, now)
    lines.push(
      <text key="account" style={{ wrapMode: 'none', fg: theme.foreground }}>
        {`Account · ${accountTitle(account)}`}
        {ago ? <span fg={theme.muted}>{` · ${ago}`}</span> : null}
      </text>,
    )
    for (const row of compact ? [] : accountRows(account, now)) {
      lines.push(
        <box key={`acct-${row.label}`} style={{ flexDirection: 'row' }}>
          <text style={{ wrapMode: 'none', fg: theme.muted }}>{`  ${pad(row.label, 16)}${pad(row.value, 22)}`}</text>
          {row.percent !== null && inner >= 60 ? (
            <ContextMeter percent={row.percent} />
          ) : inner >= 60 ? (
            <text style={{ wrapMode: 'none' }}>{' '.repeat(ACCOUNT_METER_COLUMNS)}</text>
          ) : null}
          {row.resets ? (
            <text style={{ wrapMode: 'none', fg: theme.muted }}>{`  resets ${row.resets}`}</text>
          ) : null}
        </box>,
      )
    }
    if (!compact) {
      lines.push(
        <text key="links" style={{ wrapMode: 'none', fg: theme.muted }}>
          {'  /usage details · /upgrade'}
        </text>,
      )
    }
  } else if (signedIn) {
    lines.push(
      <text key="account-loading" style={{ wrapMode: 'none', fg: theme.muted }}>
        Account · reading the plan and usage…
      </text>,
    )
  }

  let body = lines
  if (maxRows !== undefined && lines.length > Math.max(1, maxRows)) {
    const keep = Math.max(1, maxRows) - 1
    body = [
      ...lines.slice(0, keep),
      <text key="more" style={{ wrapMode: 'none', fg: theme.muted }}>
        {`  … ${lines.length - keep} more lines: /context, /usage`}
      </text>,
    ]
  }
  const help = compact ? 'esc close · /context for the breakdown' : 'esc close'

  return (
    <box
      title=" Context "
      titleAlignment="center"
      style={{
        width: panelWidth,
        borderStyle: 'single',
        borderColor: theme.border,
        customBorderChars: BORDER_CHARS,
        paddingLeft: 1,
        paddingRight: 1,
        flexDirection: 'column',
      }}
    >
      {body}
      <text style={{ wrapMode: 'none', fg: theme.muted }}>{help}</text>
    </box>
  )
}
