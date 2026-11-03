// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the
// render options say whether the call is still running. See
// ui/neuron-tui/PORTING.md.
import type { ContentBlock } from '../../types/chat'
import type { ChatTheme } from '../../types/theme-system'
import type { ToolName } from '../../types/agent-run'
import type { ReactNode } from 'react'

export type ToolBlock = Extract<ContentBlock, { type: 'tool' }>

export type ToolRenderOptions = {
  availableWidth: number
  indentationOffset: number
  previewPrefix?: string
  labelWidth: number
  /**
   * The call has no result yet in a running turn: its input is still being
   * written, or it runs.
   */
  isStreaming?: boolean
}

export type ToolRenderConfig = {
  /** Optional path to display in the tool header */
  path?: string
  /** Custom content to render in the tool body */
  content?: ReactNode
  /** Preview text to show when the tool is collapsed */
  collapsedPreview?: string
}

/**
 * Base interface for tool-specific UI components.
 * Implement this interface to create a custom renderer for a specific tool.
 */
export interface ToolComponent<T extends ToolName = ToolName> {
  /** The tool name this component handles */
  toolName: T

  /**
   * Render function that returns configuration for how to display this tool.
   *
   * @param toolBlock - The tool block data containing input/output
   * @param theme - The current chat theme
   * @param options - Rendering options like width and indentation
   * @returns Configuration for rendering the tool, or null to use default rendering
   */
  render(
    toolBlock: ToolBlock & { toolName: T },
    theme: ChatTheme,
    options: ToolRenderOptions,
  ): ToolRenderConfig
}

/**
 * Type-safe tool component definition.
 * Use this helper to create tool components with full type inference.
 */
export function defineToolComponent<T extends ToolName>(
  component: ToolComponent<T>,
): ToolComponent<T> {
  return component
}
