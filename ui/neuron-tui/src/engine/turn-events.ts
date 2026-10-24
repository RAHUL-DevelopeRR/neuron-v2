/**
 * One turn's engine events, fed through the chat's existing stream and tool
 * handlers (src/utils/sdk-event-handlers.ts), so Freebuff's message blocks and
 * tool renderers draw a Neuron turn the way they drew a Codebuff run.
 *
 * Events outside the transcript (status, usage, permission requests, notices)
 * are handled once, globally, by ./engine-session.ts. Two notices also change
 * the turn's message: an AskUserQuestion is drawn in it (./questions.ts), and
 * a retry of the model call drops what the failed attempt streamed.
 */

import { useEngineStore } from '../state/engine-store'
import { closeNativeReasoningBlock } from '../utils/block-operations'
import { updateToolBlockWithOutput } from '../utils/message-block-helpers'
import {
  createEventHandler,
  createStreamChunkHandler,
  type EventHandlerState,
  type ToolResultWithStatus,
} from '../utils/sdk-event-handlers'
import { isRetryNotice } from './engine-session'
import {
  QUESTION_PREFIX,
  engineQuestionBlock,
  insertQuestionBlock,
  markUnansweredQuestions,
} from './questions'
import { NO_RESULT_MESSAGE, mapToolCall, mapToolResult } from './tool-mapping'

import type { ContentBlock } from '../types/chat'
import type { UiEvent } from './protocol'

/**
 * The blocks without the text and reasoning a failed attempt of the model
 * call streamed: those after `attemptStart`, where the attempt began (the
 * turn's start, or its last tool call, result or question). The engine sends
 * the request again, and the next attempt streams them anew.
 */
export function dropFailedAttempt(
  blocks: ContentBlock[],
  attemptStart: number,
): ContentBlock[] {
  const kept = blocks.filter(
    (block, index) => index < attemptStart || block.type !== 'text',
  )
  return kept.length === blocks.length ? blocks : kept
}

export interface TurnEventSink {
  handle(event: UiEvent): void
  /**
   * Close the calls that never got a result (older engines sent none for a
   * denied or blocked call). Call once, when the turn ends.
   */
  finish(): void
}

type OpenCall = { id: string; name: string; toolName: string }

export interface TurnEventSinkOptions {
  /**
   * Whether a tool result that arrives after the turn stopped being drawn
   * (it was interrupted) still goes to its call's row: the engine reports how
   * the running command ended ("interrupted", with what it printed).
   */
  acceptLateToolResults?: () => boolean
}

