// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: Neuron's
// shortcuts and commands; the new-line key is "Alt+Enter" outside macOS
// ("Opt+Enter" there), and /bash is not listed while shell commands are not
// implemented (USER_SHELL_AVAILABLE). See ui/neuron-tui/PORTING.md.
import React from 'react'
import { TextAttributes } from '@opentui/core'

import { useTheme } from '../hooks/use-theme'
import { useTerminalDimensions } from '../hooks/use-terminal-dimensions'
import { USER_SHELL_AVAILABLE } from '../utils/user-shell'

export interface HelpSection {
  title: string
  rows: ReadonlyArray<readonly [key: string, description: string]>
}

/** The help's shortcut groups, worded for `platform`'s keyboard. */
export function helpSections(
  platform: NodeJS.Platform = process.platform,
  shellAvailable: boolean = USER_SHELL_AVAILABLE,
): HelpSection[] {
  return [
    {
      title: 'Compose',
      rows: [
        ['/', 'Commands'],
        // `@path` puts the path in the prompt; the agent reads the file if it needs it.
        ['@file', "Insert a file's path"],
        ['/login', 'Sign in to Zero-X'],
        ['Ctrl+J', 'New line'],
        // Option is a Mac key; Windows and Linux keyboards say Alt.
        [platform === 'darwin' ? 'Opt+Enter' : 'Alt+Enter', 'New line'],
        ...(shellAvailable ? ([['/bash', 'Shell command']] as const) : []),
      ],
    },
    {
      title: 'Chat',
      rows: [
        ['?', 'Help (empty input)'],
        ['←', 'History (empty input)'],
        ['↑ / ↓', 'Prompts (empty input)'],
        ['/new', 'New chat'],
        ['/model', 'Pick the model'],
        ['Ctrl+Q', 'Edit queued messages'],
      ],
    },
    {
      title: 'Conversation',
      rows: [
        ['Esc', 'Close help / stop'],
        ['Ctrl+C', 'Clear / stop / quit'],
        ['Ctrl+T', 'Expand / collapse agents'],
        ['/copy', 'Copy chat'],
        ['/export', 'Export chat'],
        ['Drag', 'Select and copy text'],
      ],
    },
  ]
}

const SECTIONS = helpSections()

/** Aligned shortcut columns; wrap whole columns on smaller terminals. */
export const HelpBanner = () => {
  const theme = useTheme()
  const { terminalWidth, terminalHeight } = useTerminalDimensions()
  const width = Math.max(16, terminalWidth - 4)
  const columnWidth = Math.min(37, width)
  const columns = Math.max(1, Math.floor((width + 2) / (columnWidth + 2)))
  const contentRows = Math.ceil(SECTIONS.length / columns) * 8 - 1
  return (
    <box
      style={{
        flexDirection: 'column',
        paddingLeft: 1,
        paddingRight: 1,
        marginBottom: 1,
        flexShrink: 0,
      }}
    >
      <text
        style={{
          fg: theme.foreground,
          attributes: TextAttributes.BOLD,
          marginBottom: 1,
        }}
      >
        Keyboard shortcuts
      </text>
      <scrollbox
        style={{
          height: Math.min(contentRows, Math.max(5, Math.floor(terminalHeight / 2))),
          width: '100%',
        }}
        scrollX={false}
      >
        <box
          style={{
            flexDirection: 'row',
            flexWrap: 'wrap',
            columnGap: 2,
            rowGap: 1,
          }}
        >
          {SECTIONS.map((section) => (
            <box
              key={section.title}
              style={{
                flexDirection: 'column',
                width: columnWidth,
                flexShrink: 0,
              }}
            >
              <text
                style={{
                  fg: theme.foreground,
                  attributes: TextAttributes.BOLD,
                }}
              >
                {section.title}
              </text>
              {section.rows.map(([key, description]) => (
                  <box key={key} style={{ flexDirection: 'row' }}>
                    <text
                      style={{ fg: theme.primary, width: 11, flexShrink: 0 }}
                    >
                      {key}
                    </text>
                    <text
                      style={{
                        fg: theme.muted,
                        flexShrink: 1,
                        flexGrow: 1,
                        flexBasis: 0,
                        wrapMode: 'word',
                      }}
                    >
                      {description}
                    </text>
                  </box>
                ))}
            </box>
          ))}
        </box>
      </scrollbox>
    </box>
  )
}
