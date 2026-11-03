/**
 * `/changes`: every file the agent created, changed or deleted this session,
 * with lines added and removed since it first touched them (the engine's
 * `changes`), in the composer's place. It follows new `changes` while open.
 *
 *   ↑/↓ move   enter diff of the file   a diff of everything   esc close
 *
 * A diff closes the panel and runs `/diff [path]`; its cell lands in the chat.
 */

import { TextAttributes, type KeyEvent } from '@opentui/core'
import { useKeyboard } from '@opentui/react'
import React, { useCallback, useEffect, useState } from 'react'

import { truncateToWidth, visibleWindow } from './model-picker'
import { requestDiff } from '../engine/git-views'
import { useTheme } from '../hooks/use-theme'
import {
  formatChangesSummary,
  lineCounts,
  statusMark,
  useGitViewsStore,
} from '../state/git-views-store'
import { BORDER_CHARS } from '../utils/ui-constants'

import type { ChangedFile } from '../engine/protocol'

export type ChangesKeyAction =
  | { type: 'move'; delta: number }
  | { type: 'diff-file' }
  | { type: 'diff-all' }
  | { type: 'close' }
  | { type: 'none' }

/** What a key does in the panel. */
export function resolveChangesKey(key: Pick<KeyEvent, 'name' | 'ctrl' | 'meta'>): ChangesKeyAction {
  if (key.name === 'escape' || (key.ctrl && !key.meta && key.name === 'c')) {
    return { type: 'close' }
  }
  if (key.ctrl || key.meta) return { type: 'none' }
  switch (key.name) {
    case 'up':
      return { type: 'move', delta: -1 }
    case 'down':
      return { type: 'move', delta: 1 }
    case 'return':
    case 'enter':
      return { type: 'diff-file' }
    case 'a':
      return { type: 'diff-all' }
    default:
      return { type: 'none' }
  }
}

/** Closes the panel and shows the diff of `path` (or of everything). */
export function showDiffFromPanel(path: string | null): void {
  useGitViewsStore.getState().closeChangesPanel()
  requestDiff(path ?? '', { echo: true })
}

export const ChangesPanel = ({
  files,
  width,
  maxVisibleRows = 8,
}: {
  files: ChangedFile[]
  width: number
  maxVisibleRows?: number
}) => {
  const theme = useTheme()
  const [focus, setFocus] = useState(0)
  // A shorter list (a rewind put files back) keeps the focus on a row.
  useEffect(() => {
    setFocus((index) => Math.max(0, Math.min(index, files.length - 1)))
  }, [files.length])

  useKeyboard(
    useCallback(
      (key: KeyEvent) => {
        const action = resolveChangesKey(key)
        if (action.type === 'none') return
        key.preventDefault?.()
        switch (action.type) {
          case 'close':
            useGitViewsStore.getState().closeChangesPanel()
            return
          case 'move':
            setFocus((index) =>
              Math.max(0, Math.min(files.length - 1, index + action.delta)),
            )
            return
          case 'diff-file': {
            const file = files[focus]
            if (file) showDiffFromPanel(file.path)
            return
          }
          case 'diff-all':
            if (files.length > 0) showDiffFromPanel(null)
            return
        }
      },
      [files, focus],
    ),
  )

  const [start, end] = visibleWindow(files.length, focus, maxVisibleRows)
  // Inside the border and padding, after the pointer and the status mark.
  const rowWidth = Math.max(10, Math.max(20, width) - 4 - 4)
  const summary = formatChangesSummary(files)

  return (
    <box
      title={summary ? ` Changes · ${summary} ` : ' Changes '}
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
      {files.length === 0 ? (
        <text style={{ wrapMode: 'word', fg: theme.muted }}>
          No files changed in this session yet. Files the agent writes, edits or
          deletes show here.
        </text>
      ) : (
        files.slice(start, end).map((file, offset) => {
          const focused = start + offset === focus
          const counts = `  +${file.added} -${file.removed}`
          const mark = statusMark(file.status)
          const markColor =
            file.status === 'deleted'
              ? theme.error
              : file.status === 'created'
                ? theme.success
                : theme.warning
          return (
            <text key={file.path} style={{ wrapMode: 'none' }}>
              <span fg={focused ? theme.primary : theme.muted}>{focused ? '› ' : '  '}</span>
              <span fg={markColor}>{`${mark} `}</span>
              <span
                fg={theme.foreground}
                attributes={focused ? TextAttributes.BOLD : undefined}
              >
                {truncateToWidth(file.path, Math.max(4, rowWidth - counts.length))}
              </span>
              <span fg={theme.success}>{`  +${file.added}`}</span>
              <span fg={theme.error}>{` -${file.removed}`}</span>
            </text>
          )
        })
      )}
      <text style={{ fg: theme.muted }}>
        {files.length > maxVisibleRows ? `${focus + 1}/${files.length} · ` : ''}
        {files.length > 0 ? '↑↓ move · enter diff · a diff all · esc close' : 'esc close'}
      </text>
    </box>
  )
}

/**
 * The always-visible line above the input while files changed:
 * `3 files changed +120 -4 · /changes`.
 */
export const ChangesSummary = ({ files }: { files: ChangedFile[] }) => {
  const theme = useTheme()
  if (files.length === 0) return null
  const { added, removed } = lineCounts(files)
  return (
    <box style={{ paddingLeft: 1, paddingRight: 1, flexShrink: 0 }}>
      <text style={{ wrapMode: 'none' }}>
        <span fg={theme.muted}>
          {`${files.length} ${files.length === 1 ? 'file' : 'files'} changed `}
        </span>
        <span fg={theme.success}>{`+${added}`}</span>
        <span fg={theme.error}>{` -${removed}`}</span>
        <span fg={theme.muted}>{' · /changes'}</span>
      </text>
    </box>
  )
}
