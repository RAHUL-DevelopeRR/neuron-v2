// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: a tool
// result the engine marked an error draws as a failure (✗, error colour,
// "failed" or "refused" and the reason). See ui/neuron-tui/PORTING.md.
import { TextAttributes } from '@opentui/core'
import React, { createContext, useContext, type ReactNode } from 'react'
import stringWidth from 'string-width'

import { useTheme } from '../../hooks/use-theme'
import { Button } from '../button'

import type { ChatTheme } from '../../types/theme-system'

/** Why a tool call shows as failed: the engine refused it, or it ran and failed. */
export interface ToolFailure {
  label: 'refused' | 'failed'
  /** The error text, without an `Error: ` prefix; `null` when there is none. */
  reason: string | null
  /**
   * How a failed command ended: "exit code 2", "timed out", "killed",
   * "interrupted", "blocked: …"; `null` when the engine did not say.
   */
  detail: string | null
}

// The engine's own words for a call it would not run: the permission mode
// ("tool 'write_file' requires workspace-write permission; current mode is
// read-only"), a rule or hook, the user's denial, or no result at all.
const REFUSAL_PATTERN =
  /not allowed in read-only mode|requires '?[\w-]+'? permission|current mode is|requires approval|denied by|has been denied|the call was denied/i

/** The failure to show for a tool block, or `null` for one that succeeded. */
export function toolFailureOf(block: {
  isError?: boolean
  output?: string
  exitCode?: number
  stopReason?: string
}): ToolFailure | null {
  if (!block.isError) return null
  const reason = (block.output ?? '').trim().replace(/^Error:\s*/, '')
  const detail =
    block.exitCode !== undefined ? `exit code ${block.exitCode}` : (block.stopReason ?? null)
  return {
    // A command that reports how it ended was run, whatever its output says.
    label: detail === null && REFUSAL_PATTERN.test(reason) ? 'refused' : 'failed',
    reason: reason || null,
    detail,
  }
}

/** The failure as its row says it: "failed", "refused", "failed (exit code 2)". */
export const failureLabel = (failure: ToolFailure): string =>
  failure.detail ? `${failure.label} (${failure.detail})` : failure.label

/**
 * Set by the tool row around a tool's renderer, so the shared pieces
 * (SimpleToolCallItem, ToolCallItem, the edit header, the terminal command)
 * draw a failed call without every renderer reading the flag itself.
 */
export const ToolFailureContext = createContext<ToolFailure | null>(null)

export const useToolFailure = (): ToolFailure | null =>
  useContext(ToolFailureContext)

const MAX_REASON_LINES = 3

/** The first lines of a failure's reason, under the tool's header. */
export const ToolFailureReason = ({ reason }: { reason: string }) => {
  const theme = useTheme()
  const lines = reason.split('\n')
  const shown =
    lines.length > MAX_REASON_LINES
      ? [...lines.slice(0, MAX_REASON_LINES), '…']
      : lines
  return (
    <box style={{ paddingLeft: 2, width: '100%' }}>
      <text fg={theme.muted} style={{ wrapMode: 'word' }}>
        {shown.join('\n')}
      </text>
    </box>
  )
}

interface ToolCallItemProps {
  name: string
  content: ReactNode
  isCollapsed: boolean
  isStreaming: boolean
  streamingPreview: string
  finishedPreview: string
  onToggle?: () => void
  titleSuffix?: string
  dense?: boolean
}

const isTextRenderable = (value: ReactNode): boolean => {
  if (value === null || value === undefined || typeof value === 'boolean') {
    return false
  }

  if (typeof value === 'string' || typeof value === 'number') {
    return true
  }

  if (Array.isArray(value)) {
    return value.every((child) => isTextRenderable(child))
  }

  if (React.isValidElement(value)) {
    const elProps = value.props as Record<string, unknown>
    if (value.type === React.Fragment) {
      return isTextRenderable(elProps.children as ReactNode)
    }

    if (typeof value.type === 'string') {
      if (
        value.type === 'span' ||
        value.type === 'strong' ||
        value.type === 'em'
      ) {
        return isTextRenderable(elProps.children as ReactNode)
      }

      return false
    }
  }

  return false
}

const renderExpandedContent = (
  value: ReactNode,
  theme: ChatTheme,
  getAttributes: (extra?: number) => number | undefined,
): ReactNode => {
  if (
    value === null ||
    value === undefined ||
    value === false ||
    value === true
  ) {
    return null
  }

  if (isTextRenderable(value)) {
    return (
      <text
        fg={theme.foreground}
        key="tool-expanded-text"
        attributes={getAttributes()}
      >
        {value}
      </text>
    )
  }

  if (React.isValidElement(value)) {
    if (value.key === null || value.key === undefined) {
      return (
        <box
          key="tool-expanded-node"
          style={{ flexDirection: 'column', gap: 0 }}
        >
          {value}
        </box>
      )
    }
    return value
  }

  if (Array.isArray(value)) {
    return (
      <box
        key="tool-expanded-array"
        style={{ flexDirection: 'column', gap: 0 }}
      >
        {value.map((child, idx) => (
          <box
            key={`tool-expanded-array-${idx}`}
            style={{ flexDirection: 'column', gap: 0 }}
          >
            {child}
          </box>
        ))}
      </box>
    )
  }

  return (
    <box
      key="tool-expanded-unknown"
      style={{ flexDirection: 'column', gap: 0 }}
    >
      {value}
    </box>
  )
}

