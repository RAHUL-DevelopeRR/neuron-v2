// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the
// clickable footer pattern of Freebuff's cli/src/components/freebuff-chat-footer.tsx
// (a Button around "model • effort · cwd") grew into Neuron's prompt status bar:
// five clickable segments (model, thinking level, workspace and branch,
// permission mode, context meter) and the Zero-X account on the right, fitted
// to the width by utils/status-bar-parts.ts. See ui/neuron-tui/PORTING.md.
/**
 * The line under the input:
 *
 *   G Gemini 2.5 Flash ▾ │ ◑ think high │ ⌂ neuron-v2 ⎇ main +1 │ ✎ Edit workspace │ ●●●◑○○○○○○ 35% ctx     Free · 5h 12% · 37/100 req
 *
 * Each segment opens its picker when clicked (`onActivate`); the same pickers
 * have keys (Alt+M/T/W/P/C, Shift+Tab for the permission mode) and slash
 * commands. It replaces the one-line engine footer; a running turn's live
 * activity stays in the status bar above the input.
 */

import { TextAttributes } from '@opentui/core'
import React, { useState } from 'react'

import { Button } from './button'
import { useTheme } from '../hooks/use-theme'
import { useEngineStore } from '../state/engine-store'
import { getGlyphMode, type GlyphMode } from '../utils/glyphs'
import {
  RIGHT_GAP,
  SEGMENT_SEPARATOR,
  statusBarLine,
  type StatusSegment,
  type StatusTarget,
  type StatusTone,
} from '../utils/status-bar-parts'

const Segment = ({
  segment,
  onActivate,
}: {
  segment: StatusSegment
  onActivate?: (target: StatusTarget) => void
}) => {
  const theme = useTheme()
  const [hovered, setHovered] = useState(false)
  const color = (tone: StatusTone) => theme[tone]
  return (
    <Button
      style={{ flexShrink: 0 }}
      onClick={() => onActivate?.(segment.target)}
      onMouseOver={() => setHovered(true)}
      onMouseOut={() => setHovered(false)}
    >
      <text style={{ wrapMode: 'none' }}>
        {segment.spans.map((span, index) => (
          <span
            key={index}
            fg={color(span.tone)}
            bg={span.chip ? theme.surface : undefined}
            attributes={span.bold || hovered ? TextAttributes.BOLD : TextAttributes.NONE}
          >
            {span.text}
          </span>
        ))}
      </text>
    </Button>
  )
}

export const PromptStatusBar = ({
  width,
  onActivate,
  glyphMode = getGlyphMode(),
}: {
  /** The columns the bar may use, padding included. */
  width: number
  onActivate?: (target: StatusTarget) => void
  /** The terminal's glyph set (tests draw both). */
  glyphMode?: GlyphMode
}) => {
  const theme = useTheme()
  const status = useEngineStore((state) => state.status)
  const account = useEngineStore((state) => state.account)
  const models = useEngineStore((state) => state.models)
  const line = statusBarLine(
    { status, account, models, mode: glyphMode },
    Math.max(0, width - 2),
  )
  if (!line || line.left.length === 0) return null

  return (
    <box
      style={{
        paddingLeft: 1,
        paddingRight: 1,
        flexShrink: 0,
        flexDirection: 'row',
        width,
        overflow: 'hidden',
      }}
    >
      {line.left.map((segment, index) => (
        <React.Fragment key={`${segment.target}-${index}`}>
          {index > 0 ? (
            <text style={{ wrapMode: 'none', flexShrink: 0 }}>
              <span fg={theme.border}>{segment.glue ? ' ' : SEGMENT_SEPARATOR}</span>
            </text>
          ) : null}
          <Segment segment={segment} onActivate={onActivate} />
        </React.Fragment>
      ))}
      {line.right.length > 0 ? (
        <>
          <box style={{ flexGrow: 1, minWidth: RIGHT_GAP }} />
          {line.right.map((segment, index) => (
            <Segment key={`right-${index}`} segment={segment} onActivate={onActivate} />
          ))}
        </>
      ) : null}
    </box>
  )
}
