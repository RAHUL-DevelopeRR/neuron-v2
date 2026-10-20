// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: exit now
// shuts the engine down instead of flushing analytics, logs, ad engagement and
// the Freebuff session. See ui/neuron-tui/PORTING.md.
import { describe, expect, test } from 'bun:test'

import { ensureCliTestEnv } from '../../__tests__/test-utils'

ensureCliTestEnv()

const { createExitCliCleanly } = await import('../exit-cleanly')

describe('createExitCliCleanly', () => {
  test('runs local cleanup, then the bounded engine shutdown, and exits once', async () => {
    const events: string[] = []
    const exitCleanly = createExitCliCleanly({
      cleanupLocal: () => events.push('local-cleanup'),
      shutdownEngine: async () => {
        events.push('engine-shutdown')
      },
      waitForCleanup: async (tasks) => {
        events.push('wait-start')
        await Promise.allSettled(tasks)
        events.push('wait-finish')
      },
      exit: (code) => {
        events.push(`exit-${code}`)
      },
    })

    await exitCleanly(7)

    expect(events).toEqual([
      'local-cleanup',
      'wait-start',
      'engine-shutdown',
      'wait-finish',
      'exit-7',
    ])
  })

  test('a failing engine shutdown or local cleanup never blocks the exit', async () => {
    const exitCodes: number[] = []
    const exitCleanly = createExitCliCleanly({
      cleanupLocal: () => {
        throw new Error('renderer already gone')
      },
      shutdownEngine: async () => {
        throw new Error('engine pipe closed')
      },
      waitForCleanup: async (tasks) => {
        await Promise.allSettled(tasks)
      },
      exit: (code) => exitCodes.push(code),
    })

    await exitCleanly(3)
    expect(exitCodes).toEqual([3])
  })

  test('coalesces competing exit requests and keeps the first exit code', async () => {
    let finishCleanup: (() => void) | undefined
    let cleanupCalls = 0
    const exitCodes: number[] = []
    const exitCleanly = createExitCliCleanly({
      cleanupLocal: () => cleanupCalls++,
      shutdownEngine: async () => {},
      waitForCleanup: () =>
        new Promise<void>((resolve) => {
          finishCleanup = resolve
        }),
      exit: (code) => {
        exitCodes.push(code)
      },
    })

    const first = exitCleanly(0)
    const second = exitCleanly(1)
    expect(second).toBe(first)

    await Promise.resolve()
    expect(cleanupCalls).toBe(1)
    finishCleanup?.()
    await first

    expect(exitCodes).toEqual([0])
  })
})
