// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: a written
// file reads "Write <path>" until its result says it was created, with how
// much was written (as it streams, then from the result) and a diff only
// when the engine sends one, not the whole file as added lines.
// See ui/neuron-tui/PORTING.md.
import { EditBody, StrReplaceComponent, fileCallPath } from './str-replace'
import { defineToolComponent } from './types'
import { inputSizeDetail } from '../../engine/live-turn'
import { extractValueForKey, isCreateFile } from '../../utils/implementor-helpers'

import type { ToolBlock, ToolRenderConfig } from './types'

const count = (value: string | null): number | null => {
  if (value === null) return null
  const parsed = Number(value)
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : null
}

/** Lines of `content` as the engine counts them (a final newline ends a line). */
const lineCount = (content: string): number =>
  content === '' ? 0 : content.replace(/\r?\n$/, '').split(/\r?\n/).length

/**
 * How much the call writes: from its result, else from the input while it
 * streams, else from its complete content.
 */
export function writtenSize(toolBlock: ToolBlock): string | null {
  const output = typeof toolBlock.output === 'string' ? toolBlock.output : ''
  const lines = count(extractValueForKey(output, 'lines'))
  const bytes = count(extractValueForKey(output, 'bytes'))
  if (lines !== null || bytes !== null) return inputSizeDetail(lines ?? 0, bytes ?? 0)
  const live = toolBlock.liveInput
  if (live) return inputSizeDetail(live.lines, live.bytes)
  const content = (toolBlock.input as { content?: unknown } | undefined)?.content
  if (typeof content !== 'string' || content === '') return null
  return inputSizeDetail(lineCount(content), Buffer.byteLength(content, 'utf8'))
}

export const WriteFileComponent = defineToolComponent({
  toolName: 'write_file',

  render(toolBlock, theme, options): ToolRenderConfig {
    // A proposal shows its content as the diff, as it always did.
    if (toolBlock.toolName !== 'write_file') {
      return StrReplaceComponent.render(toolBlock as any, theme, options)
    }
    const output = typeof toolBlock.output === 'string' ? toolBlock.output : ''
    const live = toolBlock.liveInput
    return {
      content: (
        <EditBody
          name={isCreateFile(toolBlock) ? 'Create' : 'Write'}
          filePath={fileCallPath(toolBlock)}
          diffText={extractValueForKey(output, 'unifiedDiff') ?? ''}
          isCreate={false}
          // A write that failed wrote nothing: its reason says why.
          detail={toolBlock.isError ? null : writtenSize(toolBlock)}
          preview={live && options.isStreaming ? live.preview : null}
        />
      ),
    }
  },
})
