/**
 * Asks before a tool call the engine wants approved. It takes the composer's
 * place (and the keyboard) while a request waits:
 *
 *   y  allow this call            a  allow it for the session (when offered)
 *   n  deny (also Esc)            e  deny with a note for the model
 *
 * Ctrl-C interrupts the turn, as it does everywhere else while one runs.
 * Other keys are ignored for the first 300 ms, so a key that was already on
 * its way (typing a prompt) cannot answer a request the user has not read.
 */

import { TextAttributes, type KeyEvent } from '@opentui/core'
import { useKeyboard } from '@opentui/react'
import React, { useCallback, useEffect, useState } from 'react'

import { MultilineInput } from './multiline-input'
import { getNeuronEngineClient } from '../engine/neuron-engine-client'
import { displayPath } from '../engine/tool-mapping'
import { useTheme } from '../hooks/use-theme'
import { useEngineStore } from '../state/engine-store'
import { stopActiveRun } from '../utils/active-run'
import { logger } from '../utils/logger'
import { BORDER_CHARS } from '../utils/ui-constants'

import type {
  PermissionDecision,
  PermissionRequestedEvent,
  PermissionView,
} from '../engine/protocol'

export const PERMISSION_KEY_GUARD_MS = 300

export type PermissionKeyAction =
  | { type: 'decide'; decision: PermissionDecision }
  | { type: 'write-note' }
  | { type: 'interrupt' }
  | { type: 'none' }

const isCtrlC = (key: Pick<KeyEvent, 'name' | 'ctrl' | 'meta'>): boolean =>
  key.ctrl && !key.meta && key.name === 'c'

/** What a key does at the prompt (not while the note is being written). */
export function resolvePermissionKey(
  key: Pick<KeyEvent, 'name' | 'ctrl' | 'meta'>,
  options: { armed: boolean; canAllowForSession: boolean },
): PermissionKeyAction {
  // Never guarded: stopping the turn answers nothing.
  if (isCtrlC(key)) return { type: 'interrupt' }
  if (!options.armed || key.ctrl || key.meta) return { type: 'none' }
  switch (key.name) {
    case 'y':
      return { type: 'decide', decision: { kind: 'allow_once' } }
    case 'a':
      return options.canAllowForSession
        ? { type: 'decide', decision: { kind: 'allow_for_session' } }
        : { type: 'none' }
    case 'n':
    case 'escape':
      return { type: 'decide', decision: { kind: 'deny', note: null } }
    case 'e':
      return { type: 'write-note' }
    default:
      return { type: 'none' }
  }
}

/** What a key does while the deny note is being written. */
export function resolvePermissionNoteKey(
  key: Pick<KeyEvent, 'name' | 'ctrl' | 'meta'>,
): 'interrupt' | 'back' | 'none' {
  if (isCtrlC(key)) return 'interrupt'
  if (key.name === 'escape') return 'back'
  return 'none'
}

/**
 * Ctrl-C at the prompt: interrupt the turn (the engine cancels the call
 * waiting for this answer) and close the prompt. The deny makes sure the
 * engine is not left waiting for an answer if the interrupt finds no turn.
 */
export function interruptAtPermission(id: number): void {
  if (!stopActiveRun('user-interrupt')) {
    try {
      getNeuronEngineClient().interrupt()
    } catch (error) {
      logger.warn({ error }, '[permission] interrupt failed')
    }
  }
  answerPermission(id, { kind: 'deny', note: null })
}

/** The part of a call's input worth showing: the command, path or URL. */
export function summarizePermissionInput(request: PermissionView): string {
  try {
    const input = JSON.parse(request.input) as Record<string, unknown>
    for (const key of ['command', 'path', 'url', 'query', 'pattern', 'skill']) {
      const value = input[key]
      if (typeof value === 'string' && value.trim()) {
        return key === 'path' ? displayPath(value) : value
      }
    }
  } catch {
    // Not JSON: show it as it came.
  }
  return request.input.length > 400
    ? `${request.input.slice(0, 400)}…`
    : request.input
}

export function answerPermission(id: number, decision: PermissionDecision): void {
  try {
    getNeuronEngineClient().replyToPermission({ id, decision })
  } catch (error) {
    logger.warn({ error }, '[permission] reply failed')
  }
  useEngineStore.getState().resolvePermission(id)
}

