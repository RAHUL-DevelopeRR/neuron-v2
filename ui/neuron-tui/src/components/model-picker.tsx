// Neuron-original, except the thinking-level submenu (Tab on a model), which is
// derived from Codebuff/Freebuff (Apache-2.0): the reasoning submenu of
// cli/src/components/freebuff-model-selector.tsx (its Tab handling and list),
// modified to take the levels from the engine's catalog and to set them through
// the engine. See ui/neuron-tui/PORTING.md.
/**
 * `/model` with no argument: the session's model, picked from the engine's
 * catalog (`list_models`, which only the Zero-X gateway publishes). Models the
 * plan can't use now are greyed with the reason and can't be picked, and so
 * are models the gateway has taken out of rotation ("blurred") until they are
 * back; "degraded" and "busy" ones are marked but can be picked. The list
 * refreshes every 30 s while the picker is open. Enter on a model that can't
 * be picked says why. Rows too long for the width end in "…". `/model <name>`
 * switches without it, for providers without a catalog.
 *
 *   ↑/↓ move   enter switch   tab thinking level   esc close
 *
 * Tab on a model that has thinking levels lists them (↑↓ Enter Esc); Enter
 * switches to that model, if it is not the current one, at that level.
 */

import { TextAttributes, type KeyEvent } from '@opentui/core'
import { useKeyboard } from '@opentui/react'
import React, { useCallback, useEffect, useMemo, useState } from 'react'
import stringWidth from 'string-width'

import { getNeuronEngineClient } from '../engine/neuron-engine-client'
import { setReasoningEffort } from '../engine/status-bar-actions'
import { useTheme } from '../hooks/use-theme'
import { useEngineStore } from '../state/engine-store'
import { cutToDisplayWidth } from '../utils/display-width'
import { formatElapsedTime } from '../utils/format-elapsed-time'
import { formatTokenCount } from '../utils/format-token-count'
import { logger } from '../utils/logger'
import { cleanModelName, distinctModelName, findCatalogModel } from '../utils/model-name'
import { DEFAULT_REASONING_EFFORTS, effortLabel } from '../utils/status-bar-parts'
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
  /** Why it can't be picked now, in words; `null` when it can. */
  unavailableReason: string | null
  /** Its thinking levels, lowest first; `null` when it has none to pick. */
  efforts: string[] | null
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
const isCurrentModel = (models: ModelsEvent, id: string): boolean =>
  findCatalogModel(models.models, models.current)?.id === id

/**
 * The picker's rows: name, then context window, plans, the gateway's health,
 * or why it's unavailable. `now` dates a blurred model's return. Names leave
 * out the route the gateway takes to a model (utils/model-name.ts), two that
 * are then the same get a number, and the raw id is not shown: `/status` has it.
 */
export function modelRows(models: ModelsEvent | null, now: number = Date.now()): ModelRow[] {
  if (!models) return []
  return models.models.map((model: ModelInfo) => {
    const details: string[] = []
    if (model.context_window) details.push(`${formatTokenCount(model.context_window)} context`)
    // Blurred until its time has come; without a time, until the next list says otherwise.
    const againAt = model.available_again_at ? Date.parse(model.available_again_at) : NaN
    const blurred =
      model.status === 'blurred' && (Number.isNaN(againAt) || againAt > now)
    let marker: string | null = null
    let unavailableReason: string | null = null
    if (!model.available) {
      const reason = model.unavailable_reason
      unavailableReason = reason ? (UNAVAILABLE_REASONS[reason] ?? reason) : 'not on your plan'
      details.push(unavailableReason)
    } else if (blurred) {
      const backIn = backInLabel(model.available_again_at, now)
      details.push(backIn ?? 'temporarily unavailable')
      unavailableReason = `the gateway took it out of rotation${backIn ? `; ${backIn}` : ' for now'}`
    } else {
      if (model.tiers.length > 0) details.push(model.tiers.join(', '))
      if (model.status === 'degraded' || model.status === 'busy') marker = model.status
    }
    const efforts =
      model.supports_reasoning === false
        ? null
        : model.reasoning_efforts && model.reasoning_efforts.length > 0
          ? model.reasoning_efforts
          : model.supports_reasoning
            ? [...DEFAULT_REASONING_EFFORTS]
            : null
    return {
      id: model.id,
      label: distinctModelName(
        models.models,
        model.id,
        cleanModelName(model.id, model.display_name, model.provider),
      ),
      detail: details.join(' · '),
      available: model.available && !blurred,
      current: isCurrentModel(models, model.id),
      marker,
      unavailableReason,
      efforts,
    }
  })
}

