// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
import { execFile } from 'child_process'

import { resetTerminalTitle } from './terminal-title'
import { stopActiveRun } from './active-run'
import { getCliEnv } from './env'
import { exitCliCleanly, registerExitCleanup } from './exit-cleanly'
import { flushLiveChatState } from './run-state-storage'
import { reportFatalErrorSync, writeTerminalControlSync } from './terminal-io'
import { TERMINAL_RESET_SEQUENCES } from './terminal-reset-sequences'
import { stopTerminalWatchdog } from './terminal-watchdog'

import type { CliRenderer } from '@opentui/core'

/**
 * Signals on which OpenTUI itself destroys the renderer: its defaults WITHOUT
 * SIGPIPE. OpenTUI's handler only calls destroy(), so a SIGPIPE — a write to a
 * pipe whose reader is gone, which Bun otherwise ignores and reports as EPIPE —
 * tore the whole UI down and left the process running behind a dead screen.
 * Worse, destroy() removes the listener, and with no listener left Bun falls
 * back to the default action: the next broken-pipe write killed the CLI on the
 * spot (exit 141) with no cleanup. SIGPIPE is ignored explicitly below; every
 * other signal here also runs our exit path (installProcessCleanupHandlers).
 */
export const CLI_RENDERER_EXIT_SIGNALS: NodeJS.Signals[] = [
  'SIGINT',
  'SIGTERM',
  'SIGQUIT',
  'SIGABRT',
  'SIGHUP',
  'SIGBREAK',
  'SIGBUS',
]

let renderer: CliRenderer | null = null
let handlersInstalled = false
let cleanupStarted = false

function isProcessRunning(pid: number, onResult: (running: boolean) => void) {
  if (process.platform === 'win32') {
    execFile(
      'tasklist',
      ['/FI', `PID eq ${pid}`, '/FO', 'CSV', '/NH'],
      // Bounded: a probe hung on a saturated machine would otherwise stall the
      // (non-overlapping) monitor forever, silently ending launcher watching.
      { windowsHide: true, timeout: 15_000 },
      (error, stdout) => {
        // A failed probe should never terminate a healthy CLI.
        if (error) {
          onResult(true)
          return
        }
        onResult(new RegExp(`(?:^|\\D)${pid}(?:\\D|$)`).test(stdout))
      },
    )
    return
  }

  try {
    process.kill(pid, 0)
    onResult(true)
  } catch (error) {
    onResult((error as NodeJS.ErrnoException).code === 'EPERM')
  }
}

/**
 * Reset terminal state by writing escape sequences to the controlling terminal.
 * This is called after renderer.destroy() so buffered renderer output cannot
 * land on the restored main screen after the reset.
 *
 * This is especially important on Windows where signals like SIGTERM and SIGHUP
 * don't work, so we rely on the 'exit' event which is guaranteed to run.
 */
function resetTerminalState(): boolean {
  try {
    if (process.stdin.isTTY && process.stdin.setRawMode) {
      process.stdin.setRawMode(false)
    }
  } catch {
    // Ignore errors - stdin may already be closed
  }
  try {
    // Reset terminal title to default
    resetTerminalTitle()
    if (!process.stdout.isTTY) return true

    const resetCompletedSynchronously = writeTerminalControlSync(
      TERMINAL_RESET_SEQUENCES,
    )
    if (!resetCompletedSynchronously) {
      // Best-effort immediate reset. Keep the watchdog armed below so it can
      // retry after this process exits if the buffered write is lost.
      process.stdout.write(TERMINAL_RESET_SEQUENCES)
    }
    return resetCompletedSynchronously
  } catch {
    // Ignore errors - stdout may already be closed
    return false
  }
}

/**
 * Destroy OpenTUI before resetting the terminal. destroy() can finalize
 * synchronously or defer until an active frame finishes; in the deferred case,
 * schedule the reset after the destroy event's remaining synchronous work.
 */
function destroyRendererAndResetTerminal(): boolean {
  const activeRenderer = renderer
  renderer = null
  try {
    if (!activeRenderer || activeRenderer.isDestroyed) {
      return resetTerminalState()
    }

    let destroyReturned = false
    let destroyFinalized = false
    activeRenderer.once('destroy', () => {
      destroyFinalized = true
      if (destroyReturned) {
        queueMicrotask(resetTerminalState)
      }
    })

    activeRenderer.destroy()
    destroyReturned = true
    return destroyFinalized ? resetTerminalState() : false
  } catch {
    // A direct reset is still safe if renderer teardown itself failed.
    return resetTerminalState()
  }
}

/**
 * Clean up the renderer by calling destroy().
 * This resets terminal state to prevent garbled output after exit.
 */
