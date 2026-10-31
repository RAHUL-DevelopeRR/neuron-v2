/**
 * The pieces of a tool row whose input the model is still writing: the last
 * lines written so far, dimmed under the row's header. The header's own
 * "412 lines · 12.3 KB" comes from engine/live-turn.ts (`inputSizeDetail`).
 */

import { sanitizeTerminalOutput } from '@neuron-tui/common/util/terminal-safe-text'
import { TextAttributes } from '@opentui/core'

import { useTheme } from '../../hooks/use-theme'

import type { ToolContentBlock } from '../../types/chat'

/** How many of the last lines a streaming row shows. */
export const PREVIEW_LINES = 4

/** The last `count` non-empty-at-the-end lines of `text`. */
export function lastLines(text: string, count = PREVIEW_LINES): string[] {
  const lines = text.replace(/\s+$/, '').split(/\r?\n/)
  return lines.slice(-count)
}

/** The block's input so far, while the model still writes it (else `null`). */
export const liveInputOf = (block: ToolContentBlock) => block.liveInput ?? null

/** The last lines written so far, dimmed and indented under the header. */
export const LiveInputPreview = ({ preview }: { preview: string | null }) => {
  const theme = useTheme()
  if (!preview || !preview.trim()) return null
  // Model output drawn as bytes: no escape sequence gets through.
  const lines = lastLines(sanitizeTerminalOutput(preview))
  return (
    <box style={{ paddingLeft: 4, width: '100%', flexDirection: 'column' }}>
      <text fg={theme.muted} attributes={TextAttributes.DIM} style={{ wrapMode: 'none' }}>
        {lines.join('\n')}
      </text>
    </box>
  )
}
