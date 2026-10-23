import { afterEach, describe, expect, test } from 'bun:test'

import { getNeuronEngineClient } from '../neuron-engine-client'
import {
  clearEngineConversation,
  resumeEngineSessionForChat,
  setEngineClientOptionsForTesting,
  startEngineSession,
} from '../engine-session'
import { useEngineStore } from '../../state/engine-store'

import type { SpawnEngine } from '../stdio-engine-client'

/**
 * Engines this test starts: their command lines and what they were sent. Each
 * says `ready` at once and finishes every slash command it gets.
 */
function fakeEngines() {
  const encoder = new TextEncoder()
  const started: { argv: string[]; sent: Array<Record<string, unknown>> }[] = []
  const spawn: SpawnEngine = (argv) => {
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
    const record = { argv, sent: [] as Array<Record<string, unknown>> }
    started.push(record)
    const emit = (event: object) => out.enqueue(encoder.encode(JSON.stringify(event) + '\n'))
    const stdout = new ReadableStream<Uint8Array>({
      start: (controller) => {
        out = controller
        queueMicrotask(() => emit({ type: 'ready', protocol: 1, version: 'test' }))
      },
    })
    return {
      stdin: {
        write: (data: string) => {
          for (const line of data.split('\n').filter(Boolean)) {
            const message = JSON.parse(line) as Record<string, unknown>
            record.sent.push(message)
            if (message.type === 'slash') queueMicrotask(() => emit({ type: 'turn_finished' }))
          }
        },
        flush: () => undefined,
        end: () => exit(0),
      },
      stdout,
      stderr: new ReadableStream<Uint8Array>({ start: (controller) => (err = controller) }),
      exited: new Promise<number>((resolve) => (exitWith = resolve)),
      signalCode: null,
      kill: () => exit(137),
    } as unknown as ReturnType<SpawnEngine>
  }
  return { spawn, started }
}

const waitUntil = async (predicate: () => boolean) => {
  const deadline = Date.now() + 2_000
  while (!predicate() && Date.now() < deadline) await new Promise((r) => setTimeout(r, 5))
  expect(predicate()).toBe(true)
}

afterEach(async () => {
  await getNeuronEngineClient().shutdown().catch(() => undefined)
  setEngineClientOptionsForTesting({})
  useEngineStore.getState().reset()
})

describe('a new chat and a resumed chat in the engine', () => {
  test('a new chat clears the engine conversation with --confirm', async () => {
    // A bare `clear` is refused by the engine, so the old context stayed.
    const engines = fakeEngines()
    setEngineClientOptionsForTesting({ spawn: engines.spawn, resolveBinary: () => 'neuron' })
    await startEngineSession({ cwd: process.cwd() })

    await clearEngineConversation()
    expect(engines.started[0]!.sent).toContainEqual({
      kind: 'command',
      type: 'slash',
      command: 'clear',
      args: '--confirm',
    })
  })

  test('a chat picked in history restarts the engine on its own session', async () => {
    const engines = fakeEngines()
    setEngineClientOptionsForTesting({ spawn: engines.spawn, resolveBinary: () => 'neuron' })
    await startEngineSession({ cwd: process.cwd(), model: 'opus' })

    await resumeEngineSessionForChat('session-42')
    await waitUntil(() => engines.started.length === 2)
    const argv = engines.started[1]!.argv
    // The same launch flags, plus the chat's session.
    expect(argv).toContain('--model')
    expect(argv.slice(argv.indexOf('--resume'), argv.indexOf('--resume') + 2)).toEqual([
      '--resume',
      'session-42',
    ])
  })

  test('a chat saved without a session gets a fresh conversation, not the current one', async () => {
    const engines = fakeEngines()
    setEngineClientOptionsForTesting({ spawn: engines.spawn, resolveBinary: () => 'neuron' })
    await startEngineSession({ cwd: process.cwd() })

    await resumeEngineSessionForChat(undefined)
    expect(engines.started.length).toBe(1)
    expect(engines.started[0]!.sent.map((message) => message.args)).toContain('--confirm')
  })
})
