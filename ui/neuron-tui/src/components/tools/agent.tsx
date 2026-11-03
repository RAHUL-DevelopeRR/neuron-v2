/**
 * An `Agent` call: the sub-agent it runs, by description and type, whether it
 * still runs, and how it ended (the first lines of its answer, or why it
 * stopped). The engine reports the sub-agent with `subagent_started` and
 * `subagent_finished`; older engines only with the call's result, which the
 * turn reads the same way (engine/live-tool-blocks.ts).
 */

import { sanitizeTerminalOutput } from '@neuron-tui/common/util/terminal-safe-text'
import { TextAttributes } from '@opentui/core'

import { ToolFailureReason, failureLabel, useToolFailure } from './tool-call-item'
import { defineToolComponent } from './types'
import { useTheme } from '../../hooks/use-theme'
import { GLYPHS } from '../../utils/glyphs'

import type { ToolBlock, ToolRenderConfig } from './types'
import type { SubagentState } from '../../types/chat'
import type { ToolName } from '../../types/agent-run'

/** The engine's tool name: the row is drawn under it (no Freebuff tool matches). */
export const AGENT_TOOL = 'Agent' as ToolName

/** How many lines of the sub-agent's answer the row shows. */
const RESULT_LINES = 3

const str = (value: unknown): string | null =>
  typeof value === 'string' && value.trim() ? value.trim() : null

/** What the row shows: the sub-agent as reported, else as the call describes it. */
export function subagentOf(block: ToolBlock): SubagentState {
  const input = (block.input ?? {}) as Record<string, unknown>
  const reported = block.subagent
  const finished = reported?.finished ?? block.output !== undefined
  return {
    description: str(reported?.description) ?? str(input.description) ?? '',
    subagentType: reported?.subagentType ?? str(input.subagent_type),
    status: reported?.status ?? (finished ? (block.isError ? 'failed' : 'completed') : 'running'),
    finished,
    isError: reported?.isError ?? block.isError === true,
    result: reported?.result ?? null,
    note: reported?.note ?? null,
  }
}

/** The first lines of `text`, with "…" when there is more. */
const firstLines = (text: string, count: number): string => {
  const lines = sanitizeTerminalOutput(text).trim().split(/\r?\n/)
  return lines.length > count ? [...lines.slice(0, count), '…'].join('\n') : lines.join('\n')
}

const AgentRow = ({ block }: { block: ToolBlock }) => {
  const theme = useTheme()
  const failure = useToolFailure()
  const agent = subagentOf(block)
  const failed = failure !== null || agent.isError
  const accent = failed ? theme.error : theme.foreground
  const bullet = failed ? `${GLYPHS.cross} ` : '• '
  const inBackground = agent.finished && agent.status === 'running'
  const state = !agent.finished
    ? 'running…'
    : failure
      ? failureLabel(failure)
      : agent.isError
        ? agent.status
        : inBackground
          ? 'continues in the background'
          : null
  // The answer, else why it stopped; a failure's reason when the call
  // itself failed and the sub-agent said nothing.
  const body = agent.result ?? agent.note
  return (
    <box style={{ flexDirection: 'column', gap: 0, width: '100%' }}>
      <text style={{ wrapMode: 'word' }}>
        <span fg={accent}>{bullet}</span>
        <span fg={accent} attributes={TextAttributes.BOLD}>
          Agent
        </span>
        {agent.description ? (
          <span fg={theme.foreground}>{` ${sanitizeTerminalOutput(agent.description)}`}</span>
        ) : null}
        {agent.subagentType ? (
          <span fg={theme.muted}>{` (${sanitizeTerminalOutput(agent.subagentType)})`}</span>
        ) : null}
        {state ? <span fg={failed ? theme.error : theme.muted}>{`  ${state}`}</span> : null}
      </text>
      {body ? (
        <box style={{ paddingLeft: 2, width: '100%' }}>
          <text fg={theme.muted} style={{ wrapMode: 'word' }}>
            {firstLines(body, RESULT_LINES)}
          </text>
        </box>
      ) : failure?.reason ? (
        <ToolFailureReason reason={failure.reason} />
      ) : null}
    </box>
  )
}

export const AgentComponent = defineToolComponent({
  toolName: AGENT_TOOL,

  render(toolBlock): ToolRenderConfig {
    const agent = subagentOf(toolBlock)
    return {
      content: <AgentRow block={toolBlock} />,
      collapsedPreview: agent.description,
    }
  },
})
