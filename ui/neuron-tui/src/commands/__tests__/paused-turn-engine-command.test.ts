import { afterEach, beforeEach, describe, expect, mock, test } from 'bun:test'

import {
  setEngineClientOptionsForTesting,
  startEngineSession,
} from '../../engine/engine-session'
import { getNeuronEngineClient } from '../../engine/neuron-engine-client'
import { clearPausedTurn, hasPausedTurn, notePausedTurn } from '../../engine/paused-turn'
import { useChatStore } from '../../state/chat-store'
import { useEngineStore } from '../../state/engine-store'
import { routeUserPrompt } from '../router'

import type { SpawnEngine } from '../../engine/stdio-engine-client'
import type { RouterParams } from '../command-registry'

/**
 * An engine that says `ready` at once and finishes every slash command it gets
 * with a notice and `turn_finished`, as `/usage` does. Records what it was sent.
 */
function fakeEngine() {
  const encoder = new TextEncoder()
  const sent: Array<Record<string, unknown>> = []
  const spawn: SpawnEngine = () => {
    let out!: ReadableStreamDefaultController<Uint8Array>
    let err!: ReadableStreamDefaultController<Uint8Array>
    let exitWith!: (code: number) => void
    const exit = (code: number) => {
      try {
        out.close()
        err.close()
      } catch {
        // Already closed.
      }
      exitWith(code)
    }
    const emit = (event: object) => out.enqueue(encoder.encode(JSON.stringify(event) + '\n'))
    return {
      stdin: {
        write: (data: string) => {
          for (const line of data.split('\n').filter(Boolean)) {
            const message = JSON.parse(line) as Record<string, unknown>
            sent.push(message)
            if (message.type === 'slash') {
              queueMicrotask(() => {
                emit({ type: 'notice', message: 'Usage: 1,234 tokens this session' })
                emit({ type: 'turn_finished' })
              })
            }
          }
        },
        flush: () => undefined,
        end: () => exit(0),
      },
      stdout: new ReadableStream<Uint8Array>({
        start: (controller) => {
          out = controller
          queueMicrotask(() => emit({ type: 'ready', protocol: 1, version: 'test' }))
        },
      }),
      stderr: new ReadableStream<Uint8Array>({ start: (controller) => (err = controller) }),
      exited: new Promise<number>((resolve) => (exitWith = resolve)),
      signalCode: null,
      kill: () => exit(137),
    } as unknown as ReturnType<SpawnEngine>
  }
  return { spawn, sent }
}

function params(inputValue: string) {
  const sendMessage = mock((_message: { content: string }) => {})
  return {
    sendMessage,
    params: {
      agentMode: 'DEFAULT',
      inputRef: { current: null },
      inputValue,
      isChainInProgressRef: { current: false },
      isStreaming: false,
      streamMessageIdRef: { current: null },
      saveToHistory: () => {},
      setInputValue: mock(() => {}),
      setInputFocused: mock(() => {}),
      setMessages: mock(() => {}),
      sendMessage,
      addToQueue: mock(() => {}),
      scrollToLatest: () => {},
    } as unknown as RouterParams,
  }
}

const waitUntil = async (predicate: () => boolean) => {
  const deadline = Date.now() + 2_000
  while (!predicate() && Date.now() < deadline) await new Promise((r) => setTimeout(r, 5))
  expect(predicate()).toBe(true)
}

beforeEach(() => {
  clearPausedTurn()
  useChatStore.getState().reset()
})

afterEach(async () => {
  await getNeuronEngineClient()
    .shutdown()
    .catch(() => undefined)
  setEngineClientOptionsForTesting({})
  useEngineStore.getState().reset()
  clearPausedTurn()
})

describe('an engine command after a paused turn', () => {
  test('/usage keeps the pause: Enter on an empty prompt still continues', async () => {
    const engine = fakeEngine()
    setEngineClientOptionsForTesting({ spawn: engine.spawn, resolveBinary: () => 'neuron' })
    await startEngineSession({ cwd: process.cwd() })
    notePausedTurn()

    const usage = params('/usage')
    await routeUserPrompt(usage.params)
    await waitUntil(() => engine.sent.some((message) => message.type === 'slash'))
    expect(engine.sent).toContainEqual(
      expect.objectContaining({ type: 'slash', command: 'usage' }),
    )
    expect(usage.sendMessage).not.toHaveBeenCalled()
    // Let the command's notice and turn_finished arrive.
    await new Promise((r) => setTimeout(r, 20))
    expect(hasPausedTurn()).toBe(true)

    const empty = params('')
    await routeUserPrompt(empty.params)
    expect(empty.sendMessage.mock.calls.map(([message]) => message.content)).toEqual([
      'continue',
    ])
    expect(hasPausedTurn()).toBe(false)
  })
})
