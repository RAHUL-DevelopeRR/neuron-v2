/**
 * The agent's questions in Freebuff's question form (components/ask-user,
 * through its AskUserBridge), and answering them.
 *
 * A `question` event (the `questions` capability) with options opens the form
 * in the input's place: ↑/↓ pick an option, Enter sends it, or the custom
 * field takes an answer in the user's own words. Skipping the form leaves the
 * question waiting, and the next text typed in the input answers it. A
 * question without options has nothing to pick: it is answered in the input.
 * Either way the answer goes back as an `answer` command with the event's id
 * (a plain submit for an older engine's "Question:" notice), and is written
 * into the question's block in the turn's message (./questions.ts).
 */

import { AskUserBridge } from '@neuron-tui/common/utils/ask-user-bridge'

import { getNeuronEngineClient, type NeuronEngineClient } from './neuron-engine-client'
import { withQuestionAnswer } from './questions'
import { useChatStore } from '../state/chat-store'
import { useEngineStore } from '../state/engine-store'
import { getUserMessage } from '../utils/message-history'

import type { QuestionEvent } from './protocol'

/** The form's tool-call id for the engine question `id`. */
export function questionFormId(id: number): string {
  return `engine-question-${id}`
}

const isQuestionFormId = (toolCallId: string) => toolCallId.startsWith('engine-question-')

/** What the form sends back (useAskUserBridge's `submitAnswers` or `skip`). */
interface FormResponse {
  skipped?: boolean
  answers?: Array<{
    questionIndex: number
    selectedOption?: string
    selectedOptions?: string[]
    otherText?: string
  }>
}

/**
 * The form's answer to the first question, as the text the engine takes: the
 * option picked, the options picked (joined with ", "), or the user's own
 * words. `null` when the form was skipped or left the question unanswered.
 */
export function formAnswerText(response: unknown): string | null {
  const form = (response ?? {}) as FormResponse
  if (form.skipped) return null
  const answer = form.answers?.find((entry) => entry.questionIndex === 0)
  if (!answer) return null
  const text =
    answer.otherText?.trim() ||
    answer.selectedOption?.trim() ||
    (answer.selectedOptions ?? []).map((option) => option.trim()).filter(Boolean).join(', ')
  return text ? text : null
}

/**
 * Answers the question the engine is waiting for with `text`: writes it into
 * the question's block (a chat line when the block is not drawn), forgets the
 * question, and sends it: `answer` with the event's id, or a plain submit for
 * a notice. `false` when no question waits. A failed write is thrown.
 */
export function answerEngineQuestion(
  text: string,
  engine: Pick<NeuronEngineClient, 'send' | 'submit'> = getNeuronEngineClient(),
): boolean {
  const { pendingQuestion, pendingQuestionId, pendingQuestionTarget } = useEngineStore.getState()
  if (pendingQuestion === null) return false
  useEngineStore.getState().setPendingQuestion(null)
  closeQuestionForm()
  // Into the question's block in the turn's message, so it reads above the
  // agent's reply to it; a chat line when the question is one too.
  useChatStore
    .getState()
    .setMessages(
      (prev) =>
        (pendingQuestionTarget && withQuestionAnswer(prev, pendingQuestionTarget, text)) || [
          ...prev,
          { ...getUserMessage(text), metadata: { answersQuestion: true, sentToEngine: false } },
        ],
    )
  if (pendingQuestionId !== null) {
    engine.send({ type: 'answer', id: pendingQuestionId, text })
  } else {
    engine.submit(text)
  }
  return true
}

/**
 * Opens the question form for a `question` event with options. Its answer is
 * sent only while that question still waits (not after the turn ended, or
 * once the input answered it).
 */
export function openQuestionForm(
  event: Pick<QuestionEvent, 'id' | 'question' | 'options'>,
  onError: (error: unknown) => void = () => {},
): boolean {
  const options = event.options.filter((option) => option.trim().length > 0)
  if (options.length === 0) return false
  // A form still open for an earlier question gives way.
  closeQuestionForm()
  void AskUserBridge.request(questionFormId(event.id), [
    {
      question: event.question.trim(),
      options: options.map((label) => ({ label })),
      multiSelect: false,
    },
  ]).then((response) => {
    const text = formAnswerText(response)
    if (text === null) return
    if (useEngineStore.getState().pendingQuestionId !== event.id) return
    try {
      answerEngineQuestion(text)
    } catch (error) {
      onError(error)
    }
  })
  return true
}

/** Closes the form of an engine question, unanswered (the turn ended, or it was answered). */
export function closeQuestionForm(): void {
  const pending = AskUserBridge.getPendingRequest()
  if (pending && isQuestionFormId(pending.toolCallId)) {
    AskUserBridge.submit({ skipped: true })
  }
}
