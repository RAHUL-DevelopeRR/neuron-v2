// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: a refused
// or failed edit shows as one (✗, error colour, the reason), without a diff;
// while the model writes the edit, the row shows its path, size and last
// lines. See ui/neuron-tui/PORTING.md.
import { TextAttributes } from '@opentui/core'

import { DiffViewer } from './diff-viewer'
import { LiveInputPreview } from './live-input'
import { ToolFailureReason, useToolFailure } from './tool-call-item'
import { defineToolComponent } from './types'
import { inputSizeDetail } from '../../engine/live-turn'
import { displayPath } from '../../engine/tool-mapping'
import { useTheme } from '../../hooks/use-theme'
import { GLYPHS } from '../../utils/glyphs'
import {
  extractDiff,
  extractFilePath,
  isCreateFile,
  shouldShowEditDiff,
} from '../../utils/implementor-helpers'

import type { ToolBlock, ToolRenderConfig } from './types'

interface EditHeaderProps {
  name: string
  filePath: string | null
  /** After the path, muted: "412 lines · 12.3 KB". */
  detail?: string | null
}

export const EditHeader = ({ name, filePath, detail }: EditHeaderProps) => {
  const theme = useTheme()
  const failure = useToolFailure()
  const bulletChar = failure ? `${GLYPHS.cross} ` : '• '
  const accent = failure ? theme.error : theme.foreground

  return (
    <box style={{ flexDirection: 'row', alignItems: 'center', width: '100%' }}>
      <text style={{ wrapMode: 'word' }}>
        <span fg={accent}>{bulletChar}</span>
        <span fg={accent} attributes={TextAttributes.BOLD}>
          {name}
        </span>
        {filePath ? <span fg={theme.foreground}>{` ${filePath}`}</span> : null}
        {detail ? <span fg={theme.muted}>{`  ${detail}`}</span> : null}
        {failure ? <span fg={theme.error}>{` ${failure.label}`}</span> : null}
      </text>
    </box>
  )
}

interface EditBodyProps {
  name: string
  filePath: string | null
  diffText: string
  isCreate: boolean
  detail?: string | null
  /** The last lines the model wrote, while it writes them. */
  preview?: string | null
}

export const EditBody = ({
  name,
  filePath,
  diffText,
  isCreate,
  detail,
  preview,
}: EditBodyProps) => {
  const failure = useToolFailure()
  if (failure) {
    return (
      <box style={{ flexDirection: 'column', gap: 0, width: '100%' }}>
        <EditHeader name={name} filePath={filePath} detail={detail} />
        {failure.reason ? <ToolFailureReason reason={failure.reason} /> : null}
      </box>
    )
  }
  return (
    <box style={{ flexDirection: 'column', gap: 0, width: '100%' }}>
      <EditHeader name={name} filePath={filePath} detail={detail} />
      {preview ? <LiveInputPreview preview={preview} /> : null}
      {!isCreate && diffText.length > 0 && (
        <box style={{ paddingLeft: 2, width: '100%' }}>
          <DiffViewer diffText={diffText} />
        </box>
      )}
    </box>
  )
}

/**
 * The path of a file call: from its result or complete input, else what the
 * model has written of it so far.
 */
export function fileCallPath(toolBlock: ToolBlock): string | null {
  const target = toolBlock.liveInput?.target
  return extractFilePath(toolBlock) || (target ? displayPath(target) : null)
}

export const StrReplaceComponent = defineToolComponent({
  toolName: 'str_replace',

  render(toolBlock, _theme, options): ToolRenderConfig {
    const diff = extractDiff(toolBlock)
    const filePath = fileCallPath(toolBlock)
    const isCreate = isCreateFile(toolBlock)
    const showDiff = shouldShowEditDiff(toolBlock)
    const live = toolBlock.liveInput

    return {
      content: (
        <EditBody
          name={isCreate ? 'Create' : 'Edit'}
          filePath={filePath}
          diffText={showDiff ? (diff ?? '') : ''}
          isCreate={isCreate}
          detail={live ? inputSizeDetail(live.lines, live.bytes) : null}
          preview={live && options.isStreaming ? live.preview : null}
        />
      ),
    }
  },
})
