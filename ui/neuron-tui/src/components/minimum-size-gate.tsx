/**
 * The full-screen UI needs a minimum terminal size: below it the prompt, the
 * status bar and the panels overlap one another. A smaller terminal (a narrow
 * embedded pane, a split window) gets a short "make it bigger" screen
 * instead; the chat stays mounted underneath, hidden, so a running turn, the
 * typed draft and the engine carry on, and it comes back as soon as the
 * terminal is big enough again.
 *
 * Hidden is not deaf: every `useKeyboard` handler listens on the renderer's
 * key handler whatever is drawn. While the notice shows, the gate takes every
 * key and paste first and drops it, so a permission prompt, the composer or a
 * panel the user cannot see is never answered. Ctrl-C still goes through, so
 * the user can stop a turn or quit.
 *
 * The launcher (`neuron`, rust full_screen.rs) checks the same size before it
 * starts this UI and runs the line REPL in a smaller terminal.
 */

import { useAppContext } from '@opentui/react'
import React, { useEffect, useRef } from 'react'

import { useTerminalDimensions } from '../hooks/use-terminal-dimensions'
import { useTheme } from '../hooks/use-theme'
import { cutToDisplayWidth } from '../utils/display-width'

/** The fewest columns and rows the UI draws in. Keep in step with full_screen.rs. */
export const MIN_TERMINAL_COLUMNS = 60
export const MIN_TERMINAL_ROWS = 16

/** The terminal is smaller than the UI can draw in. */
export function isTerminalTooSmall(columns: number, rows: number): boolean {
  return columns < MIN_TERMINAL_COLUMNS || rows < MIN_TERMINAL_ROWS
}

/** The lines of the "too small" screen, each cut to fit beside its 1-column margin. */
export function tooSmallLines(columns: number, rows: number): string[] {
  const lines = [
    'Terminal too small',
    `${columns}x${rows}, needs ${MIN_TERMINAL_COLUMNS}x${MIN_TERMINAL_ROWS}`,
    'Make the window bigger,',
    'or run neuron --no-tui.',
  ]
  // A terminal with fewer rows than lines keeps the first ones.
  return lines
    .slice(0, Math.max(1, rows))
    .map((line) => cutToDisplayWidth(line, Math.max(1, columns - 1)))
}

/** A key the too-small screen lets through: Ctrl-C, to stop a turn or quit. */
export function passesTooSmallScreen(key: { name?: string; ctrl?: boolean; meta?: boolean }): boolean {
  return Boolean(key.ctrl) && !key.meta && key.name === 'c'
}

type SwallowableEvent = {
  name?: string
  ctrl?: boolean
  meta?: boolean
  preventDefault?: () => void
  stopPropagation?: () => void
}

export const MinimumSizeGate = ({ children }: { children: React.ReactNode }) => {
  const theme = useTheme()
  const { keyHandler } = useAppContext()
  const { terminalWidth, terminalHeight } = useTerminalDimensions()
  const tooSmall = isTerminalTooSmall(terminalWidth, terminalHeight)
  const tooSmallRef = useRef(tooSmall)
  tooSmallRef.current = tooSmall

  useEffect(() => {
    if (!keyHandler) return
    const swallow = (event: SwallowableEvent) => {
      if (!tooSmallRef.current || passesTooSmallScreen(event)) return
      // Stopped here, no other global handler nor the focused input sees it.
      event.stopPropagation?.()
      event.preventDefault?.()
    }
    // Prepended: the chat's handlers subscribed first (child effects run
    // before this one), and the gate must run before all of them.
    keyHandler.prependListener('keypress', swallow)
    keyHandler.prependListener('paste', swallow)
    return () => {
      keyHandler.off('keypress', swallow)
      keyHandler.off('paste', swallow)
    }
  }, [keyHandler])

  return (
    <>
      {tooSmall ? (
        <box style={{ flexDirection: 'column', width: '100%', height: '100%', paddingLeft: 1 }}>
          {tooSmallLines(terminalWidth, terminalHeight).map((line, index) => (
            <text
              key={index}
              style={{ wrapMode: 'none', fg: index === 0 ? theme.warning : theme.muted }}
            >
              {line}
            </text>
          ))}
        </box>
      ) : null}
      <box
        visible={!tooSmall}
        style={{ flexDirection: 'column', width: '100%', height: '100%' }}
      >
        {children}
      </box>
    </>
  )
}
