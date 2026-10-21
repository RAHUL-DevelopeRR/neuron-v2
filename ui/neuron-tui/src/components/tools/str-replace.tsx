// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: a refused
// or failed edit shows as one (✗, error colour, the reason), without a diff.
// See ui/neuron-tui/PORTING.md.
import { TextAttributes } from '@opentui/core'

import { DiffViewer } from './diff-viewer'
import { ToolFailureReason, useToolFailure } from './tool-call-item'
import { defineToolComponent } from './types'
import { useTheme } from '../../hooks/use-theme'
import {
  extractDiff,
  extractFilePath,
  isCreateFile,
  shouldShowEditDiff,
} from '../../utils/implementor-helpers'

import type { ToolRenderConfig } from './types'

interface EditHeaderProps {
  name: string
  filePath: string | null
}

const EditHeader = ({ name, filePath }: EditHeaderProps) => {
  const theme = useTheme()
  const failure = useToolFailure()
  const bulletChar = failure ? '✗ ' : '• '
  const accent = failure ? theme.error : theme.foreground

  return (
    <box style={{ flexDirection: 'row', alignItems: 'center', width: '100%' }}>
      <text style={{ wrapMode: 'word' }}>
        <span fg={accent}>{bulletChar}</span>
        <span fg={accent} attributes={TextAttributes.BOLD}>
          {name}
        </span>
        {filePath ? <span fg={theme.foreground}>{` ${filePath}`}</span> : null}
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
}

const EditBody = ({ name, filePath, diffText, isCreate }: EditBodyProps) => {
  const failure = useToolFailure()
  if (failure) {
    return (
      <box style={{ flexDirection: 'column', gap: 0, width: '100%' }}>
        <EditHeader name={name} filePath={filePath} />
        {failure.reason ? <ToolFailureReason reason={failure.reason} /> : null}
      </box>
    )
  }
  return (
    <box style={{ flexDirection: 'column', gap: 0, width: '100%' }}>
      <EditHeader name={name} filePath={filePath} />
      {!isCreate && diffText.length > 0 && (
        <box style={{ paddingLeft: 2, width: '100%' }}>
          <DiffViewer diffText={diffText} />
        </box>
      )}
    </box>
  )
}

export const StrReplaceComponent = defineToolComponent({
  toolName: 'str_replace',

  render(toolBlock): ToolRenderConfig {
    const diff = extractDiff(toolBlock)
    const filePath = extractFilePath(toolBlock)
    const isCreate = isCreateFile(toolBlock)
    const showDiff = shouldShowEditDiff(toolBlock)

    return {
      content: (
        <EditBody
          name={isCreate ? 'Create' : 'Edit'}
          filePath={filePath}
          diffText={showDiff ? (diff ?? '') : ''}
          isCreate={isCreate}
        />
      ),
    }
  },
})
