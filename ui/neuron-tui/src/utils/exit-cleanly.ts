// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: exit no
// longer flushes analytics, client logs, ad engagement or survey pings, ends a
// Freebuff session, or settles a sponsored run. It shuts the engine down
// instead, before the renderer's teardown, gives the console its own title
// (and font) back, and prints the exit message (the --continue hint) once the
// screen is restored. It exits 1 when the engine crashed during the session,
// and stops reading the keyboard once the quit is asked for.
// See ui/neuron-tui/PORTING.md.
import { writeFileSync, writeSync } from 'fs'

import { withTimeoutOr } from '@neuron-tui/common/util/promise'

import { restoreConsoleAtExit } from './console-guard'
import { writeTerminalControlSync } from './terminal-io'
import { engineCrashedThisSession } from '../engine/engine-session'
import { getNeuronEngineClient } from '../engine/neuron-engine-client'

const EXIT_CLEANUP_TIMEOUT_MS = 1_000
// The quitting key's key-up (and a held Ctrl's) arrives within this, before the
// teardown; see createExitCliCleanly.
const EXIT_TEARDOWN_MIN_WAIT_MS = 500
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
  /** The shortest wait before the renderer's teardown, alongside the engine's stop. */
  beforeTeardown?: () => Promise<void>
  /** Give the console its own title (and font) back, right before the message. */
  restoreConsole?: () => void
  /** The code to exit with for the one asked for (a crashed engine makes 0 a 1). */
  exitCodeFor?: (requested: number) => number
  /**
   * Stop reading the keyboard, as soon as the quit is asked for: keys typed
   * during the exit then stay in the console for the shell, instead of the
   * UI swallowing the start of the user's next command.
   */
  stopReadingInput?: () => void
}

/** A quit that would exit 0 exits 1 after the engine crashed during the session. */
export function exitCodeAfterSession(requested: number, engineCrashed: boolean): number {
  return requested === 0 && engineCrashed ? 1 : requested
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

    try {
      deps.stopReadingInput?.()
    } catch {
      // The quit goes ahead either way.
    }

    // Start on the next microtask so exitPromise is assigned before any cleanup
    // callback can re-enter this function.
    exitPromise = Promise.resolve().then(async () => {
      try {
        // The engine stops first, while the UI is still up. In a classic
        // Windows console window, conhost crashes (an access violation that
        // closes the window) when this process lives on after OpenTUI's
        // renderer has torn down and a key still arrives: the key-up of the
        // Enter or Ctrl-C that asked to quit is enough. So the teardown waits
        // at least for those keys to come in, also when the engine has already
        // stopped and has nothing to wait for, and after it the exit only
        // prints its message and leaves.
        try {
          await deps.waitForCleanup([
            Promise.resolve()
              .then(deps.shutdownEngine)
              .catch(() => {}),
            Promise.resolve()
              .then(deps.beforeTeardown)
              .catch(() => {}),
          ])
        } catch {
          // A stuck engine never keeps the UI open; the wait is bounded too.
        }
        try {
          deps.cleanupLocal()
        } catch {
          // Cleanup is best-effort; never strand the process in a half-exited UI.
        }
        try {
          deps.restoreConsole?.()
        } catch {
          // The title is a nicety too.
        }
        // Awaited only when there is one. A lost message never stops the exit.
        if (deps.printExitMessage) {
          try {
            await deps.printExitMessage()
          } catch {
            // The hint is a nicety.
          }
        }
      } finally {
        let code = exitCode
        try {
          code = deps.exitCodeFor?.(exitCode) ?? exitCode
        } catch {
          // Exit with the code asked for.
        }
        deps.exit(code)
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
  // The renderer stays up and in raw mode until its teardown; only the reading
  // stops. Its teardown pauses stdin again, which is harmless.
  stopReadingInput: () => {
    process.stdin.pause()
  },
  exitCodeFor: (code) => exitCodeAfterSession(code, engineCrashedThisSession()),
  restoreConsole: () => restoreConsoleAtExit(),
  beforeTeardown: () =>
    new Promise((resolve) => setTimeout(resolve, EXIT_TEARDOWN_MIN_WAIT_MS)),
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
