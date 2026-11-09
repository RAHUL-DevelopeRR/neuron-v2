import { AskUserBridge } from '@neuron-tui/common/utils/ask-user-bridge'
import { afterEach, beforeEach, describe, expect, test } from 'bun:test'

import { useChatStore } from '../../state/chat-store'
import { useEngineStore } from '../../state/engine-store'
import { handleEngineEvent } from '../engine-session'
import {
  setNeuronEngineClientForTesting,
  type NeuronEngineClient,
} from '../neuron-engine-client'
import {
  answerEngineQuestion,
  closeQuestionForm,
  formAnswerText,
  questionFormId,
} from '../question-form'
import {
  engineQuestionBlock,
  insertQuestionBlock,
  markUnansweredQuestions,
  parseEngineQuestion,
  questionAnswer,
  questionEventBlock,
  questionNoticeText,
  withQuestionAnswer,
} from '../questions'

import type { EngineCommand } from '../protocol'
import type { ChatMessage, ContentBlock } from '../../types/chat'

describe('parseEngineQuestion', () => {
  test("reads the engine's question notice: the question, its options and the hint", () => {
    expect(
      parseEngineQuestion(
        'Question: Which branch?\n  1. main\n  2. dev\n  Answer with 1-2 or in your own words.',
      ),
    ).toEqual({
      question: 'Which branch?',
      options: ['main', 'dev'],
      hint: 'Answer with 1-2 or in your own words.',
    })
  })

  test('a question without options, or over several lines', () => {
    expect(parseEngineQuestion('Question: What should the release be called?')).toEqual({
      question: 'What should the release be called?',
      options: [],
      hint: null,
    })
    expect(
      parseEngineQuestion('Question: Two things:\nthe name, and\nthe date?\n  1. later\n'),
    ).toEqual({ question: 'Two things:\nthe name, and\nthe date?', options: ['later'], hint: null })
  })

  test('numbered lines that are not 1, 2, 3... stay in the question', () => {
    expect(parseEngineQuestion('Question: Pick one\n  2. b\n  3. c').options).toEqual([])
  })
})

describe('answers', () => {
  const block = engineQuestionBlock('Question: Which branch?\n  1. main\n  2. dev', 'toolu_q')

  test('a number or an option picks it; other words are the answer as typed', () => {
    expect(questionAnswer(block, '2')).toEqual({ questionIndex: 0, selectedOption: 'dev' })
    expect(questionAnswer(block, ' Main ')).toEqual({ questionIndex: 0, selectedOption: 'main' })
    expect(questionAnswer(block, '7')).toEqual({ questionIndex: 0, otherText: '7' })
    expect(questionAnswer(block, 'neither, use release')).toEqual({
      questionIndex: 0,
      otherText: 'neither, use release',
    })
  })

  test("go into the question's block; null when it is not there", () => {
    const messages: ChatMessage[] = [
      { id: 'ai-1', variant: 'ai', content: '', blocks: [block], timestamp: 'now' },
    ]
    const answered = withQuestionAnswer(messages, { messageId: 'ai-1', toolCallId: 'toolu_q' }, '1')
    expect(answered?.[0]?.blocks?.[0]).toMatchObject({
      answers: [{ questionIndex: 0, selectedOption: 'main' }],
    })
    expect(withQuestionAnswer(messages, { messageId: 'ai-2', toolCallId: 'toolu_q' }, '1')).toBeNull()
    expect(withQuestionAnswer(messages, { messageId: 'ai-1', toolCallId: 'other' }, '1')).toBeNull()
  })

  test('the block replaces its tool row, or goes last', () => {
    const tool: ContentBlock = {
      type: 'tool',
      toolCallId: 'toolu_q',
      toolName: 'AskUserQuestion' as never,
      input: {},
    }
    const text: ContentBlock = { type: 'text', content: 'Asking.' }
    expect(insertQuestionBlock([text, tool], block)).toEqual([text, block])
    expect(insertQuestionBlock([text], block)).toEqual([text, block])
  })

  test('unanswered questions are marked when the turn ends', () => {
    const answered = { ...block, toolCallId: 'a', answers: [{ questionIndex: 0, otherText: 'x' }] }
    expect(markUnansweredQuestions([answered, block])).toEqual([
      answered,
      { ...block, skipped: true },
    ])
    expect(markUnansweredQuestions([answered])).toEqual([answered])
  })
})

