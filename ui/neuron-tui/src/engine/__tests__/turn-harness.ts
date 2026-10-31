/**
 * A turn sink over one AI message, wired as use-send-message wires it, that
 * records what the sink asks of the chat (stream status, retrying, streaming
 * rows). For tests of a turn's events.
 */

import { createMessageUpdater } from '../../utils/message-updater'
import { createTurnEventSink, type TurnEventSinkOptions } from '../turn-events'

import type { ChatMessage, ContentBlock, ToolContentBlock } from '../../types/chat'
import type { EventHandlerState } from '../../utils/sdk-event-handlers'
import type { Logger } from '@neuron-tui/common/types/contracts/logger'

export function turnHarness(
  options: TurnEventSinkOptions & { isActive?: () => boolean } = {},
) {
  let messages: ChatMessage[] = [
    { id: 'ai-1', variant: 'ai', content: '', blocks: [], timestamp: 'now' },
  ]
  const noop = () => {}
  const recorded = {
    streamStatus: [] as string[],
    retrying: [] as boolean[],
    streamingRows: new Set<string>(),
  }
  const streamState = {
    rootStreamBuffer: '',
    agentStreamAccumulators: new Map<string, string>(),
    rootStreamSeen: false,
    planExtracted: false,
    wasAbortedByUser: false,
    spawnAgentsMap: new Map(),
  }
  const state = {
    isActive: options.isActive,
    streaming: {
      streamRefs: {
        state: streamState,
        reset: noop,
        setters: {
          setRootStreamBuffer: (value: string) => {
            streamState.rootStreamBuffer = value
          },
          appendRootStreamBuffer: (value: string) => {
            streamState.rootStreamBuffer += value
          },
          setAgentAccumulator: noop,
          removeAgentAccumulator: noop,
          setRootStreamSeen: noop,
          setPlanExtracted: noop,
          setWasAbortedByUser: noop,
          setSpawnAgentInfo: noop,
          removeSpawnAgentInfo: noop,
        },
      },
      setStreamingAgents: (update: (prev: Set<string>) => Set<string>) => {
        recorded.streamingRows = update(recorded.streamingRows)
      },
      setStreamStatus: (status: string) => recorded.streamStatus.push(status),
    },
    message: {
      aiMessageId: 'ai-1',
      updater: createMessageUpdater('ai-1', (fn: (prev: ChatMessage[]) => ChatMessage[]) => {
        messages = fn(messages)
      }),
      hasReceivedContentRef: { current: false },
    },
    subagents: { addActiveSubagent: noop, removeActiveSubagent: noop },
    mode: { agentMode: 'DEFAULT', setHasReceivedPlanResponse: noop },
    logger: { info: noop, warn: noop, error: noop, debug: noop } as unknown as Logger,
    setIsRetrying: (value: boolean) => recorded.retrying.push(value),
  } as unknown as EventHandlerState
  const sink = createTurnEventSink(state, options)
  const blocks = (): ContentBlock[] => messages[0]!.blocks ?? []
  const tools = (): ToolContentBlock[] =>
    blocks().filter((block): block is ToolContentBlock => block.type === 'tool')
  const text = () =>
    blocks()
      .filter((block) => block.type === 'text')
      .map((block) => (block as { content: string }).content)
  return { sink, blocks, tools, text, recorded }
}
