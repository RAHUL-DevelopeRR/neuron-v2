/**
 * The engine's AskUserQuestion, drawn where the agent asked it.
 *
 * The engine asks with a notice ("Question: Which branch?\n  1. main\n  2. dev
 * \n  Answer with 1-2 or in your own words.") and takes the next submit as
 * the answer. During a turn the question goes into the turn's own message, in
 * place of the AskUserQuestion tool row (an `ask-user` block), and the answer
 * is written into that block; the agent's reply to it follows below. Drawn
 * as chat lines after the turn's message instead, both ended up under the
 * reply.
 */

import type { AskUserContentBlock, ChatMessage, ContentBlock } from '../types/chat'

/** AskUserQuestion reaches the UI as a notice with this prefix. */
export const QUESTION_PREFIX = 'Question:'

/** Where a question waiting for its answer is drawn. */
export interface QuestionTarget {
  messageId: string
  toolCallId: string
}

export interface EngineQuestion {
  question: string
  options: string[]
  /** The engine's "Answer with 1-2 or in your own words." */
  hint: string | null
}

const OPTION_LINE = /^\s*(\d+)\.\s+(.*)$/
const HINT_LINE = /^\s*Answer\b/

/** The question, its numbered options and the answer hint of a question notice. */
export function parseEngineQuestion(message: string): EngineQuestion {
  const body = message.startsWith(QUESTION_PREFIX)
    ? message.slice(QUESTION_PREFIX.length)
    : message
  const lines = body.replace(/\r/g, '').split('\n')
  while (lines.length > 0 && !lines[lines.length - 1]!.trim()) lines.pop()

  let hint: string | null = null
  if (lines.length > 1 && HINT_LINE.test(lines[lines.length - 1]!)) {
    hint = lines.pop()!.trim()
  }
  // The options are the numbered lines at the end, 1, 2, 3...
  let firstOption = lines.length
  while (firstOption > 1 && OPTION_LINE.test(lines[firstOption - 1]!)) firstOption -= 1
  const optionLines = lines.slice(firstOption)
  const numbered = optionLines.every(
    (line, index) => Number(OPTION_LINE.exec(line)![1]) === index + 1,
  )
  const options = numbered
    ? optionLines.map((line) => OPTION_LINE.exec(line)![2]!.trim())
    : []
  const questionLines = numbered ? lines.slice(0, firstOption) : lines
  return {
    question: questionLines.join('\n').trim(),
    options,
    hint,
  }
}

/** The `ask-user` block for a question notice, not answered yet. */
export function engineQuestionBlock(
  message: string,
  toolCallId: string,
): AskUserContentBlock {
  const parsed = parseEngineQuestion(message)
  return {
    type: 'ask-user',
    toolCallId,
    questions: [
      {
        question: parsed.question,
        options: parsed.options.map((label) => ({ label })),
      },
    ],
    // Always set: it marks an engine question (answered in the input).
    hint: parsed.hint ?? '',
  }
}

/** The answer as the block shows it: "2" picks the second option. */
export function questionAnswer(
  block: AskUserContentBlock,
  answer: string,
): NonNullable<AskUserContentBlock['answers']>[number] {
  const text = answer.trim()
  const options = block.questions[0]?.options ?? []
  if (/^\d+$/.test(text)) {
    const option = options[Number(text) - 1]
    if (option) return { questionIndex: 0, selectedOption: option.label }
  }
  const named = options.find(
    (option) => option.label.toLowerCase() === text.toLowerCase(),
  )
  if (named) return { questionIndex: 0, selectedOption: named.label }
  return { questionIndex: 0, otherText: text }
}

function mapQuestionBlock(
  blocks: ContentBlock[],
  toolCallId: string,
  update: (block: AskUserContentBlock) => AskUserContentBlock,
): ContentBlock[] {
  let changed = false
  const next = blocks.map((block) => {
    if (block.type === 'ask-user' && block.toolCallId === toolCallId) {
      changed = true
      return update(block)
    }
    if (block.type === 'agent' && block.blocks) {
      const inner = mapQuestionBlock(block.blocks, toolCallId, update)
      if (inner !== block.blocks) {
        changed = true
        return { ...block, blocks: inner }
      }
    }
    return block
  })
  return changed ? next : blocks
}

/**
 * Writes the answer into the question's block. `null` when the block is not
 * in `messages` (the chat was switched), so the caller can show it otherwise.
 */
export function withQuestionAnswer(
  messages: ChatMessage[],
  target: QuestionTarget,
  answer: string,
): ChatMessage[] | null {
  let found = false
  const next = messages.map((message) => {
    if (message.id !== target.messageId || !message.blocks) return message
    const blocks = mapQuestionBlock(message.blocks, target.toolCallId, (block) => ({
      ...block,
      answers: [questionAnswer(block, answer)],
    }))
    if (blocks === message.blocks) return message
    found = true
    return { ...message, blocks }
  })
  return found ? next : null
}

/** Questions still waiting when the turn ended were never answered. */
export function markUnansweredQuestions(blocks: ContentBlock[]): ContentBlock[] {
  let changed = false
  const next = blocks.map((block) => {
    if (block.type === 'ask-user' && !block.answers && !block.skipped) {
      changed = true
      return { ...block, skipped: true }
    }
    return block
  })
  return changed ? next : blocks
}

/**
 * Puts the question in place of its tool's row (`toolCallId`), or after the
 * last block when that row is not there.
 */
export function insertQuestionBlock(
  blocks: ContentBlock[],
  question: AskUserContentBlock,
): ContentBlock[] {
  const index = blocks.findIndex(
    (block) => block.type === 'tool' && block.toolCallId === question.toolCallId,
  )
  if (index < 0) return [...blocks, question]
  return [...blocks.slice(0, index), question, ...blocks.slice(index + 1)]
}
