// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: exit now
// shuts the engine down instead of flushing analytics, logs, ad engagement and
// the Freebuff session, restores the console title before the exit message,
// and exits 1 after an engine crash. See ui/neuron-tui/PORTING.md.
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

  test('the keyboard stops being read as soon as the quit is asked for, once', () => {
    // Keys typed during the exit must stay in the console for the shell.
    const events: string[] = []
    const exitCleanly = createExitCliCleanly({
      stopReadingInput: () => events.push('stop-reading'),
      cleanupLocal: () => events.push('local-cleanup'),
      shutdownEngine: async () => {
        events.push('engine-shutdown')
      },
      waitForCleanup: async (tasks) => {
        await Promise.allSettled(tasks)
      },
      exit: (code) => events.push(`exit-${code}`),
    })

    void exitCleanly(0)
    void exitCleanly(0)
    // Synchronously, before any of the exit's own work.
    expect(events).toEqual(['stop-reading'])
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

  test('the console title comes back right before the message, and a crashed engine exits 1', async () => {
    const events: string[] = []
    const exitCleanly = createExitCliCleanly({
      cleanupLocal: () => events.push('teardown'),
      restoreConsole: () => events.push('title'),
      printExitMessage: () => {
        events.push('message')
      },
      shutdownEngine: async () => {
        events.push('engine-shutdown')
      },
      beforeTeardown: async () => {
        events.push('min-wait')
      },
      waitForCleanup: async (tasks) => {
        await Promise.allSettled(tasks)
      },
      // The engine crashed earlier in the session.
      exitCodeFor: (code) => (code === 0 ? 1 : code),
      exit: (code) => events.push(`exit-${code}`),
    })

    await exitCleanly(0)
    expect(events).toEqual([
      'engine-shutdown',
      'min-wait',
      'teardown',
      'title',
      'message',
      'exit-1',
    ])
  })

  test('a failing title restore never blocks the message or the exit', async () => {
    const events: string[] = []
    const exitCleanly = createExitCliCleanly({
      cleanupLocal: () => {},
      restoreConsole: () => {
        throw new Error('console gone')
      },
      printExitMessage: () => {
        events.push('message')
      },
      shutdownEngine: async () => {},
      waitForCleanup: async (tasks) => {
        await Promise.allSettled(tasks)
      },
      exitCodeFor: () => {
        throw new Error('no engine state')
      },
      exit: (code) => events.push(`exit-${code}`),
    })
    await exitCleanly(4)
    expect(events).toEqual(['message', 'exit-4'])
  })

  test('the teardown waits its minimum also when the engine has already stopped', async () => {
    // A dead engine stops at once; the quitting key's key-up must still come in
    // before the teardown, or a classic Windows console crashes.
    const events: string[] = []
    let releaseWait: (() => void) | undefined
    const exitCleanly = createExitCliCleanly({
      cleanupLocal: () => events.push('local-cleanup'),
      shutdownEngine: async () => {
        events.push('engine-stopped-already')
      },
      beforeTeardown: () =>
        new Promise<void>((resolve) => {
          releaseWait = resolve
        }),
      waitForCleanup: async (tasks) => {
        await Promise.allSettled(tasks)
      },
      exit: (code) => events.push(`exit-${code}`),
    })

    const exiting = exitCleanly(0)
    for (let tick = 0; tick < 5; tick++) await Promise.resolve()
    expect(events).toEqual(['engine-stopped-already'])
    releaseWait?.()
    await exiting
    expect(events).toEqual(['engine-stopped-already', 'local-cleanup', 'exit-0'])
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
