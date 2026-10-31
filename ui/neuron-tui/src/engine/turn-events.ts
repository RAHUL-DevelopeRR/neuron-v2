/**
 * One turn's engine events, fed through the chat's existing stream and tool
 * handlers (src/utils/sdk-event-handlers.ts), so Freebuff's message blocks and
 * tool renderers draw a Neuron turn the way they drew a Codebuff run.
 *
 * Events outside the transcript (status, usage, permission requests, notices)
 * are handled once, globally, by ./engine-session.ts. Two notices also change
 * the turn's message: an AskUserQuestion is drawn in it (./questions.ts), and
 * a retry of the model call drops what the failed attempt streamed.
 *
 * Every event also goes to the live view of the turn (./live-turn.ts): the
 * status line, the todo strip and the footer's running totals. With
 * `live_progress`, a tool call's row is drawn when the model names the tool
 * and grows with its input (./live-tool-blocks.ts); its `tool_call_started`
 * then completes that row instead of adding one.
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
  NO_INPUT_YET,
  subagentFromResult,
  withCompleteToolInput,
  withSubagent,
  withToolInputSoFar,
  withoutToolBlocks,
} from './live-tool-blocks'
import { startLiveTurn } from './live-turn'
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

/**
 * The result shown for a call the model never finished writing: the turn
 * ended (the stream failed, the request was retried out) before its input was
 * complete.
 */
export const INCOMPLETE_CALL_MESSAGE =
  'The response ended before this call was complete, so it did not run.'