interface SimpleToolCallItemProps {
  name: string
  /** Description - can be a string or ReactNode for rich formatting */
  description: string | ReactNode
  descriptionColor?: string
}

export const SimpleToolCallItem = ({
  name,
  description,
  descriptionColor,
}: SimpleToolCallItemProps) => {
  const theme = useTheme()
  const failure = useToolFailure()
  const bulletChar = failure ? '✗ ' : '• '
  const accent = failure ? theme.error : theme.foreground

  const row = (
    <box style={{ flexDirection: 'row', alignItems: 'center', width: '100%' }}>
      <text style={{ wrapMode: 'word' }}>
        <span fg={accent}>{bulletChar}</span>
        <span fg={accent} attributes={TextAttributes.BOLD}>
          {name}
        </span>
        <span fg={theme.foreground}> </span>
        {typeof description === 'string' ? (
          <span fg={descriptionColor ?? theme.foreground}>{description}</span>
        ) : (
          description
        )}
        {failure ? <span fg={theme.error}>{` ${failureLabel(failure)}`}</span> : null}
      </text>
    </box>
  )

  if (!failure?.reason) return row
  return (
    <box style={{ flexDirection: 'column', gap: 0, width: '100%' }}>
      {row}
      <ToolFailureReason reason={failure.reason} />
    </box>
  )
}

export const ToolCallItem = ({
  name,
  content,
  isCollapsed,
  isStreaming,
  streamingPreview,
  finishedPreview,
  onToggle,
  titleSuffix,
  dense = false,
}: ToolCallItemProps) => {
  const theme = useTheme()
  const failure = useToolFailure()
  const accent = failure ? theme.error : theme.foreground

  const baseTextAttributes = theme.messageTextAttributes ?? 0
  const getAttributes = (extra: number = 0): number | undefined => {
    const combined = baseTextAttributes | extra
    return combined === 0 ? undefined : combined
  }

  const isExpanded = !isCollapsed
  const bulletChar = failure ? '✗ ' : '• '
  const toggleIndicator = onToggle ? (isCollapsed ? '▸ ' : '▾ ') : ''
  const toggleLabel = onToggle ? toggleIndicator : bulletChar
  // Width in cells of the toggle label (toggle arrow or bullet). Used to align
  // expanded content directly under the toggle icon.
  const toggleIndent = stringWidth(toggleLabel)
  const collapsedPreviewText = isStreaming ? streamingPreview : finishedPreview
  const showCollapsedPreview = collapsedPreviewText.length > 0

  return (
    <box style={{ flexDirection: 'column', gap: 0, width: '100%' }}>
      <box
        style={{
          flexDirection: 'column',
          gap: 0,
          paddingLeft: 0,
          paddingRight: 0,
          paddingTop: 0,
          paddingBottom: 0,
          width: '100%',
        }}
      >
        <Button
          style={{
            flexDirection: 'row',
            alignItems: 'center',
            paddingLeft: 0,
            paddingRight: 0,
            paddingTop: 0,
            paddingBottom: 0,
            width: '100%',
          }}
          onClick={onToggle}
        >
          <text style={{ wrapMode: 'none' }}>
            <span
              fg={accent}
              attributes={isExpanded ? TextAttributes.BOLD : undefined}
            >
              {toggleLabel}
            </span>
            <span fg={accent} attributes={TextAttributes.BOLD}>
              {name}
            </span>
            {titleSuffix ? (
              <span fg={theme.foreground} attributes={TextAttributes.BOLD}>
                {` ${titleSuffix}`}
              </span>
            ) : null}
            {failure && !isStreaming ? (
              <span fg={theme.error}>{` ${failureLabel(failure)}`}</span>
            ) : null}
            {isStreaming ? (
              <span fg={theme.primary} attributes={TextAttributes.DIM}>
                {' running'}
              </span>
            ) : null}
          </text>
        </Button>

        {isCollapsed ? (
          showCollapsedPreview ? (
            <box
              style={{
                paddingLeft: 0,
                paddingRight: 0,
                paddingTop: 0,
                paddingBottom: 0,
                width: '100%',
              }}
            >
              <text
                fg={isStreaming ? theme.foreground : theme.muted}
                attributes={getAttributes(TextAttributes.ITALIC)}
                style={{ wrapMode: 'word' }}
              >
                {collapsedPreviewText}
              </text>
            </box>
          ) : null
        ) : (
          <box
            style={{
              flexDirection: 'column',
              gap: 0,
              // Indent expanded content underneath the toggle icon
              paddingLeft: toggleIndent,
              paddingRight: dense ? 0 : toggleIndent,
              paddingTop: 0,
              paddingBottom: 0,
            }}
          >
            {renderExpandedContent(content, theme, getAttributes)}
          </box>
        )}
      </box>
    </box>
  )
}