describe('question events (the questions capability)', () => {
  const event = {
    type: 'question' as const,
    id: 7,
    question: 'Which branch?',
    options: ['main', 'dev'],
  }

  /** A client that records what is sent. */
  function recordingClient() {
    const sent: EngineCommand[] = []
    const submitted: string[] = []
    const client = {
      send: (command: EngineCommand) => void sent.push(command),
      submit: (text: string) => void submitted.push(text),
    }
    return { sent, submitted, client }
  }

  const flush = () => new Promise((resolve) => setTimeout(resolve, 0))

  beforeEach(() => {
    useEngineStore.getState().reset()
    useChatStore.getState().setMessages([])
    closeQuestionForm()
  })

  afterEach(() => {
    closeQuestionForm()
    setNeuronEngineClientForTesting(null)
    useEngineStore.getState().reset()
  })

  test('the block and the chat line read as the older notice did', () => {
    const notice = 'Question: Which branch?\n  1. main\n  2. dev\nAnswer with 1-2 or in your own words.'
    expect(questionEventBlock(event, 'toolu_q')).toEqual(engineQuestionBlock(notice, 'toolu_q'))
    expect(questionNoticeText(event)).toBe(notice)
    expect(questionEventBlock({ question: 'Name?', options: [] }, 'q').hint).toBe('')
    expect(questionNoticeText({ question: 'Name?', options: [] })).toBe('Question: Name?')
  })

  test("the form's response becomes the answer's text; a skip is none", () => {
    expect(formAnswerText({ answers: [{ questionIndex: 0, selectedOption: 'dev' }] })).toBe('dev')
    expect(formAnswerText({ answers: [{ questionIndex: 0, otherText: ' release ' }] })).toBe('release')
    expect(
      formAnswerText({ answers: [{ questionIndex: 0, selectedOptions: ['main', 'dev'] }] }),
    ).toBe('main, dev')
    expect(formAnswerText({ answers: [{ questionIndex: 0 }] })).toBeNull()
    expect(formAnswerText({ skipped: true })).toBeNull()
    expect(formAnswerText(undefined)).toBeNull()
  })

  test('an answer to a question event is the answer command with its id; to a notice, a submit', () => {
    const { sent, submitted, client } = recordingClient()
    useEngineStore.getState().setPendingQuestion('Which branch?', 7)
    expect(answerEngineQuestion('main', client)).toBe(true)
    expect(sent).toEqual([{ type: 'answer', id: 7, text: 'main' }])
    expect(submitted).toEqual([])
    expect(useEngineStore.getState().pendingQuestion).toBeNull()
    expect(useChatStore.getState().messages.at(-1)).toMatchObject({
      content: 'main',
      metadata: { answersQuestion: true, sentToEngine: false },
    })

    useEngineStore.getState().setPendingQuestion('Question: Ship it?')
    expect(answerEngineQuestion('yes', client)).toBe(true)
    expect(submitted).toEqual(['yes'])
    expect(answerEngineQuestion('again', client)).toBe(false)
  })

  test('the session takes the event: it waits for the answer and opens the form', async () => {
    const { sent, client } = recordingClient()
    setNeuronEngineClientForTesting(client as unknown as NeuronEngineClient)
    handleEngineEvent(event)
    expect(useEngineStore.getState()).toMatchObject({
      pendingQuestion: 'Which branch?',
      pendingQuestionId: 7,
    })
    const request = AskUserBridge.getPendingRequest()
    expect(request?.toolCallId).toBe(questionFormId(7))
    expect(request?.questions[0]).toMatchObject({
      question: 'Which branch?',
      options: [{ label: 'main' }, { label: 'dev' }],
      multiSelect: false,
    })
    // Outside a turn the question is a chat line too.
    expect(useChatStore.getState().messages.at(-1)?.content).toContain('Question: Which branch?')

    AskUserBridge.submit({ answers: [{ questionIndex: 0, selectedOption: 'dev' }] })
    await flush()
    expect(sent).toEqual([{ type: 'answer', id: 7, text: 'dev' }])
    expect(useEngineStore.getState().pendingQuestion).toBeNull()
  })

  test('a skipped form leaves the question to the input', async () => {
    const { sent, client } = recordingClient()
    setNeuronEngineClientForTesting(client as unknown as NeuronEngineClient)
    handleEngineEvent(event)
    AskUserBridge.submit({ skipped: true })
    await flush()
    expect(sent).toEqual([])
    expect(useEngineStore.getState().pendingQuestionId).toBe(7)
  })

  test('the turn ending closes the form, and nothing is sent for it', async () => {
    const { sent, client } = recordingClient()
    setNeuronEngineClientForTesting(client as unknown as NeuronEngineClient)
    handleEngineEvent(event)
    handleEngineEvent({ type: 'turn_finished' })
    expect(AskUserBridge.getPendingRequest()).toBeNull()
    await flush()
    expect(sent).toEqual([])
    expect(useEngineStore.getState().pendingQuestion).toBeNull()
  })

  test('a question without options has no form: it is answered in the input', () => {
    const { client } = recordingClient()
    setNeuronEngineClientForTesting(client as unknown as NeuronEngineClient)
    handleEngineEvent({ ...event, options: [] })
    expect(AskUserBridge.getPendingRequest()).toBeNull()
    expect(useEngineStore.getState().pendingQuestionId).toBe(7)
  })
})
