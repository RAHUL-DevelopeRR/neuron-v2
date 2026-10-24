/**
 * "Before Neuron works here" (T1): drawn in place of the chat while the folder
 * is not trusted. It names the folder (and the git root trust is recorded
 * for), says what Neuron may do there and what the project's own settings
 * would turn on (the engine's `trust_preview`), and offers *Allow Neuron in
 * this folder* or *Quit*. Until the user allows it, the folder's hooks, MCP
 * servers and permission rules stay off.
 *
 * The words are the shared trust copy (`@neuron/client/copy/trust`), which the
 * desktop's trust dialog uses too.
 */

import { TextAttributes, type KeyEvent } from '@opentui/core'
import { useKeyboard } from '@opentui/react'
import React, { useCallback, useState } from 'react'

import {
  TRUST_DOCS_URL,
  trustChoices,
  trustCopy,
  trustPreviewLines,
  trustRootNote,
  type TrustChoice,
} from '@neuron/client/copy/trust'

import { TerminalLink } from './terminal-link'
import { useTheme } from '../hooks/use-theme'
import { getGlyphMode, type GlyphMode } from '../utils/glyphs'
import { safeOpen } from '../utils/open-url'

import type { TrustPreviewEvent } from '../engine/protocol'
import type { TrustStep } from '../state/start-gate'

const CHOICES = trustChoices('terminal')

/**
 * The key hint for `glyphMode`: a classic console window showed `↑↓` as `??`
 * (the dash and the dot came through), so there the arrows are words.
 */
export function trustKeysHint(glyphMode: GlyphMode): string {
  return glyphMode === 'console'
    ? trustCopy.terminalKeys.replace('↑↓', 'Up/Down')
    : trustCopy.terminalKeys
}

/** What a key does on the trust screen, with `selected` the highlighted choice. */
export type TrustKeyAction =
  | { type: 'select'; index: number }
  | { type: 'choose'; choice: TrustChoice['id'] }
  | { type: 'none' }

export function trustKeyAction(key: Pick<KeyEvent, 'name' | 'ctrl' | 'sequence'>, selected: number): TrustKeyAction {
  if (key.name === 'escape' || (key.ctrl && key.name === 'c')) return { type: 'choose', choice: 'quit' }
  const digit = Number(key.sequence ?? key.name)
  if (Number.isInteger(digit) && digit >= 1 && digit <= CHOICES.length) {
    return { type: 'choose', choice: CHOICES[digit - 1]!.id }
  }
  // Up and down stop at the ends: with two choices, wrapping would make each
  // key do the same thing.
  if (key.name === 'up') return { type: 'select', index: Math.max(0, selected - 1) }
  if (key.name === 'down') return { type: 'select', index: Math.min(CHOICES.length - 1, selected + 1) }
  if (key.name === 'return' || key.name === 'enter') {
    return { type: 'choose', choice: CHOICES[selected]!.id }
  }
  return { type: 'none' }
}

export interface TrustScreenProps {
  /** The folder Neuron runs in. */
  cwd: string
  /** What trusting it would turn on; `null` until the engine says (or when it cannot). */
  preview: TrustPreviewEvent | null
  /** The engine was asked for the preview and has not answered yet. */
  previewPending?: boolean
  step: TrustStep
  width: number
  onAllow: () => void
  onQuit: () => void
  /** Test seam: the console glyph set marks the choice with `>`. */
  glyphMode?: GlyphMode
}