/** `text` cut to `width` columns, ending in "…" when it was cut. */
export function truncateToWidth(text: string, width: number): string {
  return cutToDisplayWidth(text, width)
}

/** A row's parts as drawn: `current`, `marker` and `detail` keep their leading spaces. */
export interface FittedRow {
  label: string
  current: string
  marker: string
  detail: string
}

/**
 * A row in `width` columns (after its 2-column pointer): the detail gives way
 * first, then the label; "(current)" and the health marker stay.
 */
export function fitModelRow(row: ModelRow, width: number): FittedRow {
  const current = row.current ? '  (current)' : ''
  const marker = row.marker ? `  ${row.marker}` : ''
  const detail = row.detail ? `  ${row.detail}` : ''
  const fixed = stringWidth(current) + stringWidth(marker)
  const labelWidth = stringWidth(row.label)
  if (labelWidth + fixed + stringWidth(detail) <= width) {
    return { label: row.label, current, marker, detail }
  }
  const detailRoom = width - labelWidth - fixed
  // A detail cut to almost nothing says nothing.
  if (detailRoom >= 8) {
    return { label: row.label, current, marker, detail: truncateToWidth(detail, detailRoom) }
  }
  return {
    label: truncateToWidth(row.label, Math.max(1, width - fixed)),
    current,
    marker,
    detail: '',
  }
}

/** A model's thinking levels in the submenu: its own, then its default (`''`). */
export function reasoningLevels(row: Pick<ModelRow, 'efforts'>): string[] {
  const levels = row.efforts ?? []
  return levels.includes('default') ? levels : [...levels, '']
}

