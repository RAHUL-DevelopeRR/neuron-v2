/**
 * The full-screen UI needs a minimum terminal size: below it the prompt, the
 * status bar and the panels overlap one another. A smaller terminal (a narrow
 * embedded pane, a split window) gets a short "make it bigger" screen
 * instead; the chat stays mounted underneath, hidden, so a running turn, the
 * typed draft and the engine carry on, and it comes back as soon as the
 * terminal is big enough again.
 *
 * The launcher (`neuron`, rust full_screen.rs) checks the same size before it
 * starts this UI and runs the line REPL in a smaller terminal.
 */

import React from 'react'

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

export const MinimumSizeGate = ({ children }: { children: React.ReactNode }) => {
  const theme = useTheme()
  const { terminalWidth, terminalHeight } = useTerminalDimensions()
  const tooSmall = isTerminalTooSmall(terminalWidth, terminalHeight)
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
