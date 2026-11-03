/**
 * A small list in the panel slot under the chat (where the model picker
 * opens): the thinking level, the permission mode, the workspace and the
 * branch pickers of the status bar.
 *
 *   ↑/↓ move   enter pick   esc close   (and the panel's own keys)
 *
 * Rows that cannot be picked are greyed and say why when Enter is pressed on
 * them. Rows are clickable.
 */

import { TextAttributes, type KeyEvent } from '@opentui/core'
import { useKeyboard } from '@opentui/react'
import React, { useCallback, useState } from 'react'

import { Button } from './button'
import { truncateToWidth, visibleWindow } from './model-picker'
import { useTheme } from '../hooks/use-theme'
import { isPlainEnterKey } from '../utils/terminal-enter-detection'
import { BORDER_CHARS } from '../utils/ui-constants'

export interface ChoiceRow {
  id: string
  label: string
  detail?: string | null
  current?: boolean
  /** Why it cannot be picked; `null` or absent when it can. */
  disabledReason?: string | null
}

export type ChoiceKeyAction =
  | { type: 'move'; delta: number }
  | { type: 'pick' }
  | { type: 'close' }
  | { type: 'key'; name: string }
  | { type: 'none' }

/** What a key does in a choice panel; `extraKeys` are letters the panel handles itself. */
export function resolveChoiceKey(
  key: Pick<KeyEvent, 'name' | 'ctrl' | 'meta' | 'option' | 'shift'> & { sequence?: string },
  extraKeys: readonly string[] = [],
): ChoiceKeyAction {
  if (key.name === 'escape' || (key.ctrl && key.name === 'c')) return { type: 'close' }
  if (key.ctrl || key.meta || key.option) return { type: 'none' }
  if (key.name === 'up') return { type: 'move', delta: -1 }
  if (key.name === 'down') return { type: 'move', delta: 1 }
  if (key.name === 'return' || key.name === 'enter' || key.name === 'space') return { type: 'pick' }
  if (extraKeys.includes(key.name)) return { type: 'key', name: key.name }
  return { type: 'none' }
}

export const ChoicePanel = ({
  title,
  rows,
  onPick,
  onClose,
  onKey,
  onMove,
  extraKeys = [],
  hint,
  notice,
  header,
  width,
  maxVisibleRows = 8,
  emptyText = 'Nothing to pick.',
}: {
  title: string
  rows: ChoiceRow[]
  onPick: (row: ChoiceRow) => void
  onClose: () => void
  /** A key from `extraKeys` on the focused row (`null` when there are no rows). */
  onKey?: (name: string, row: ChoiceRow | null) => void
  /** The focus moved (a pending confirmation is cancelled). */
  onMove?: () => void
  extraKeys?: readonly string[]
  /** The help line's extra part ("n new branch"). */
  hint?: string
  /** A line above the help (a refusal, a confirmation question). */
  notice?: { text: string; tone: 'warning' | 'muted' | 'error' } | null
  /** A line under the title (what the list is about). */
  header?: string | null
  width: number
  maxVisibleRows?: number
  emptyText?: string
}) => {
  const theme = useTheme()
  const currentIndex = rows.findIndex((row) => row.current)
  const [focus, setFocus] = useState(() => Math.max(0, currentIndex))
  const [refusal, setRefusal] = useState<string | null>(null)
  const clamped = Math.min(focus, Math.max(0, rows.length - 1))

  const pick = useCallback(
    (row: ChoiceRow | undefined) => {
      if (!row) return
      if (row.disabledReason) {
        setRefusal(`${row.label} can't be picked: ${row.disabledReason}.`)
        return
      }
      onPick(row)
    },
    [onPick],
  )

  useKeyboard(
    useCallback(
      (key: KeyEvent) => {
        // Enter arrives as a plain Enter on terminals that send \n too.
        const action = isPlainEnterKey(key) ? ({ type: 'pick' } as const) : resolveChoiceKey(key, extraKeys)
        if (action.type === 'none') return
        key.preventDefault?.()
        switch (action.type) {
          case 'close':
            onClose()
            return
          case 'move':
            setRefusal(null)
            onMove?.()
            setFocus((index) => Math.max(0, Math.min(rows.length - 1, index + action.delta)))
            return
          case 'pick':
            pick(rows[clamped])
            return
          case 'key':
            onKey?.(action.name, rows[clamped] ?? null)
            return
        }
      },
      [clamped, extraKeys, onClose, onKey, onMove, pick, rows],
    ),
  )

  const [start, end] = visibleWindow(rows.length, clamped, maxVisibleRows)
  const panelWidth = Math.max(20, width)
  const rowWidth = Math.max(4, panelWidth - 4 - 2)
  const shownNotice = refusal ? { text: refusal, tone: 'warning' as const } : notice

  return (
    <box
      title={` ${title} `}
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
      {header ? <text style={{ wrapMode: 'word', fg: theme.muted }}>{header}</text> : null}
      {rows.length === 0 ? (
        <text style={{ wrapMode: 'word', fg: theme.muted }}>{emptyText}</text>
      ) : (
        rows.slice(start, end).map((row, offset) => {
          const index = start + offset
          const focused = index === clamped
          const enabled = !row.disabledReason
          const current = row.current ? '  (current)' : ''
          const label = truncateToWidth(row.label, Math.max(4, rowWidth - current.length))
          const detailRoom = rowWidth - label.length - current.length - 2
          const detail = row.detail && detailRoom >= 8 ? `  ${truncateToWidth(row.detail, detailRoom)}` : ''
          return (
            <Button
              key={row.id}
              onClick={() => {
                setFocus(index)
                pick(row)
              }}
            >
              <text style={{ wrapMode: 'none' }}>
                <span fg={focused ? theme.primary : theme.muted}>{focused ? '› ' : '  '}</span>
                <span
                  fg={enabled ? theme.foreground : theme.muted}
                  attributes={focused && enabled ? TextAttributes.BOLD : undefined}
                >
                  {label}
                </span>
                {current ? <span fg={theme.success}>{current}</span> : null}
                {detail ? <span fg={theme.muted}>{detail}</span> : null}
              </text>
            </Button>
          )
        })
      )}
      {shownNotice ? (
        <text
          style={{
            wrapMode: 'word',
            fg: shownNotice.tone === 'error' ? theme.error : shownNotice.tone === 'warning' ? theme.warning : theme.muted,
          }}
        >
          {shownNotice.text}
        </text>
      ) : null}
      <text style={{ fg: theme.muted }}>
        {rows.length > maxVisibleRows ? `${clamped + 1}/${rows.length} · ` : ''}
        {`↑↓ move · enter pick${hint ? ` · ${hint}` : ''} · esc close`}
      </text>
    </box>
  )
}
