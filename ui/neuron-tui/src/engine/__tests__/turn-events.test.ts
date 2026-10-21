import { describe, expect, test } from 'bun:test'

import { createMessageUpdater } from '../../utils/message-updater'
import { createTurnEventSink } from '../turn-events'

import type { ChatMessage, ContentBlock, ToolContentBlock } from '../../types/chat'
import type { EventHandlerState } from '../../utils/sdk-event-handlers'
import type { Logger } from '@neuron-tui/common/types/contracts/logger'

/** A turn sink over one AI message, as use-send-message wires it. */
function turn() {
  let messages: ChatMessage[] = [
    { id: 'ai-1', variant: 'ai', content: '', blocks: [], timestamp: 'now' },
  ]
  const noop = () => {}
  const streamState = {
    rootStreamBuffer: '',
    agentStreamAccumulators: new Map<string, string>(),
    rootStreamSeen: false,
    planExtracted: false,
    wasAbortedByUser: false,
    spawnAgentsMap: new Map(),
  }
  const state = {
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
      setStreamingAgents: noop,
      setStreamStatus: noop,
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
    setIsRetrying: noop,
  } as unknown as EventHandlerState
  const sink = createTurnEventSink(state)
  const blocks = (): ContentBlock[] => messages[0]!.blocks ?? []
  const text = () =>
    blocks()
      .filter((block) => block.type === 'text')
      .map((block) => (block as { content: string }).content)
  return { sink, blocks, text }
}

describe('tool results', () => {
  test('is_error reaches the tool block; a success is not marked', () => {
    const { sink, blocks } = turn()
    sink.handle({
      type: 'tool_call_started',
      id: 'toolu_1',
      name: 'write_file',
      input: '{"path":"ro.txt","content":"x"}',
      cut_off: false,
    })
    sink.handle({
      type: 'tool_call_started',
      id: 'toolu_2',
      name: 'read_file',
      input: '{"path":"a.txt"}',
      cut_off: false,
    })
    sink.handle({
      type: 'tool_result',
      id: 'toolu_1',
      name: 'write_file',
      output: "tool 'write_file' requires workspace-write permission; current mode is read-only",
      is_error: true,
    })
    sink.handle({
      type: 'tool_result',
      id: 'toolu_2',
      name: 'read_file',
      output: JSON.stringify({ file: { content: 'hello' } }),
      is_error: false,
    })
    const [write, read] = blocks() as ToolContentBlock[]
    expect(write).toMatchObject({
      toolName: 'write_file',
      isError: true,
      output:
        "Error: tool 'write_file' requires workspace-write permission; current mode is read-only",
    })
    expect(read).toMatchObject({ toolName: 'read_files', isError: false, output: 'hello' })
  })

  test('a command that failed keeps its output and its error flag', () => {
    const { sink, blocks } = turn()
    sink.handle({
      type: 'tool_call_started',
      id: 'toolu_1',
      name: 'bash',
      input: '{"command":"false"}',
      cut_off: false,
    })
    sink.handle({
      type: 'tool_result',
      id: 'toolu_1',
      name: 'bash',
      output: JSON.stringify({
        stdout: '',
        stderr: 'boom\n',
        interrupted: false,
        returnCodeInterpretation: 'exit_code:1',
      }),
      is_error: true,
    })
    expect(blocks()[0]).toMatchObject({ isError: true, output: 'boom\n' })
  })

  test('calls closed without a result are errors too', () => {
    const { sink, blocks } = turn()
    sink.handle({
      type: 'tool_call_started',
      id: 'toolu_1',
      name: 'bash',
      input: '{"command":"rm -rf x"}',
      cut_off: false,
    })
    sink.finish()
    expect(blocks()[0]).toMatchObject({ isError: true })
  })
})

describe('text blocks', () => {
  test('a new text block with nothing drawn between starts a new paragraph', () => {
    const { sink, text } = turn()
    sink.handle({ type: 'text_delta', text: '**Files:**\nfoo.ts' })
    sink.handle({ type: 'text_block_end' })
    // A thinking block with no visible text in between draws nothing.
    sink.handle({ type: 'reasoning_started', chars: null, redacted: false })
    sink.handle({ type: 'text_delta', text: '**Done**' })
    sink.handle({ type: 'text_block_end' })
    expect(text()).toEqual(['**Files:**\nfoo.ts\n\n**Done**'])
  })

  test('no extra blank lines when the block already ended with them', () => {
    const { sink, text } = turn()
    sink.handle({ type: 'text_delta', text: 'one\n\n' })
    sink.handle({ type: 'text_block_end' })
    sink.handle({ type: 'text_delta', text: 'two\n' })
    sink.handle({ type: 'text_block_end' })
    sink.handle({ type: 'text_delta', text: 'three' })
    expect(text()).toEqual(['one\n\ntwo\n\nthree'])
  })

  test('a tool call between text blocks separates them already', () => {
    const { sink, text } = turn()
    sink.handle({ type: 'text_delta', text: 'Editing.' })
    sink.handle({ type: 'text_block_end' })
    sink.handle({
      type: 'tool_call_started',
      id: 'toolu_1',
      name: 'read_file',
      input: '{"path":"a.txt"}',
      cut_off: false,
    })
    sink.handle({ type: 'text_delta', text: 'Done.' })
    expect(text()).toEqual(['Editing.', 'Done.'])
  })

  test('deltas of one block run on as they came', () => {
    const { sink, text } = turn()
    sink.handle({ type: 'text_delta', text: 'Hello **wor' })
    sink.handle({ type: 'text_delta', text: 'ld**\n- item' })
    expect(text()).toEqual(['Hello **world**\n- item'])
  })
})
