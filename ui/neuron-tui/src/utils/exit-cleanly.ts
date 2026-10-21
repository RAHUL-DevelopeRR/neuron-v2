// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: exit no
// longer flushes analytics, client logs, ad engagement or survey pings, ends a
// Freebuff session, or settles a sponsored run. It shuts the engine down
// instead, and prints the exit message (the --continue hint) once the screen is
// restored. See ui/neuron-tui/PORTING.md.
import { writeFileSync, writeSync } from 'fs'

import { withTimeoutOr } from '@neuron-tui/common/util/promise'

import { writeTerminalControlSync } from './terminal-io'
import { getNeuronEngineClient } from '../engine/neuron-engine-client'

const EXIT_CLEANUP_TIMEOUT_MS = 1_000
// How long the exit message waits for the terminal reset.
const EXIT_MESSAGE_WAIT_MS = 250
// And then for the renderer's native teardown to reach the terminal.
const EXIT_MESSAGE_SETTLE_MS = 200

type ExitCliDependencies = {
  cleanupLocal: () => void
  /** Ask the engine to save and stop (`EngineCommand::Shutdown`). */
  shutdownEngine: () => Promise<void>
  waitForCleanup: (tasks: Promise<void>[]) => Promise<void>
  exit: (code: number) => void
  /** Print the registered exit message, after the screen is restored. */
  printExitMessage?: () => Promise<void> | void
}

let localExitCleanup: (() => void) | undefined
let terminalRestored: (() => Promise<void>) | undefined
let exitMessage: (() => string | null) | undefined

/**
 * Register the synchronous renderer/terminal finalizer once it is available,
 * and how to know when it has put the terminal back.
 */
export function registerExitCleanup(
  cleanup: () => void,
  whenTerminalRestored?: () => Promise<void>,
): void {
  localExitCleanup = cleanup
  terminalRestored = whenTerminalRestored
}

/**
 * A last line for the main screen, such as the --continue hint. It is printed
 * here, not from a process 'exit' listener: on Windows the console is gone by
 * then.
 */
export function registerExitMessage(message: () => string | null): void {
  exitMessage = message
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
      // The exit message goes out before the engine stops, so its wait for the
      // screen to settle never eats into the engine's bounded shutdown. Awaited
      // only when there is one, so without it the engine shutdown starts in the
      // same tick as before. A lost message never stops the exit.
      if (deps.printExitMessage) {
        try {
          await deps.printExitMessage()
        } catch {
          // The hint is a nicety.
        }
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
  printExitMessage: async () => {
    const message = exitMessage?.()
    if (!message) return
    // `neuron` started this UI and prints the message itself once the UI has
    // exited, when the main screen is back for certain; the write below races the
    // renderer's teardown (see rust/crates/rusty-claude-cli/src/full_screen.rs).
    const handOff = process.env.NEURON_TUI_EXIT_MESSAGE_FILE?.trim()
    if (handOff) {
      try {
        writeFileSync(handOff, message)
        return
      } catch {
        // Print it here, as without `neuron`.
      }
    }
    await withTimeoutOr(
      terminalRestored?.() ?? Promise.resolve(),
      EXIT_MESSAGE_WAIT_MS,
      undefined,
    )
    // OpenTUI emits its destroy event before the native teardown that leaves
    // the alternate screen, and on Windows that output goes through a render
    // thread; a line written before it lands on the alternate screen and is
    // lost. Let it settle, then write synchronously.
    await new Promise((resolve) => setTimeout(resolve, EXIT_MESSAGE_SETTLE_MS))
    if (!writeTerminalControlSync(message)) writeSync(1, message)
  },
})
