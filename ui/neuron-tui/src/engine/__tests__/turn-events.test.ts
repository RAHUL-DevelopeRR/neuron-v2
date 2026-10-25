import { beforeEach, describe, expect, test } from 'bun:test'

import { useEngineStore } from '../../state/engine-store'
import { createMessageUpdater } from '../../utils/message-updater'
import { withQuestionAnswer } from '../questions'
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
  /** Changes the chat outside the turn, as the router does with an answer. */
  const update = (fn: (prev: ChatMessage[]) => ChatMessage[] | null) => {
    messages = fn(messages) ?? messages
  }
  /** The turn's AI message as the chat holds it. */
  const message = () => messages[0]!
  return { sink, blocks, text, update, message }
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

describe('reasoning', () => {
  test('streams into a thinking block apart from the answer', () => {
    const { sink, blocks } = turn()
    sink.handle({ type: 'reasoning_started', chars: 0, redacted: false })
    sink.handle({ type: 'reasoning_delta', text: 'The user asks for 17 * 23.' })
    sink.handle({ type: 'reasoning_delta', text: ' That is 391.\n\n' })
    sink.handle({ type: 'text_delta', text: '17 * 23 = ' })
    sink.handle({ type: 'text_delta', text: '391.' })
    sink.handle({ type: 'text_block_end' })

    const [thinking, answer] = blocks()
    expect(blocks()).toHaveLength(2)
    expect(thinking).toMatchObject({ type: 'text', textType: 'reasoning' })
    expect((thinking as { content: string }).content).toBe(
      'The user asks for 17 * 23. That is 391.\n\n',
    )
    expect(answer).toMatchObject({ type: 'text', content: '17 * 23 = 391.' })
    expect((answer as { textType?: string }).textType).not.toBe('reasoning')
    // The answer's text blocks hold no reasoning.
    const answerText = blocks()
      .filter((block) => block.type === 'text' && block.textType !== 'reasoning')
      .map((block) => (block as { content: string }).content)
      .join('')
    expect(answerText).toBe('17 * 23 = 391.')
  })
})

describe('a refusal', () => {
  const REFUSAL =
    'The model declined to continue this request. Declined (safety category: cyber). Rephrase it or choose another model.'

  test('ends the message apart from the text the model wrote before', () => {
    const { sink, text, message } = turn()
    sink.handle({ type: 'text_delta', text: 'Here is how the lock works. First,' })
    sink.handle({ type: 'text_block_end' })
    sink.handle({ type: 'notice', message: REFUSAL, kind: 'refusal' })

    expect(text()).toEqual(['Here is how the lock works. First,'])
    expect(message().refusal).toBe(REFUSAL)
  })

  test('before any text marks the message instead of leaving it empty', () => {
    const { sink, blocks, message } = turn()
    sink.handle({ type: 'notice', message: REFUSAL, kind: 'refusal' })

    expect(blocks()).toEqual([])
    expect(message().refusal).toBe(REFUSAL)
  })

  test('is only a notice of the refusal kind', () => {
    const { sink, message } = turn()
    sink.handle({ type: 'notice', message: REFUSAL })
    sink.handle({ type: 'notice', message: REFUSAL, kind: null })
    expect(message().refusal).toBeUndefined()
  })
})

