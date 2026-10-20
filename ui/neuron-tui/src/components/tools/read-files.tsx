// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the
// "(blocked)" and "(allowed - example only)" labels mirrored the Codebuff SDK's
// file filter, which Neuron's engine does not use, so paths are listed plainly.
// See ui/neuron-tui/PORTING.md.
import { readFilePathsOf } from '@codebuff/common/tools/params/tool/read-files'

import { SimpleToolCallItem } from './tool-call-item'
import { defineToolComponent } from './types'

import type { ToolRenderConfig } from './types'

/**
 * UI component for read_files tool.
 * Displays the file paths that were read.
 */
export const ReadFilesComponent = defineToolComponent({
  toolName: 'read_files',

  render(toolBlock): ToolRenderConfig {
    const input = toolBlock.input as any

    // Extract file paths from input
    const filePaths: string[] = readFilePathsOf(input?.paths)

    if (filePaths.length === 0) {
      return { content: null }
    }

    return {
      content: (
        <SimpleToolCallItem name="Read" description={filePaths.join(', ')} />
      ),
    }
  },
})