type OpenCall = {
  id: string
  name: string
  toolName: string
  /** Drawn from `tool_input_started`: its complete input has not come yet. */
  inputStreaming?: boolean
}

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
  const live = startLiveTurn()
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

  const drawing = () => state.isActive?.() !== false

  /**
   * A tool call is content too: the turn is no longer waiting for its first
   * token, and a retry that was shown is over.
   */
  const toolActivity = () => {
    if (!state.message.hasReceivedContentRef.current) {
      state.message.hasReceivedContentRef.current = true
      state.streaming.setStreamStatus('streaming')
    }
    state.setIsRetrying(false)
  }

  /**
   * The call's row as soon as the model names the tool, with an empty input
   * until the complete one arrives. One row per id: later events update it.
   */
  const drawStreamingCall = (id: string, name: string) => {
    if (open.some((call) => call.id === id)) return
    separateTextBlocks()
    toolActivity()
    const mapped = mapToolCall(name, '')
    sdk({ type: 'tool_call', toolCallId: id, toolName: mapped.toolName, input: mapped.input })
    updater.updateAiMessageBlocks((blocks) => withToolInputSoFar(blocks, id, NO_INPUT_YET))
    open.push({ id, name, toolName: mapped.toolName, inputStreaming: true })
  }

  /**
   * The engine sends the model call again. Calls whose input the failed
   * attempt was still writing never ran and get no result: their rows go
   * with the attempt's text.
   */
  const dropStreamingCalls = () => {
    const dropped = new Set(
      open.filter((call) => call.inputStreaming).map((call) => call.id),
    )
    if (dropped.size === 0) return
    for (let index = open.length - 1; index >= 0; index--) {
      if (dropped.has(open[index]!.id)) open.splice(index, 1)
    }
    updater.updateAiMessageBlocks((blocks) => withoutToolBlocks(blocks, dropped))
    state.streaming.setStreamingAgents((prev) => {
      const next = new Set(prev)
      for (const id of dropped) next.delete(id)
      return next
    })
  }

  const close = (call: OpenCall, output: string, isError: boolean) => {
    const showSubagentEnd = () => {
      if (call.name !== 'Agent') return
      // The sub-agent as its result reports it (older engines send no
      // sub-agent events), or the final text those events left out.
      updater.updateAiMessageBlocks((blocks) =>
        withSubagent(blocks, call.id, (previous) =>
          subagentFromResult(output, isError, previous),
        ),
      )
    }
    if (state.isActive?.() === false) {
      // The turn was interrupted: only its row is updated, nothing that
      // belongs to the run (streaming state, agents) any more.
      if (options.acceptLateToolResults?.()) {
        showSubagentEnd()
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
    showSubagentEnd()
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
      live.track(event)
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
        case 'tool_input_started':
          if (!drawing()) return
          drawStreamingCall(event.id, event.name)
          return
        case 'tool_input_progress': {
          if (!drawing()) return
          // Its row, also when the start was missed.
          drawStreamingCall(event.id, event.name)
          const { bytes, lines, target, preview } = event
          updater.updateAiMessageBlocks((blocks) =>
            withToolInputSoFar(blocks, event.id, { bytes, lines, target, preview }),
          )
          return
        }
        case 'tool_call_started': {
          separateTextBlocks()
          const mapped = mapToolCall(event.name, event.input)
          const streamed = open.find((call) => call.id === event.id && call.inputStreaming)
          let call: OpenCall
          if (streamed) {
            // Its row is drawn: the complete input replaces what streamed.
            streamed.inputStreaming = false
            streamed.toolName = mapped.toolName
            call = streamed
            if (drawing()) {
              toolActivity()
              updater.updateAiMessageBlocks((blocks) =>
                withCompleteToolInput(blocks, event.id, mapped.toolName, mapped.input),
              )
            }
          } else {
            if (drawing()) toolActivity()
            sdk({
              type: 'tool_call',
              toolCallId: event.id,
              toolName: mapped.toolName,
              input: mapped.input,
            })
            call = { id: event.id, name: event.name, toolName: mapped.toolName }
          }
          if (event.cut_off) {
            if (streamed) open.splice(open.indexOf(streamed), 1)
            close(call, 'The output limit cut this call off, so it did not run.', true)
          } else if (!streamed) {
            open.push(call)
          }
          markAttemptStart()
          return
        }
        case 'subagent_started': {
          if (!drawing()) return
          const { description, subagent_type: subagentType } = event
          updater.updateAiMessageBlocks((blocks) =>
            withSubagent(blocks, event.id, (previous) => ({
              ...previous,
              description,
              subagentType,
              status: 'running',
              finished: false,
              isError: false,
              result: null,
              note: null,
            })),
          )
          return
        }
        case 'subagent_finished': {
          if (!drawing() && !options.acceptLateToolResults?.()) return
          const { status, is_error: isError, result, note } = event
          updater.updateAiMessageBlocks((blocks) =>
            withSubagent(blocks, event.id, (previous) => ({
              description: previous?.description ?? '',
              subagentType: previous?.subagentType ?? null,
              status,
              finished: true,
              isError,
              result,
              note,
            })),
          )
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
          if (event.kind === 'refusal') {
            // The model declined: the turn's message ends with the refusal,
            // drawn apart from what it wrote before (RefusalBanner).
            separateTextBlocks()
            const refusal = event.message
            updater.updateAiMessage((message) => ({ ...message, refusal }))
            return
          }
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
            dropStreamingCalls()
            // "retrying..." until the next attempt's first token.
            state.setIsRetrying(true)
            live.retry()
          }
          return
        }
        case 'output_progress':
          // Tokens flow again: a retry shown is over.
          if (drawing()) state.setIsRetrying(false)
          return
        default:
          return
      }
    },
    finish() {
      live.end()
      // An interrupted turn's calls without a result were marked
      // "interrupted" when it stopped: they did not fail for want of one.
      const calls = open.splice(0)
      if (state.isActive?.() !== false) {
        for (const call of calls) {
          close(call, call.inputStreaming ? INCOMPLETE_CALL_MESSAGE : NO_RESULT_MESSAGE, true)
        }
      }
      // An interrupted turn leaves its question unanswered.
      if (questionCount > 0) updater.updateAiMessageBlocks(markUnansweredQuestions)
    },
  }
}