export const PermissionPrompt = ({
  request,
  width,
}: {
  request: PermissionRequestedEvent
  width: number
}) => {
  const theme = useTheme()
  const [armed, setArmed] = useState(false)
  const [note, setNote] = useState<{ text: string; cursor: number } | null>(
    null,
  )
  const view = request.request
  const canAllowForSession = view.session_prefixes.length > 0

  useEffect(() => {
    setArmed(false)
    setNote(null)
    const timer = setTimeout(() => setArmed(true), PERMISSION_KEY_GUARD_MS)
    return () => clearTimeout(timer)
  }, [request.id])

  useKeyboard(
    useCallback(
      (key: KeyEvent) => {
        if (note) {
          const noteAction = resolvePermissionNoteKey(key)
          if (noteAction === 'none') return
          key.preventDefault?.()
          if (noteAction === 'back') setNote(null)
          else interruptAtPermission(request.id)
          return
        }
        const action = resolvePermissionKey(key, { armed, canAllowForSession })
        if (action.type === 'none') return
        key.preventDefault?.()
        if (action.type === 'write-note') {
          setNote({ text: '', cursor: 0 })
          return
        }
        if (action.type === 'interrupt') {
          interruptAtPermission(request.id)
          return
        }
        answerPermission(request.id, action.decision)
      },
      [armed, canAllowForSession, note, request.id],
    ),
  )

  const summary = summarizePermissionInput(view)
  // No spaces after the last one: they would wrap onto a line of their own.
  const option = (keyLabel: string, label: string, last = false) => (
    <span>
      <span fg={theme.primary} attributes={TextAttributes.BOLD}>
        {keyLabel}
      </span>
      <span fg={theme.foreground}>{last ? ` ${label}` : ` ${label}   `}</span>
    </span>
  )

  return (
    <box
      title=" Permission needed "
      titleAlignment="center"
      style={{
        width: Math.max(20, width),
        borderStyle: 'single',
        borderColor: theme.warning,
        customBorderChars: BORDER_CHARS,
        paddingLeft: 1,
        paddingRight: 1,
        flexDirection: 'column',
      }}
    >
      <text style={{ wrapMode: 'word' }}>
        <span fg={theme.foreground} attributes={TextAttributes.BOLD}>
          {view.tool}
        </span>
        <span fg={theme.muted}>{` needs ${view.required_mode} (now ${view.current_mode})`}</span>
      </text>
      <text style={{ wrapMode: 'word', fg: theme.foreground }}>{summary}</text>
      {view.reason ? (
        <text style={{ wrapMode: 'word', fg: theme.muted }}>{view.reason}</text>
      ) : null}
      {canAllowForSession ? (
        <text style={{ wrapMode: 'word', fg: theme.muted }}>
          {`For the session: ${view.session_prefixes.join(', ')}`}
        </text>
      ) : null}
      {note ? (
        <box style={{ flexDirection: 'column', paddingTop: 1 }}>
          <text style={{ fg: theme.muted }}>
            Why deny? The note goes to the model. Enter sends, Esc goes back,
            Ctrl-C stops the turn.
          </text>
          <MultilineInput
            value={note.text}
            cursorPosition={note.cursor}
            onChange={(value) =>
              setNote({ text: value.text, cursor: value.cursorPosition })
            }
            onSubmit={() =>
              answerPermission(request.id, {
                kind: 'deny',
                note: note.text.trim() || null,
              })
            }
            onPaste={(text) => {
              if (!text) return
              setNote((current) =>
                current
                  ? {
                      text: current.text + text,
                      cursor: current.text.length + text.length,
                    }
                  : current,
              )
            }}
            focused={true}
            maxHeight={4}
            minHeight={1}
            placeholder="Use a dry run first…"
          />
        </box>
      ) : (
        <box style={{ flexDirection: 'column', paddingTop: 1 }}>
          <text>
            {option('y', 'allow once')}
            {canAllowForSession ? option('a', 'allow for session') : null}
            {option('n', 'deny')}
            {option('e', 'deny with a note', true)}
          </text>
          <text style={{ fg: theme.muted }}>esc deny · ctrl-c stop the turn</text>
        </box>
      )}
    </box>
  )
}
