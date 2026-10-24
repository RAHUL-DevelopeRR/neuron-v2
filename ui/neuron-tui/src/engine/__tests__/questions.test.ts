import { describe, expect, test } from 'bun:test'

import {
  engineQuestionBlock,
  insertQuestionBlock,
  markUnansweredQuestions,
  parseEngineQuestion,
  questionAnswer,
  withQuestionAnswer,
} from '../questions'

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
