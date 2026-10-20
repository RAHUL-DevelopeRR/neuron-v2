// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: exit no
// longer flushes analytics, client logs, ad engagement or survey pings, ends a
// Freebuff session, or settles a sponsored run. It shuts the engine down
// instead. See ui/neuron-tui/PORTING.md.
import { withTimeoutOr } from '@neuron-tui/common/util/promise'

import { getNeuronEngineClient } from '../engine/neuron-engine-client'

const EXIT_CLEANUP_TIMEOUT_MS = 1_000

type ExitCliDependencies = {
  cleanupLocal: () => void
  /** Ask the engine to save and stop (`EngineCommand::Shutdown`). */
  shutdownEngine: () => Promise<void>
  waitForCleanup: (tasks: Promise<void>[]) => Promise<void>
  exit: (code: number) => void
}

let localExitCleanup: (() => void) | undefined

/** Register the synchronous renderer/terminal finalizer once it is available. */
export function registerExitCleanup(cleanup: () => void): void {
  localExitCleanup = cleanup
}

/**
 * Build an idempotent exit request. Exported for dependency-injected tests;
 * production uses the singleton below so competing exit triggers converge.
 */
export function createExitCliCleanly(deps: ExitCliDependencies) {
  let exitPromise: Promise<void> | undefined

  return (exitCode = 0): Promise<void> => {
    if (exitPromise) return exitPromise

    // Start on the next microtask so exitPromise is assigned before any cleanup
    // callback can re-enter this function.
    exitPromise = Promise.resolve().then(async () => {
      try {
        deps.cleanupLocal()
      } catch {
        // Cleanup is best-effort; never strand the process in a half-exited UI.
      }

      const tasks = [
        Promise.resolve()
          .then(deps.shutdownEngine)
          .catch(() => {}),
      ]

      try {
        await deps.waitForCleanup(tasks)
      } finally {
        deps.exit(exitCode)
      }
    })

    return exitPromise
  }
}

export const exitCliCleanly = createExitCliCleanly({
  cleanupLocal: () => localExitCleanup?.(),
  shutdownEngine: async () => {
    await getNeuronEngineClient().shutdown()
  },
  waitForCleanup: async (tasks) => {
    await withTimeoutOr(
      Promise.allSettled(tasks),
      EXIT_CLEANUP_TIMEOUT_MS,
      undefined,
    )
  },
  exit: (code) => process.exit(code),
})