export function createTurnEventSink(
  state: EventHandlerState,
  options: TurnEventSinkOptions = {},
): TurnEventSink {
  const chunk = createStreamChunkHandler(state)
  const sdk = createEventHandler(state)
  // Results carry their call's id. Older engines named only the tool, so
  // without an id a result goes to the oldest open call of that tool, which is
  // the engine's order.
  const open: OpenCall[] = []
  // The engine ends every text block with `text_block_end`. Two text blocks
  // with nothing drawn between them (text, a thinking block with no visible
  // text, text) land in the same chat block, so the next block starts a new
  // paragraph instead of running on: "foo.ts" + "**Done**" read as
  // "foo.ts**Done**", one line. A tool or reasoning block in between already
  // separates them.
  let textBlockEnded = false
  /** The last characters of the chat's current text block ('' when none). */
  let textTail = ''
  const separateTextBlocks = () => {
    textBlockEnded = false
    textTail = ''
  }

  const updater = state.message.updater
  /**
   * How many blocks the message had when the current attempt of the model
   * call began. Read in the updater's order, so it counts the blocks of the
   * events before it even while updates are batched.
   */
  let attemptStart = 0
  const markAttemptStart = () =>
    updater.updateAiMessage((message) => {
      attemptStart = message.blocks?.length ?? 0
      return message
    })

  let questionCount = 0
  const showQuestion = (message: string) => {
    separateTextBlocks()
    // In place of the AskUserQuestion row: the question comes from the call
    // that is running, the last one still open.
    const asking = open[open.length - 1]
    questionCount += 1
    const toolCallId = asking?.id ?? `question-${questionCount}`
    const block = engineQuestionBlock(message, toolCallId)
    updater.updateAiMessageBlocks((blocks) =>
      insertQuestionBlock(closeNativeReasoningBlock(blocks), block),
    )
    markAttemptStart()
    useEngineStore
      .getState()
      .setPendingQuestionTarget({ messageId: state.message.aiMessageId, toolCallId })
  }

  const close = (call: OpenCall, output: string, isError: boolean) => {
    if (state.isActive?.() === false) {
      // The turn was interrupted: only its row is updated, nothing that
      // belongs to the run (streaming state, agents) any more.
      if (options.acceptLateToolResults?.()) {
        updater.updateAiMessageBlocks((blocks) =>
          updateToolBlockWithOutput(blocks, {
            toolCallId: call.id,
            toolOutput: mapToolResult(call.name, output, isError),
            isError,
          }),
        )
      }
      return
    }
    // `isError` reaches the tool block, so a refused write or a failed
    // command does not read like a success.
    const result: ToolResultWithStatus = {
      type: 'tool_result',
      toolCallId: call.id,
      toolName: call.toolName,
      output: mapToolResult(call.name, output, isError),
      isError,
    }
    sdk(result)
  }

  return {
    handle(event) {
      switch (event.type) {
        case 'text_delta': {
          if (!event.text) return
          let text = event.text
          if (textBlockEnded && textTail) {
            const separator = textTail.endsWith('\n\n')
              ? ''
              : textTail.endsWith('\n')
                ? '\n'
                : '\n\n'
            text = separator + text
          }
          textBlockEnded = false
          textTail = (textTail + text).slice(-2)
          chunk(text)
          return
        }
        case 'text_block_end':
          textBlockEnded = true
          return
        case 'reasoning_delta':
          // Empty text draws no reasoning block, so it separates nothing.
          if (event.text) separateTextBlocks()
          chunk({
            type: 'reasoning_chunk',
            agentId: '',
            ancestorRunIds: [],
            chunk: event.text,
          })
          return
        case 'reasoning_started':
          if (event.redacted) {
            separateTextBlocks()
            chunk({
              type: 'reasoning_chunk',
              agentId: '',
              ancestorRunIds: [],
              chunk: '(The provider keeps this reasoning hidden.)',
            })
          }
          return
        case 'tool_call_started': {
          separateTextBlocks()
          const mapped = mapToolCall(event.name, event.input)
          sdk({
            type: 'tool_call',
            toolCallId: event.id,
            toolName: mapped.toolName,
            input: mapped.input,
          })
          const call = { id: event.id, name: event.name, toolName: mapped.toolName }
          if (event.cut_off) {
            close(call, 'The output limit cut this call off, so it did not run.', true)
          } else {
            open.push(call)
          }
          markAttemptStart()
          return
        }
        case 'tool_result': {
          const index = event.id
            ? open.findIndex((call) => call.id === event.id)
            : open.findIndex((call) => call.name === event.name)
          if (index < 0) {
            // A cut-off call was closed when it started; its result is no news.
            state.logger.debug(
              { tool: event.name, id: event.id },
              '[engine] tool_result without an open call',
            )
            return
          }
          const [call] = open.splice(index, 1)
          close(call!, event.output, event.is_error)
          markAttemptStart()
          return
        }
        case 'notice': {
          if (state.isActive?.() === false) return
          if (event.message.startsWith(QUESTION_PREFIX)) {
            showQuestion(event.message)
            return
          }
          if (isRetryNotice(event.message)) {
            // The engine sends the model call again; what this attempt
            // streamed would run on into the next one ("...no extra
            // text.The user asks: ..."). The notice itself goes above the
            // turn's message (engine-session.ts).
            separateTextBlocks()
            updater.updateAiMessageBlocks((blocks) =>
              dropFailedAttempt(blocks, attemptStart),
            )
          }
          return
        }
        default:
          return
      }
    },
    finish() {
      // An interrupted turn's calls without a result were marked
      // "interrupted" when it stopped: they did not fail for want of one.
      const calls = open.splice(0)
      if (state.isActive?.() !== false) {
        for (const call of calls) close(call, NO_RESULT_MESSAGE, true)
      }
      // An interrupted turn leaves its question unanswered.
      if (questionCount > 0) updater.updateAiMessageBlocks(markUnansweredQuestions)
    },
  }
}
