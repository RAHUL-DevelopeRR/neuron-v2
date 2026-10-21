/**
 * One turn's engine events, fed through the chat's existing stream and tool
 * handlers (src/utils/sdk-event-handlers.ts), so Freebuff's message blocks and
 * tool renderers draw a Neuron turn the way they drew a Codebuff run.
 *
 * Events outside the transcript (status, usage, permission requests, notices)
 * are handled once, globally, by ./engine-session.ts.
 */

import {
  createEventHandler,
  createStreamChunkHandler,
  type EventHandlerState,
  type ToolResultWithStatus,
} from '../utils/sdk-event-handlers'
import { NO_RESULT_MESSAGE, mapToolCall, mapToolResult } from './tool-mapping'

import type { UiEvent } from './protocol'

export interface TurnEventSink {
  handle(event: UiEvent): void
  /**
   * Close the calls that never got a result (older engines sent none for a
   * denied or blocked call). Call once, when the turn ends.
   */
  finish(): void
}

type OpenCall = { id: string; name: string; toolName: string }

export function createTurnEventSink(state: EventHandlerState): TurnEventSink {
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

  const close = (call: OpenCall, output: string, isError: boolean) => {
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
          return
        }
        default:
          return
      }
    },
    finish() {
      for (const call of open.splice(0)) close(call, NO_RESULT_MESSAGE, true)
    },
  }
}
