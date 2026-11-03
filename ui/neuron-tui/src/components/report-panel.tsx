/**
 * A command's report above the input (T5, T14): `/usage` (today's requests
 * and tokens against the plan, when they reset, credit, the dashboard),
 * `/init`, `/memory` and `/doctor`. The engine sends it as a `report` event
 * (a title, Markdown sections and links) when the UI announced the `reports`
 * capability. PgUp and PgDn scroll it; Esc closes it, and so does sending the
 * next prompt.
 */

import { TextAttributes, type ScrollBoxRenderable } from '@opentui/core'
import React, { useEffect, useRef } from 'react'

import { TerminalLink } from './terminal-link'
import { useTheme } from '../hooks/use-theme'
import { renderMarkdown } from '../utils/markdown-renderer'
import { safeOpen } from '../utils/open-url'
import { createMarkdownPalette } from '../utils/theme-system'

import type { ReportEvent } from '../engine/protocol'

/** Rows PgUp and PgDn move the report by. */
const PAGE_ROWS = 8

let scrollReport: ((rows: number) => void) | null = null

/**
 * PgUp/PgDn while a report is open: scroll it, not the chat. `false` when no
 * report panel is drawn.
 */
export function scrollOpenReport(direction: 'up' | 'down'): boolean {
  if (!scrollReport) return false
  scrollReport(direction === 'up' ? -PAGE_ROWS : PAGE_ROWS)
  return true
}

export interface ReportPanelProps {
  report: ReportEvent
  width: number
  /** The most rows the panel takes; it scrolls beyond that. */
  maxHeight: number
}

export const ReportPanel = ({ report, width, maxHeight }: ReportPanelProps) => {
  const theme = useTheme()
  const palette = createMarkdownPalette(theme)
  const scrollRef = useRef<ScrollBoxRenderable | null>(null)

  useEffect(() => {
    scrollReport = (rows) => {
      const box = scrollRef.current
      if (box) box.scrollTop = Math.max(0, box.scrollTop + rows)
    }
    return () => {
      scrollReport = null
    }
  }, [])

  const textWidth = Math.max(16, width - 4)
  return (
    <box style={{ flexDirection: 'column', paddingLeft: 1, paddingRight: 1, marginBottom: 1, flexShrink: 0 }}>
      <text style={{ fg: theme.foreground, attributes: TextAttributes.BOLD }}>{report.title}</text>
      <scrollbox
        ref={scrollRef}
        style={{ maxHeight: Math.max(4, maxHeight), width: '100%' }}
        scrollX={false}
      >
        <box style={{ flexDirection: 'column' }}>
          {report.sections.map((section, index) => (
            <box key={`${index}-${section.heading ?? ''}`} style={{ flexDirection: 'column', paddingTop: 1 }}>
              {section.heading ? (
                <text style={{ fg: theme.foreground, attributes: TextAttributes.BOLD }}>{section.heading}</text>
              ) : null}
              <text style={{ wrapMode: 'word', fg: theme.foreground, width: textWidth }}>
                {renderMarkdown(section.markdown, { codeBlockWidth: textWidth, palette })}
              </text>
            </box>
          ))}
          {report.links.length > 0 ? (
            <box style={{ flexDirection: 'column', paddingTop: 1 }}>
              {report.links.map((link) => (
                <box key={link.url} style={{ flexDirection: 'row' }}>
                  <text style={{ fg: theme.muted, flexShrink: 0, marginRight: 1 }}>{`${link.label}:`}</text>
                  <TerminalLink text={link.url} href={link.url} keepScheme={true} onActivate={() => safeOpen(link.url)} />
                </box>
              ))}
            </box>
          ) : null}
        </box>
      </scrollbox>
      <text style={{ fg: theme.muted }}>esc close · pgup/pgdn scroll</text>
    </box>
  )
}
