/**
 * `/rewind`: go back to before an earlier prompt, in the composer's place.
 *
 *   1. pick the checkpoint (newest first: time, prompt, files; a warning
 *      when shell commands ran, which a rewind cannot undo);
 *   2. pick what to put back: code and chat, chat only, or code only;
 *   3. read the dry run's preview (files restored, removed, not restored and
 *      why; shell commands not undone) and confirm with Enter.
 *
 *   ↑/↓ move   enter next   esc back (closes from the list)
 *
 * The confirm ignores Enter for 300 ms after the preview arrives, so the
 * Enter that asked for it cannot rewind before it was read.
 */

import { TextAttributes, type KeyEvent } from '@opentui/core'
import { useKeyboard } from '@opentui/react'
import React, { useCallback, useEffect, useState } from 'react'

import { truncateToWidth, visibleWindow } from './model-picker'
import { confirmRewind, previewRewind } from '../engine/git-views'
import { useTheme } from '../hooks/use-theme'
import {
  REWIND_MODES,
  rewindModeLabel,
  rewindStep,
  useGitViewsStore,
  type RewindFlow,
  type RewindStep,
} from '../state/git-views-store'
import { stopActiveRun } from '../utils/active-run'
import { rewoundLines, type RewoundTone } from '../utils/git-view-text'
import { formatTimestamp } from '../utils/helpers'
import { BORDER_CHARS } from '../utils/ui-constants'

import type { CheckpointInfo } from '../engine/protocol'

export const REWIND_CONFIRM_GUARD_MS = 300

export type RewindKeyAction =
  | { type: 'move'; delta: number }
  | { type: 'select' }
  | { type: 'back' }
  | { type: 'none' }

/** What a key does at a step of the picker. `armed`: the preview can be confirmed. */
export function resolveRewindKey(
  step: RewindStep,
  key: Pick<KeyEvent, 'name' | 'ctrl' | 'meta'>,
  options: { armed: boolean } = { armed: true },
): RewindKeyAction {
  if (key.name === 'escape' || (key.ctrl && !key.meta && key.name === 'c')) {
    return { type: 'back' }
  }
  if (key.ctrl || key.meta || step === 'running') return { type: 'none' }
  const enter = key.name === 'return' || key.name === 'enter'
  if (step === 'preview') {
    return enter && options.armed ? { type: 'select' } : { type: 'none' }
  }
  if (key.name === 'up') return { type: 'move', delta: -1 }
  if (key.name === 'down') return { type: 'move', delta: 1 }
  if (enter) return { type: 'select' }
  return { type: 'none' }
}

/**
 * Esc at a step: from the list it closes the picker; elsewhere it goes one
 * step back. A rewind already sent cannot be taken back: there Esc and
 * Ctrl-C still interrupt a turn that runs, as they do in the composer.
 */
export function rewindBack(flow: RewindFlow): void {
  const store = useGitViewsStore.getState()
  switch (rewindStep(flow)) {
    case 'pick':
      store.closeRewind()
      return
    case 'mode':
      store.pickCheckpoint(null)
      return
    case 'preview':
      store.pickMode(null)
      return
    case 'running':
      stopActiveRun('user-interrupt')
      return
  }
}

/**
 * Enter at a step, on row `focus`: pick the checkpoint, pick the mode (and
 * ask for its dry run), or confirm the previewed rewind.
 */
export function rewindSelect(flow: RewindFlow, focus: number): void {
  const store = useGitViewsStore.getState()
  switch (rewindStep(flow)) {
    case 'pick': {
      const checkpoint = flow.items[focus]
      if (checkpoint) store.pickCheckpoint(checkpoint)
      return
    }
    case 'mode': {
      const mode = REWIND_MODES[focus]?.mode
      if (!mode) return
      store.pickMode(mode)
      previewRewind()
      return
    }
    case 'preview':
      if (flow.preview) confirmRewind()
      return
    case 'running':
      return
  }
}

/** `14:02`, or `Oct 30 14:02` for an earlier day. */
export function checkpointTime(createdAtMs: number, now: number = Date.now()): string {
  const at = new Date(createdAtMs)
  const time = formatTimestamp(at)
  if (at.toDateString() === new Date(now).toDateString()) return time
  return `${at.toLocaleDateString([], { month: 'short', day: 'numeric' })} ${time}`
}

/** The details after a checkpoint's prompt: its files and shell commands. */
export function checkpointDetail(checkpoint: CheckpointInfo): {
  files: string
  warning: string | null
} {
  const files = checkpoint.files.length
  const shell = checkpoint.shell_commands.length
  return {
    files: files === 0 ? 'no files' : `${files} ${files === 1 ? 'file' : 'files'}`,
    warning:
      shell === 0
        ? null
        : `${shell} shell ${shell === 1 ? 'command' : 'commands'} (not undone)`,
  }
}

