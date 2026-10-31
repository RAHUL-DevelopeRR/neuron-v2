// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: output
// that is itself JSON (`echo 42`, `cat package.json`) is shown as printed, not
// read as an empty result envelope; the command shows while the model writes
// it, then "Running…" until its result. See ui/neuron-tui/PORTING.md.
import { sanitizeTerminalOutput } from '@neuron-tui/common/util/terminal-safe-text'

import { defineToolComponent } from './types'
import { TerminalCommandDisplay } from '../terminal-command-display'

import type { ToolRenderConfig } from './types'

export interface ParsedTerminalOutput {
  output: string | null
  startingCwd?: string
}

/** A result envelope: an object with the tool's stdout, stderr or error. */
const isResultEnvelope = (value: unknown): value is Record<string, any> =>
  typeof value === 'object' &&
  value !== null &&
  !Array.isArray(value) &&
  ('stdout' in value || 'stderr' in value || 'errorMessage' in value)

/**
 * Parse terminal command output from JSON or raw string format.
 * Exported for testing.
 */
export const parseTerminalOutput = (rawOutput: string | undefined): ParsedTerminalOutput => {
  if (!rawOutput) {
    return { output: null }
  }

  try {
    const parsed = JSON.parse(rawOutput)
    // Handle array format [{ type: 'json', value: {...} }]
    const value = Array.isArray(parsed) ? parsed[0]?.value : parsed
    // Anything else that parses (a number, an object the command printed) is
    // the command's own output.
    if (!isResultEnvelope(value)) {
      return { output: rawOutput.trimEnd() || null }
    }
    const startingCwd = value.startingCwd
    // Handle error case
    if (value.errorMessage) {
      return { output: `Error: ${value.errorMessage}`, startingCwd }
    }
    // Combine stdout and stderr for display
    // Use trimEnd() to preserve leading spaces (used for UI elements like trees/tables)
    const stdout = value.stdout || ''
    const stderr = value.stderr || ''
    const output = (stdout + stderr).trimEnd() || null
    return { output, startingCwd }
  } catch {
    // If not JSON, use raw output (preserve leading spaces)
    return { output: rawOutput.trimEnd() || null }
  }
}

/**
 * UI component for run_terminal_command tool.
 * Displays the command in bold next to the bullet point,
 * with the output indented below.
 */
export const RunTerminalCommandComponent = defineToolComponent({
  toolName: 'run_terminal_command',

  render(toolBlock, _theme, options): ToolRenderConfig {
    // Extract command and timeout from input
    const input = toolBlock.input as { command?: string; timeout_seconds?: number } | undefined
    // While the model writes the call, the command as far as it got.
    const live = toolBlock.liveInput
    const written =
      typeof input?.command === 'string' && input.command.trim()
        ? input.command
        : (live?.target ?? live?.preview ?? '')
    const command = sanitizeTerminalOutput(written.trim())
    const timeoutSeconds = typeof input?.timeout_seconds === 'number' ? input.timeout_seconds : undefined

    // Extract output and startingCwd from tool result
    const { output, startingCwd } = parseTerminalOutput(toolBlock.output)

    // Custom content component using shared TerminalCommandDisplay
    const content = (
      <TerminalCommandDisplay
        command={command}
        output={output}
        expandable={true}
        maxVisibleLines={5}
        cwd={startingCwd}
        timeoutSeconds={timeoutSeconds}
        availableWidth={options.availableWidth}
        // Started, no result yet: it runs (or waits for approval).
        isRunning={
          options.isStreaming === true &&
          !live &&
          toolBlock.output === undefined &&
          !toolBlock.stopReason
        }
      />
    )

    return {
      content,
      collapsedPreview: `$ ${command}`,
    }
  },
})