describe('a retried model call', () => {
  const STREAM_RETRY =
    '\n⚠ The response stream was interrupted (Generation interrupted.) — retrying once…'

  test("replaces what the failed attempt streamed instead of running on into it", () => {
    const { sink, blocks, text } = turn()
    sink.handle({ type: 'reasoning_delta', text: 'The user asks: 17 * 23. No extra text.' })
    sink.handle({ type: 'notice', message: STREAM_RETRY })
    sink.handle({ type: 'reasoning_delta', text: 'The user asks: compute 17*23 = 391.' })
    sink.handle({ type: 'text_delta', text: '391' })
    expect(text()).toEqual(['The user asks: compute 17*23 = 391.', '391'])
    expect(blocks()).toHaveLength(2)
  })

  test('keeps what came before the attempt: earlier text, tool calls and their results', () => {
    const { sink, blocks, text } = turn()
    sink.handle({ type: 'text_delta', text: 'Reading it.' })
    sink.handle({ type: 'text_block_end' })
    sink.handle({
      type: 'tool_call_started',
      id: 'toolu_1',
      name: 'read_file',
      input: '{"path":"a.txt"}',
      cut_off: false,
    })
    sink.handle({
      type: 'tool_result',
      id: 'toolu_1',
      name: 'read_file',
      output: JSON.stringify({ file: { content: 'hello' } }),
      is_error: false,
    })
    sink.handle({ type: 'text_delta', text: 'It says hel' })
    sink.handle({ type: 'notice', message: 'Zero-X upstream_timeout; retrying in 1.3s (retry 1 of 2)' })
    sink.handle({ type: 'text_delta', text: 'It says hello.' })
    expect(text()).toEqual(['Reading it.', 'It says hello.'])
    expect(blocks().map((block) => block.type)).toEqual(['text', 'tool', 'text'])
  })

  test('other notices change nothing', () => {
    const { sink, text } = turn()
    sink.handle({ type: 'text_delta', text: 'Partial' })
    sink.handle({ type: 'notice', message: 'Compacted 12 messages.' })
    sink.handle({ type: 'text_delta', text: ' answer' })
    expect(text()).toEqual(['Partial answer'])
  })
})

describe('AskUserQuestion', () => {
  const QUESTION =
    'Question: May I run the command?\n  1. Yes\n  2. No\n  Answer with 1-2 or in your own words.'

  beforeEach(() => useEngineStore.getState().reset())

  test('the question takes the tool row’s place, and the reply to it comes after', () => {
    const { sink, blocks, update } = turn()
    sink.handle({ type: 'text_delta', text: 'I need your approval.' })
    sink.handle({ type: 'text_block_end' })
    sink.handle({
      type: 'tool_call_started',
      id: 'toolu_q',
      name: 'AskUserQuestion',
      input: '{"question":"May I run the command?","options":["Yes","No"]}',
      cut_off: false,
    })
    useEngineStore.getState().setPendingQuestion(QUESTION)
    sink.handle({ type: 'notice', message: QUESTION })
    expect(useEngineStore.getState().pendingQuestionTarget).toEqual({
      messageId: 'ai-1',
      toolCallId: 'toolu_q',
    })
    expect(blocks()[1]).toEqual({
      type: 'ask-user',
      toolCallId: 'toolu_q',
      questions: [{ question: 'May I run the command?', options: [{ label: 'Yes' }, { label: 'No' }] }],
      hint: 'Answer with 1-2 or in your own words.',
    })

    // The user types "2" in the input.
    update((prev) => withQuestionAnswer(prev, { messageId: 'ai-1', toolCallId: 'toolu_q' }, '2'))
    sink.handle({ type: 'tool_result', id: 'toolu_q', name: 'AskUserQuestion', output: '2', is_error: false })
    sink.handle({ type: 'text_delta', text: 'You said no, so I did not run it.' })
    sink.finish()

    expect(blocks().map((block) => block.type)).toEqual(['text', 'ask-user', 'text'])
    expect(blocks()[1]).toMatchObject({
      answers: [{ questionIndex: 0, selectedOption: 'No' }],
    })
    expect((blocks()[2] as { content: string }).content).toBe('You said no, so I did not run it.')
  })

  test('a question still waiting when the turn ends was not answered', () => {
    const { sink, blocks } = turn()
    sink.handle({ type: 'notice', message: QUESTION })
    sink.finish()
    expect(blocks()).toEqual([
      expect.objectContaining({ type: 'ask-user', toolCallId: 'question-1', skipped: true }),
    ])
  })
})