/** What Enter on a model that can't be picked says. */
export function unavailableMessage(row: ModelRow): string {
  return `${row.label} can't be picked: ${row.unavailableReason ?? 'not available now'}.`
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
  const currentEffort = useEngineStore((state) => state.status?.reasoning_effort ?? null)
  const loaded = models !== null
  // The model whose thinking levels Tab opened, and the focused level.
  const [reasoningFor, setReasoningFor] = useState<ModelRow | null>(null)
  const [reasoningIndex, setReasoningIndex] = useState(0)
  // Each 30 s refresh is a new list, so "back in …" is dated afresh with it.
  const rows = useMemo(() => modelRows(models, Date.now()), [models])
  const currentIndex = rows.findIndex((row) => row.current)
  const currentId = currentIndex >= 0 ? rows[currentIndex]!.id : null
  // Each open starts on the current model (the picker mounts per open).
  const [focus, setFocus] = useState(() => Math.max(0, currentIndex))
  // Why the model Enter was pressed on can't be picked, until the focus moves.
  // Kept with its row, not cleared by an effect on `focus`: that effect could
  // run after a quick Enter on the new row and clear what Enter just said.
  const [noticeFor, setNotice] = useState<{ focus: number; text: string } | null>(null)
  const notice = noticeFor?.focus === focus ? noticeFor.text : null

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

  // Move to the current model again whenever it changes: the list kept from
  // the last open can name the model from before a switch, and the fresh one
  // that replaces it moments later has as many rows. A refresh that keeps the
  // same model leaves the focus where the user moved it. The window below
  // follows the focus, so the row is in view.
  useEffect(() => {
    if (currentIndex >= 0) setFocus(currentIndex)
  }, [currentId])

  useKeyboard(
    useCallback(
      (key: KeyEvent) => {
        if (reasoningFor) {
          const levels = reasoningLevels(reasoningFor)
          key.preventDefault?.()
          if (key.name === 'escape') setReasoningFor(null)
          else if (key.name === 'up' || key.name === 'down') {
            const delta = key.name === 'up' ? -1 : 1
            setReasoningIndex((index) => (index + delta + levels.length) % levels.length)
          } else if (key.name === 'return' || key.name === 'enter') {
            const level = levels[reasoningIndex]
            if (level === undefined) return
            try {
              if (!reasoningFor.current) switchModel(reasoningFor.id)
            } catch {
              return
            }
            setReasoningEffort(level === '' ? null : level)
            setReasoningFor(null)
            onClose()
          }
          return
        }
        if (key.name === 'escape' || (key.ctrl && key.name === 'c')) {
          key.preventDefault?.()
          onClose()
          return
        }
        if (key.ctrl || key.meta || key.option) return
        if (key.name === 'tab' && !key.shift) {
          key.preventDefault?.()
          const row = rows[focus]
          if (!row) return
          if (!row.available) {
            setNotice({ focus, text: unavailableMessage(row) })
            return
          }
          if (!row.efforts) {
            setNotice({ focus, text: `${row.label} has no thinking levels to pick.` })
            return
          }
          const levels = reasoningLevels(row)
          const wanted = row.current ? (currentEffort ?? '') : ''
          setReasoningIndex(Math.max(0, levels.indexOf(wanted)))
          setReasoningFor(row)
          return
        }
        if (key.name === 'up') {
          key.preventDefault?.()
          setNotice(null)
          setFocus((index) => Math.max(0, index - 1))
        } else if (key.name === 'down') {
          key.preventDefault?.()
          setNotice(null)
          setFocus((index) => Math.min(rows.length - 1, index + 1))
        } else if (key.name === 'return' || key.name === 'enter') {
          key.preventDefault?.()
          const row = rows[focus]
          if (!row) return
          if (!row.available) {
            setNotice({ focus, text: unavailableMessage(row) })
            return
          }
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
      [currentEffort, focus, onClose, reasoningFor, reasoningIndex, rows],
    ),
  )

  if (reasoningFor) {
    const levels = reasoningLevels(reasoningFor)
    return (
      <box
        title=" Thinking level "
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
        <text style={{ fg: theme.foreground }}>{`${reasoningFor.label} · thinking level`}</text>
        {levels.map((level, index) => {
          const focused = index === reasoningIndex
          const current = reasoningFor.current && (currentEffort ?? '') === level
          return (
            <text key={level || 'default'} style={{ wrapMode: 'none' }}>
              <span fg={focused ? theme.primary : theme.muted}>{focused ? '› ' : '  '}</span>
              <span fg={theme.foreground} attributes={focused ? TextAttributes.BOLD : undefined}>
                {level === '' ? 'default' : effortLabel(level)}
              </span>
              {current ? <span fg={theme.success}>  (current)</span> : null}
            </text>
          )
        })}
        <text style={{ fg: theme.muted }}>↑↓ choose · enter save · esc back</text>
      </box>
    )
  }

  const [start, end] = visibleWindow(rows.length, focus, maxVisibleRows)
  // Inside the border and padding, after the 2-column pointer.
  const rowWidth = Math.max(4, Math.max(20, width) - 4 - 2)

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
          const fitted = fitModelRow(row, rowWidth)
          return (
            <text key={row.id} style={{ wrapMode: 'none' }}>
              <span fg={focused ? theme.primary : theme.muted}>{focused ? '› ' : '  '}</span>
              <span
                fg={row.available ? theme.foreground : theme.muted}
                attributes={focused && row.available ? TextAttributes.BOLD : undefined}
              >
                {fitted.label}
              </span>
              {fitted.current ? <span fg={theme.success}>{fitted.current}</span> : null}
              {fitted.marker ? (
                <span fg={theme.warning} attributes={TextAttributes.DIM}>
                  {fitted.marker}
                </span>
              ) : null}
              {fitted.detail ? <span fg={theme.muted}>{fitted.detail}</span> : null}
            </text>
          )
        })
      )}
      {notice ? (
        <text style={{ wrapMode: 'word', fg: theme.warning }}>{notice}</text>
      ) : null}
      <text style={{ fg: theme.muted }}>
        {rows.length > maxVisibleRows ? `${focus + 1}/${rows.length} · ` : ''}
        ↑↓ move · enter switch · tab thinking · esc close
      </text>
    </box>
  )
}