export const TrustScreen = ({
  cwd,
  preview,
  previewPending = false,
  step,
  width,
  onAllow,
  onQuit,
  glyphMode = getGlyphMode(),
}: TrustScreenProps) => {
  const theme = useTheme()
  const [selected, setSelected] = useState(0)
  const busy = step.kind === 'saving' || step.kind === 'restarting'

  useKeyboard(
    useCallback(
      (key: KeyEvent) => {
        const action = trustKeyAction(key, selected)
        if (action.type === 'none') return
        key.preventDefault?.()
        // Quitting is always possible; nothing else while trust is saved.
        if (action.type === 'choose' && action.choice === 'quit') return onQuit()
        if (busy) return
        if (action.type === 'select') return setSelected(action.index)
        if (action.choice === 'allow') onAllow()
      },
      [busy, onAllow, onQuit, selected],
    ),
  )

  const marker = glyphMode === 'console' ? '>' : '›'
  const textWidth = Math.max(20, width - 4)
  const note = preview ? trustRootNote(preview) : ''
  const turnsOn = preview ? trustPreviewLines(preview) : null

  return (
    <box style={{ flexDirection: 'column', paddingLeft: 2, paddingRight: 2, paddingTop: 1 }}>
      <text style={{ fg: theme.foreground, attributes: TextAttributes.BOLD }}>{trustCopy.title}</text>
      <text style={{ wrapMode: 'char', paddingTop: 1, width: textWidth }}>
        <span fg={theme.foreground}>{cwd}</span>
        {note ? <span fg={theme.muted}>{`   ${note}`}</span> : null}
      </text>
      <text style={{ wrapMode: 'word', paddingTop: 1, fg: theme.foreground, width: textWidth }}>
        {trustCopy.access}
      </text>
      {preview?.session_only ? (
        <text style={{ wrapMode: 'word', fg: theme.foreground, width: textWidth }}>{trustCopy.sessionOnly}</text>
      ) : null}
      {turnsOn === null ? (
        previewPending ? (
          <text style={{ fg: theme.muted }}>Checking what this project would turn on…</text>
        ) : null
      ) : turnsOn.length === 0 ? (
        <text style={{ wrapMode: 'word', fg: theme.foreground, width: textWidth }}>{trustCopy.turnsOnNothing}</text>
      ) : (
        <>
          <text style={{ fg: theme.foreground }}>{trustCopy.turnsOnHeading}</text>
          <box style={{ flexDirection: 'column', paddingLeft: 2 }}>
            {turnsOn.map((line) => (
              <text key={line} style={{ wrapMode: 'word', fg: theme.foreground, width: textWidth - 2 }}>
                {`• ${line}`}
              </text>
            ))}
          </box>
        </>
      )}
      <text style={{ wrapMode: 'word', fg: theme.warning, width: textWidth }}>{trustCopy.caution}</text>
      <box style={{ flexDirection: 'row' }}>
        <text style={{ fg: theme.foreground, flexShrink: 0, marginRight: 1 }}>{trustCopy.whyLabel}</text>
        <TerminalLink text={trustCopy.whyLink} href={TRUST_DOCS_URL} onActivate={() => safeOpen(TRUST_DOCS_URL)} />
      </box>
      <box style={{ flexDirection: 'column', paddingTop: 1 }}>
        {CHOICES.map((choice, index) => {
          const isSelected = index === selected
          return (
            <text key={choice.id} style={{ fg: isSelected ? theme.primary : theme.foreground }}>
              <span>{isSelected ? `${marker} ` : '  '}</span>
              <span attributes={isSelected ? TextAttributes.BOLD : undefined}>{`${index + 1}  ${choice.label}`}</span>
            </text>
          )
        })}
      </box>
      <box style={{ paddingTop: 1, flexDirection: 'column' }}>
        {step.kind === 'restarting' || step.kind === 'saving' ? (
          <text style={{ fg: theme.muted }}>{trustCopy.restarting}</text>
        ) : step.kind === 'failed' ? (
          <>
            <text style={{ wrapMode: 'word', fg: theme.error, width: textWidth }}>{trustCopy.failed}</text>
            <text style={{ wrapMode: 'word', fg: theme.muted, width: textWidth }}>{step.error}</text>
          </>
        ) : (
          <text style={{ fg: theme.muted }}>{trustKeysHint(glyphMode)}</text>
        )}
      </box>
    </box>
  )
}
