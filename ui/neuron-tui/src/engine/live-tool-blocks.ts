/**
 * Tool blocks that change while their call streams: drawn as soon as the
 * model names the tool, their input growing (`tool_input_progress`), then
 * replaced by the complete input (`tool_call_started` with the same id). An
 * `Agent` call's block also carries its sub-agent. One block per call id,
 * always: these update it in place.
 */

import type {
  ContentBlock,
  SubagentState,
  ToolContentBlock,
  ToolInputSoFar,
} from '../types/chat'

/** Before the first progress: nothing written yet. */
export const NO_INPUT_YET: ToolInputSoFar = {
  bytes: 0,
  lines: 0,
  target: null,
  preview: null,
}

/** The blocks with the tool block of `id` updated (the same array when there is none). */
function updateToolBlock(
  blocks: ContentBlock[],
  id: string,
  update: (block: ToolContentBlock) => ToolContentBlock,
): ContentBlock[] {
  let changed = false
  const next = blocks.map((block) => {
    if (block.type !== 'tool' || block.toolCallId !== id) return block
    const updated = update(block)
    if (updated !== block) changed = true
    return updated
  })
  return changed ? next : blocks
}

/** How far the model got writing the call's input. */
export function withToolInputSoFar(
  blocks: ContentBlock[],
  id: string,
  progress: ToolInputSoFar,
): ContentBlock[] {
  return updateToolBlock(blocks, id, (block) => ({ ...block, liveInput: progress }))
}

/** The call's complete input, in place of what streamed. */
export function withCompleteToolInput(
  blocks: ContentBlock[],
  id: string,
  toolName: string,
  input: Record<string, unknown>,
): ContentBlock[] {
  return updateToolBlock(blocks, id, ({ liveInput: _streamed, ...block }) => ({
    ...block,
    toolName: toolName as ToolContentBlock['toolName'],
    input,
  }))
}

/** The blocks without the calls of `ids` (input of an attempt that is retried). */
export function withoutToolBlocks(
  blocks: ContentBlock[],
  ids: ReadonlySet<string>,
): ContentBlock[] {
  if (ids.size === 0) return blocks
  const kept = blocks.filter(
    (block) => !(block.type === 'tool' && ids.has(block.toolCallId)),
  )
  return kept.length === blocks.length ? blocks : kept
}

/** The sub-agent of an `Agent` call, updated. */
export function withSubagent(
  blocks: ContentBlock[],
  id: string,
  update: (previous: SubagentState | undefined) => SubagentState,
): ContentBlock[] {
  return updateToolBlock(blocks, id, (block) => ({
    ...block,
    subagent: update(block.subagent),
  }))
}

const str = (value: unknown): string | null =>
  typeof value === 'string' && value.trim() ? value : null

/**
 * The sub-agent as an `Agent` call's result reports it (its manifest, final
 * text and note), for engines without the sub-agent events, and for the
 * result text those events left out.
 */
export function subagentFromResult(
  output: string,
  isError: boolean,
  previous: SubagentState | undefined,
): SubagentState {
  let parsed: Record<string, unknown> | null = null
  try {
    const value = JSON.parse(output)
    if (value && typeof value === 'object' && !Array.isArray(value)) parsed = value
  } catch {
    // Not the tool's JSON: an error the engine wrote instead.
  }
  const base: SubagentState = previous ?? {
    description: str(parsed?.description) ?? '',
    subagentType: str(parsed?.subagentType),
    status: 'running',
    finished: false,
    isError: false,
    result: null,
    note: null,
  }
  if (previous?.finished) {
    // The sub-agent's own end was reported; the result only adds its text.
    return { ...previous, result: previous.result ?? str(parsed?.result) }
  }
  const status = str(parsed?.status) ?? (isError ? 'failed' : 'completed')
  return {
    ...base,
    status,
    finished: true,
    isError: isError || status === 'failed',
    result: str(parsed?.result),
    note: str(parsed?.note) ?? str(parsed?.error) ?? (parsed ? null : str(output)),
  }
}
