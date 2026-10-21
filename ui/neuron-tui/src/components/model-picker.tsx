/**
 * `/model` with no argument: the session's model, picked from the engine's
 * catalog (`list_models`, which only the Zero-X gateway publishes). Models the
 * plan can't use now are greyed with the reason and can't be picked, and so
 * are models the gateway has taken out of rotation ("blurred") until they are
 * back; "degraded" and "busy" ones are marked but can be picked. The list
 * refreshes every 30 s while the picker is open. `/model <name>` switches
 * without it, for providers without a catalog.
 *
 *   ↑/↓ move   enter switch   esc close
 */

import { TextAttributes, type KeyEvent } from '@opentui/core'
import { useKeyboard } from '@opentui/react'
import React, { useCallback, useEffect, useMemo, useState } from 'react'

import { getNeuronEngineClient } from '../engine/neuron-engine-client'
import { useTheme } from '../hooks/use-theme'
import { useEngineStore } from '../state/engine-store'
import { formatElapsedTime } from '../utils/format-elapsed-time'
import { formatTokenCount } from '../utils/format-token-count'
import { logger } from '../utils/logger'
import { BORDER_CHARS } from '../utils/ui-constants'

import type { ModelInfo, ModelsEvent } from '../engine/protocol'

export const MODEL_REFRESH_MS = 30_000

export interface ModelRow {
  id: string
  label: string
  detail: string
  /** Can be picked now: on the plan, configured, and not out of rotation. */
  available: boolean
  current: boolean
  /** A usable model's health worth a glance ("degraded", "busy"), or `null`. */
  marker: string | null
}

/** The gateway's `unavailable_reason` codes, in words. Other text is shown as sent. */
const UNAVAILABLE_REASONS: Record<string, string> = {
  upgrade_required: 'needs a plan upgrade',
  region_unavailable: 'not available in your region',
  not_configured: 'not configured on this gateway',
}

/** "back in 3h 20m" for a blurred model, from its `available_again_at`. */
export function backInLabel(availableAgainAt: string | null | undefined, now: number): string | null {
  if (!availableAgainAt) return null
  const at = Date.parse(availableAgainAt)
  if (Number.isNaN(at)) return null
  const minutes = Math.max(1, Math.ceil((at - now) / 60_000))
  return `back in ${formatElapsedTime(minutes * 60)}`
}

/**
 * The engine sends `current` as the wire model, which can carry the
 * provider's `openai/` prefix the catalog ids do not have.
 */
const isCurrentModel = (id: string, current: string): boolean =>
  id === current || current === `openai/${id}`

/**
 * The picker's rows: name, then context window, plans, the gateway's health,
 * or why it's unavailable. `now` dates a blurred model's return.
 */
export function modelRows(models: ModelsEvent | null, now: number = Date.now()): ModelRow[] {
  if (!models) return []
  return models.models.map((model: ModelInfo) => {
    const details: string[] = []
    if (model.display_name && model.display_name !== model.id) details.push(model.id)
    if (model.context_window) details.push(`${formatTokenCount(model.context_window)} context`)
    // Blurred until its time has come; without a time, until the next list says otherwise.
    const againAt = model.available_again_at ? Date.parse(model.available_again_at) : NaN
    const blurred =
      model.status === 'blurred' && (Number.isNaN(againAt) || againAt > now)
    let marker: string | null = null
    if (!model.available) {
      const reason = model.unavailable_reason
      details.push(reason ? (UNAVAILABLE_REASONS[reason] ?? reason) : 'not on your plan')
    } else if (blurred) {
      details.push(backInLabel(model.available_again_at, now) ?? 'temporarily unavailable')
    } else {
      if (model.tiers.length > 0) details.push(model.tiers.join(', '))
      if (model.status === 'degraded' || model.status === 'busy') marker = model.status
    }
    return {
      id: model.id,
      label: model.display_name ?? model.id,
      detail: details.join(' · '),
      available: model.available && !blurred,
      current: isCurrentModel(model.id, models.current),
      marker,
    }
  })
}

/** The rows to draw: a window of `size` rows that keeps `focus` in view. */
export function visibleWindow(count: number, focus: number, size: number): [number, number] {
  if (count <= size) return [0, count]
  const start = Math.min(Math.max(0, focus - Math.floor(size / 2)), count - size)
  return [start, start + size]
}

