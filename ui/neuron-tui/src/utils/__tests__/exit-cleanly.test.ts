// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: exit now
// shuts the engine down instead of flushing analytics, logs, ad engagement and
// the Freebuff session. See ui/neuron-tui/PORTING.md.
import { describe, expect, test } from 'bun:test'

import { ensureCliTestEnv } from '../../__tests__/test-utils'

ensureCliTestEnv()

const { createExitCliCleanly } = await import('../exit-cleanly')

describe('createExitCliCleanly', () => {
  test('runs the bounded engine shutdown, then local cleanup, and exits once', async () => {
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

    // The renderer's teardown comes last: in a classic Windows console, a key
    // arriving while the process waits after it crashes conhost.
    expect(events).toEqual([
      'wait-start',
      'engine-shutdown',
      'wait-finish',
      'local-cleanup',
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

  test('the exit message is printed after the engine stopped and the screen was restored', async () => {
    const events: string[] = []
    const exitCleanly = createExitCliCleanly({
      cleanupLocal: () => events.push('local-cleanup'),
      printExitMessage: async () => {
        await Promise.resolve()
        events.push('message')
      },
      shutdownEngine: async () => {
        events.push('engine-shutdown')
      },
      waitForCleanup: async (tasks) => {
        await Promise.allSettled(tasks)
      },
      exit: (code) => events.push(`exit-${code}`),
    })

    await exitCleanly(0)
    expect(events).toEqual(['engine-shutdown', 'local-cleanup', 'message', 'exit-0'])
  })

  test('a failing exit message never blocks the exit', async () => {
    const exitCodes: number[] = []
    const exitCleanly = createExitCliCleanly({
      cleanupLocal: () => {},
      printExitMessage: () => {
        throw new Error('console gone')
      },
      shutdownEngine: async () => {},
      waitForCleanup: async (tasks) => {
        await Promise.allSettled(tasks)
      },
      exit: (code) => exitCodes.push(code),
    })

    await exitCleanly(2)
    expect(exitCodes).toEqual([2])
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
    await Promise.resolve()
    expect(finishCleanup).toBeDefined()
    expect(cleanupCalls).toBe(0)
    finishCleanup?.()
    await first

    expect(cleanupCalls).toBe(1)
    expect(exitCodes).toEqual([0])
  })
})
