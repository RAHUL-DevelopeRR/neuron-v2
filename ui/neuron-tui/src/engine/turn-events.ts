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
} from '../utils/sdk-event-handlers'
import { NO_RESULT_MESSAGE, mapToolCall, mapToolResult } from './tool-mapping'

import type { UiEvent } from './protocol'

export interface TurnEventSink {
  handle(event: UiEvent): void
  /**
   * Close the calls that never got a result: the engine sends none for a call
   * that was denied or blocked by policy. Call once, when the turn ends.
   */
  finish(): void
}

type OpenCall = { id: string; name: string; toolName: string }

export function createTurnEventSink(state: EventHandlerState): TurnEventSink {
  const chunk = createStreamChunkHandler(state)
  const sdk = createEventHandler(state)
  // A result names its tool but not its call, so results are matched to the
  // oldest open call of the same tool, which is the engine's order.
  const open: OpenCall[] = []

  const close = (call: OpenCall, output: string, isError: boolean) =>
    sdk({
      type: 'tool_result',
      toolCallId: call.id,
      toolName: call.toolName,
      output: mapToolResult(call.name, output, isError),
    })

  return {
    handle(event) {
      switch (event.type) {
        case 'text_delta':
          chunk(event.text)
          return
        case 'reasoning_delta':
          chunk({
            type: 'reasoning_chunk',
            agentId: '',
            ancestorRunIds: [],
            chunk: event.text,
          })
          return
        case 'reasoning_started':
          if (event.redacted) {
            chunk({
              type: 'reasoning_chunk',
              agentId: '',
              ancestorRunIds: [],
              chunk: '(The provider keeps this reasoning hidden.)',
            })
          }
          return
        case 'tool_call_started': {
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
          const index = open.findIndex((call) => call.name === event.name)
          if (index < 0) {
            state.logger.warn(
              { tool: event.name },
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