/** Asks the engine to switch; its notice and new status say how it went. */
export function switchModel(id: string): void {
  try {
    getNeuronEngineClient().setModel(id)
  } catch (error) {
    logger.warn({ error }, '[model] switch failed')
    throw error
  }
}

export const ModelPicker = ({
  onClose,
  width,
  maxVisibleRows = 8,
}: {
  onClose: () => void
  width: number
  maxVisibleRows?: number
}) => {
  const theme = useTheme()
  const models = useEngineStore((state) => state.models)
  const loaded = models !== null
  // Each 30 s refresh is a new list, so "back in …" is dated afresh with it.
  const rows = useMemo(() => modelRows(models, Date.now()), [models])
  const [focus, setFocus] = useState(0)

  // Ask now and every 30 s; until the first reply the last list (if any) shows.
  useEffect(() => {
    const ask = () => {
      try {
        getNeuronEngineClient().listModels()
      } catch (error) {
        logger.debug({ error }, '[model] list_models failed')
      }
    }
    ask()
    const timer = setInterval(ask, MODEL_REFRESH_MS)
    return () => clearInterval(timer)
  }, [])

  // Start on the current model once the list arrives.
  useEffect(() => {
    const current = rows.findIndex((row) => row.current)
    if (current >= 0) setFocus(current)
    // Only when the set of models changes, not on every refresh.
  }, [rows.length])

  useKeyboard(
    useCallback(
      (key: KeyEvent) => {
        if (key.name === 'escape' || (key.ctrl && key.name === 'c')) {
          key.preventDefault?.()
          onClose()
          return
        }
        if (key.ctrl || key.meta || key.option) return
        if (key.name === 'up') {
          key.preventDefault?.()
          setFocus((index) => Math.max(0, index - 1))
        } else if (key.name === 'down') {
          key.preventDefault?.()
          setFocus((index) => Math.min(rows.length - 1, index + 1))
        } else if (key.name === 'return' || key.name === 'enter') {
          key.preventDefault?.()
          const row = rows[focus]
          if (!row || !row.available) return
          if (!row.current) {
            try {
              switchModel(row.id)
            } catch {
              return
            }
          }
          onClose()
        }
      },
      [focus, onClose, rows],
    ),
  )

  const [start, end] = visibleWindow(rows.length, focus, maxVisibleRows)

  return (
    <box
      title=" Model "
      titleAlignment="center"
      style={{
        width: Math.max(20, width),
        borderStyle: 'single',
        borderColor: theme.border,
        customBorderChars: BORDER_CHARS,
        paddingLeft: 1,
        paddingRight: 1,
        flexDirection: 'column',
      }}
    >
      {!loaded ? (
        <text style={{ fg: theme.muted }}>Loading models…</text>
      ) : rows.length === 0 ? (
        <text style={{ wrapMode: 'word', fg: theme.muted }}>
          {`This provider has no model catalog. The session uses ${models?.current ?? 'its default model'}; type /model <name> to switch (for example /model opus).`}
        </text>
      ) : (
        rows.slice(start, end).map((row, offset) => {
          const index = start + offset
          const focused = index === focus
          return (
            <text key={row.id} style={{ wrapMode: 'none' }}>
              <span fg={focused ? theme.primary : theme.muted}>{focused ? '› ' : '  '}</span>
              <span
                fg={row.available ? theme.foreground : theme.muted}
                attributes={focused && row.available ? TextAttributes.BOLD : undefined}
              >
                {row.label}
              </span>
              {row.current ? <span fg={theme.success}>{'  (current)'}</span> : null}
              {row.marker ? (
                <span fg={theme.warning} attributes={TextAttributes.DIM}>
                  {`  ${row.marker}`}
                </span>
              ) : null}
              {row.detail ? <span fg={theme.muted}>{`  ${row.detail}`}</span> : null}
            </text>
          )
        })
      )}
      <text style={{ fg: theme.muted }}>
        {rows.length > maxVisibleRows ? `${focus + 1}/${rows.length} · ` : ''}
        ↑↓ move · enter switch · esc close
      </text>
    </box>
  )
}