function cleanup(): boolean {
  if (cleanupStarted) {
    // The process 'exit' handler deliberately reaches this branch to make the
    // terminal reset the final write, even if destroy was deferred above.
    return resetTerminalState()
  }
  cleanupStarted = true

  // Finalize the active message before reading the live provider. This makes
  // the synchronous flush include the interruption UI and prevents a late
  // SDK callback from continuing to own the chat while shutdown proceeds.
  try {
    stopActiveRun('process-exit')
  } catch {
    // Continue restoring the terminal even if run finalization fails.
  }

  // Persist any in-flight chat state first (synchronous, best-effort) so
  // closing the terminal or killing the process mid-run doesn't lose the turn.
  try {
    flushLiveChatState()
  } catch {
    // Persistence is best-effort during process teardown.
  }

  return destroyRendererAndResetTerminal()
}

/** Restore the terminal, report a fatal error synchronously, and exit. */
export function exitCliWithFatalError(label: string, error: unknown): never {
  const resetCompletedSynchronously = cleanup() || resetTerminalState()
  if (resetCompletedSynchronously) {
    stopTerminalWatchdog()
  }
  reportFatalErrorSync(label, error)
  process.exit(1)
}

/**
 * Install process-level signal handlers to ensure terminal cleanup on all exit scenarios.
 * Call this once after creating the renderer in index.tsx.
 *
 * This handles:
 * - SIGTERM (kill)
 * - SIGHUP (terminal hangup)
 * - SIGINT (Ctrl+C)
 * - release launcher exit
 * - beforeExit / exit events
 * - uncaughtException / unhandledRejection
 *
 * Note: SIGKILL cannot be caught - it's an immediate termination signal.
 */
export function installProcessCleanupHandlers(cliRenderer: CliRenderer): void {
  if (handlersInstalled) return
  handlersInstalled = true
  renderer = cliRenderer
  registerExitCleanup(cleanup)

  const handleExitRequest = () => {
    void exitCliCleanly()
  }

  // A signal from outside the app: the terminal window closed (SIGHUP), a
  // kill/shutdown (SIGTERM), or the launcher gone.
  const handleExternalExit = handleExitRequest

  // A broad `taskkill node.exe` can kill the launcher process without
  // killing its Bun child. Polling avoids Bun's Windows behavior of terminating
  // before JavaScript can handle a broken IPC channel or inherited pipe.
  const launcherPid = Number(getCliEnv().NEURON_TUI_LAUNCHER_PID)
  if (
    Number.isInteger(launcherPid) &&
    launcherPid > 0 &&
    launcherPid !== process.pid
  ) {
    let launcherCheckInFlight = false
    const launcherMonitor = setInterval(() => {
      if (launcherCheckInFlight) return
      launcherCheckInFlight = true
      isProcessRunning(launcherPid, (running) => {
        launcherCheckInFlight = false
        if (running) return
        clearInterval(launcherMonitor)
        handleExternalExit()
      })
    }, 500)
    launcherMonitor.unref()
  }

  // SIGTERM - Default kill signal (e.g., `kill <pid>`)
  process.on('SIGTERM', handleExternalExit)

  // SIGHUP - Terminal hangup (e.g., closing the terminal window)
  process.on('SIGHUP', handleExternalExit)

  // SIGINT - Ctrl+C
  process.on('SIGINT', handleExitRequest)

  // The rest of OpenTUI's exit signals. Its own handler only destroys the
  // renderer, which without an exit left a live process behind a dead screen.
  for (const signal of ['SIGQUIT', 'SIGABRT', 'SIGBUS'] as const) {
    process.on(signal, handleExternalExit)
  }
  if (process.platform === 'win32') process.on('SIGBREAK', handleExternalExit)

  // A broken pipe is a write error (EPIPE), not a request to quit. Keep a
  // listener for the life of the process: with none, Bun restores the default
  // action and a single SIGPIPE terminates the CLI.
  process.on('SIGPIPE', () => {})

  // beforeExit - Called when the event loop is empty and about to exit
  process.on('beforeExit', () => {
    cleanup()
  })

  // exit - Last chance to run synchronous cleanup code
  process.on('exit', () => {
    // Only silence the external fallback after the final reset bytes were
    // synchronously accepted. If direct terminal access failed, leave it armed
    // to repair the terminal after this process disappears.
    if (cleanup()) {
      stopTerminalWatchdog()
    }
  })

  // uncaughtException - Safety net for unhandled errors
  process.on('uncaughtException', (error) => {
    exitCliWithFatalError('Uncaught exception', error)
  })

  // unhandledRejection - Safety net for unhandled promise rejections
  process.on('unhandledRejection', (reason) => {
    exitCliWithFatalError('Unhandled rejection', reason)
  })
}