export const RewindPicker = ({
  flow,
  width,
  maxVisibleRows = 8,
}: {
  flow: RewindFlow
  width: number
  maxVisibleRows?: number
}) => {
  const theme = useTheme()
  const step = rewindStep(flow)
  const [focus, setFocus] = useState(0)
  const [armed, setArmed] = useState(false)
  const rowCount = step === 'pick' ? flow.items.length : step === 'mode' ? REWIND_MODES.length : 0

  // Each step starts on its first row (the newest checkpoint, code and chat).
  useEffect(() => setFocus(0), [step])
  useEffect(() => {
    setArmed(false)
    if (!flow.preview) return
    const timer = setTimeout(() => setArmed(true), REWIND_CONFIRM_GUARD_MS)
    return () => clearTimeout(timer)
  }, [flow.preview])

  useKeyboard(
    useCallback(
      (key: KeyEvent) => {
        const action = resolveRewindKey(step, key, { armed })
        if (action.type === 'none') return
        key.preventDefault?.()
        if (action.type === 'move') {
          setFocus((index) => Math.max(0, Math.min(rowCount - 1, index + action.delta)))
        } else if (action.type === 'back') {
          rewindBack(flow)
        } else {
          rewindSelect(flow, focus)
        }
      },
      [armed, flow, focus, rowCount, step],
    ),
  )

  const innerWidth = Math.max(10, Math.max(20, width) - 4)
  const pointer = (focused: boolean) => (
    <span fg={focused ? theme.primary : theme.muted}>{focused ? '› ' : '  '}</span>
  )
  const toneColor = (tone: RewoundTone) =>
    tone === 'warning' ? theme.warning : tone === 'muted' ? theme.muted : theme.foreground

  let body: React.ReactNode
  let hint: string
  if (step === 'pick') {
    const [start, end] = visibleWindow(flow.items.length, focus, maxVisibleRows)
    body = flow.items.slice(start, end).map((checkpoint, offset) => {
      const focused = start + offset === focus
      const time = checkpointTime(checkpoint.created_at_ms)
      const detail = checkpointDetail(checkpoint)
      const after = `  ${detail.files}${detail.warning ? `  ${detail.warning}` : ''}`
      const promptWidth = Math.max(8, innerWidth - 2 - time.length - 2 - after.length)
      return (
        <text key={checkpoint.id} style={{ wrapMode: 'none' }}>
          {pointer(focused)}
          <span fg={theme.muted}>{`${time}  `}</span>
          <span
            fg={theme.foreground}
            attributes={focused ? TextAttributes.BOLD : undefined}
          >
            {truncateToWidth(checkpoint.prompt_preview || '(empty prompt)', promptWidth)}
          </span>
          <span fg={theme.muted}>{`  ${detail.files}`}</span>
          {detail.warning ? <span fg={theme.warning}>{`  ${detail.warning}`}</span> : null}
        </text>
      )
    })
    hint = `${flow.items.length > maxVisibleRows ? `${focus + 1}/${flow.items.length} · ` : ''}↑↓ move · enter pick · esc close`
  } else if (step === 'mode') {
    body = REWIND_MODES.map((entry, index) => (
      <text key={entry.mode} style={{ wrapMode: 'none' }}>
        {pointer(index === focus)}
        <span
          fg={theme.foreground}
          attributes={index === focus ? TextAttributes.BOLD : undefined}
        >
          {entry.label}
        </span>
        <span fg={theme.muted}>{`  ${entry.detail}`}</span>
      </text>
    ))
    hint = '↑↓ move · enter preview · esc back'
  } else if (step === 'preview') {
    if (flow.error) {
      body = <text style={{ wrapMode: 'word', fg: theme.warning }}>{flow.error}</text>
      hint = 'esc back'
    } else if (!flow.preview) {
      body = <text style={{ fg: theme.muted }}>Checking what this rewind would do…</text>
      hint = 'esc back'
    } else {
      body = rewoundLines(flow.preview)
        .slice(1)
        .map((line, index) => (
          <text key={index} style={{ wrapMode: 'word', fg: toneColor(line.tone) }}>
            {line.text}
          </text>
        ))
      hint = 'enter rewind · esc back'
    }
  } else {
    body = <text style={{ fg: theme.muted }}>Rewinding…</text>
    hint = ''
  }

  const picked = flow.checkpoint
  return (
    <box
      title=" Rewind "
      titleAlignment="center"
      style={{
        width: Math.max(20, width),
        borderStyle: 'single',
        borderColor: step === 'preview' ? theme.warning : theme.border,
        customBorderChars: BORDER_CHARS,
        paddingLeft: 1,
        paddingRight: 1,
        flexDirection: 'column',
      }}
    >
      {picked && step !== 'pick' ? (
        <text style={{ wrapMode: 'none' }}>
          <span fg={theme.muted}>{`Back to before ${checkpointTime(picked.created_at_ms)}  `}</span>
          <span fg={theme.foreground} attributes={TextAttributes.BOLD}>
            {truncateToWidth(picked.prompt_preview || '(empty prompt)', Math.max(8, innerWidth - 24))}
          </span>
          {flow.mode && step !== 'mode' ? (
            <span fg={theme.muted}>{`  ${rewindModeLabel(flow.mode).toLowerCase()}`}</span>
          ) : null}
        </text>
      ) : null}
      {body}
      {hint ? <text style={{ fg: theme.muted }}>{hint}</text> : null}
    </box>
  )
}
