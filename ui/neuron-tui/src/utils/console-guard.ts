/**
 * Keeps the window's title and (in a classic Windows console) its font the way
 * the UI wants them while it runs, and gives the user's back at exit.
 *
 * Every powershell.exe that starts in a Windows console retitles it "Windows
 * PowerShell" and, in a classic console window whose font is the default
 * TrueType one (HKCU\Console FaceName `__DefaultTTFont__`), switches the window
 * to the raster "Terminal" font, where "…", "›" and "×" degrade. Neither is
 * undone when that PowerShell exits. The UI's terminal-reset watchdog is two
 * such PowerShells (terminal-watchdog.ts), started right after the UI; the
 * engine's PowerShell tool and hooks start more, and a bash tool call's
 * cmd.exe retitles the window to its command. Bun's own switch of the code
 * page to 65001 (restored by Bun at exit) leaves the font alone; OpenTUI
 * makes no title, font or code page calls at all.
 *
 * So: at start the console's title and font are saved and the title becomes
 * "Neuron"; for a few seconds the window is checked and put back when one of
 * those PowerShells changed it (re-set blindly at 1 s and 3 s where the title
 * can't be read back); after every tool result the title is set again and the
 * font repaired; and at exit both are restored. Only a font that went from
 * TrueType to raster is "repaired", so a font the user picks is left alone.
 */

import { probeConsoleWindowClass } from './glyphs'
import {
  reassertTerminalTitle,
  restoreTerminalTitle,
  saveTerminalTitle,
  setNeuronTitle,
  terminalTitleDrifted,
} from './terminal-title'
import {
  isTrueTypeConsoleFont,
  win32Console,
  type ConsoleFont,
  type Win32Console,
} from './win32-console'

/** How often, and for how long after start, the window is checked. */
export const GUARD_POLL_MS = 250
export const GUARD_DURATION_MS = 12_000
/** Where the title can't be read back, it is set again at these times. */
export const BLIND_RESET_AT_MS = [1_000, 3_000] as const

/**
 * The font to put back: the saved one, when the window went from it (a
 * TrueType font) to a raster font. `null` otherwise, including a font the
 * user chose themselves.
 */
export function fontToRepair(
  saved: ConsoleFont | null,
  current: ConsoleFont | null,
): ConsoleFont | null {
  if (!saved || !current) return null
  if (!isTrueTypeConsoleFont(saved) || isTrueTypeConsoleFont(current)) return null
  return saved
}

export interface ConsoleGuardDeps {
  console: Win32Console
  /** A classic console window (conhost), whose font is the window's own. */
  isClassicConsole: () => boolean
  now: () => number
  setInterval: (run: () => void, ms: number) => { unref?: () => void } | number
  clearInterval: (handle: unknown) => void
}

export function createConsoleGuard(deps: ConsoleGuardDeps) {
  let savedFont: ConsoleFont | null = null
  let timer: unknown = null
  let startedAt = 0
  let blindResets = 0
  let stopped = false

  const repairFont = (): boolean => {
    if (!savedFont) return false
    const repair = fontToRepair(savedFont, deps.console.getFont())
    return repair ? deps.console.setFont(repair) : false
  }

  const stopPolling = () => {
    if (timer !== null) deps.clearInterval(timer)
    timer = null
  }

  /** One check: put the title and the font back if something changed them. */
  const check = () => {
    if (stopped) return
    const elapsed = deps.now() - startedAt
    const drifted = terminalTitleDrifted()
    let blindDue = false
    while (
      blindResets < BLIND_RESET_AT_MS.length &&
      elapsed >= BLIND_RESET_AT_MS[blindResets]!
    ) {
      blindResets += 1
      blindDue = true
    }
    if (drifted === true || (drifted === null && blindDue)) reassertTerminalTitle()
    repairFont()
    if (elapsed >= GUARD_DURATION_MS) stopPolling()
  }

  return {
    /** Save the title and font, title the window "Neuron", start checking. */
    start() {
      if (stopped || timer !== null) return
      saveTerminalTitle()
      try {
        if (deps.isClassicConsole()) savedFont = deps.console.getFont()
      } catch {
        savedFont = null
      }
      setNeuronTitle()
      startedAt = deps.now()
      const handle = deps.setInterval(check, GUARD_POLL_MS)
      if (typeof handle === 'object') handle.unref?.()
      timer = handle
    },
    check,
    /**
     * A command ran in the console (a tool call, a hook): it may have
     * retitled the window (cmd.exe) or changed its font (powershell.exe).
     */
    afterConsoleCommand() {
      if (stopped) return
      reassertTerminalTitle()
      repairFont()
    },
    /** At exit: stop checking, put the font and the title back. Once. */
    restore() {
      if (stopped) return
      stopped = true
      stopPolling()
      try {
        repairFont()
      } catch {
        // The console may be gone already.
      }
      restoreTerminalTitle()
    },
  }
}

const guard = createConsoleGuard({
  console: win32Console,
  isClassicConsole: () => probeConsoleWindowClass() === 'ConsoleWindowClass',
  now: () => Date.now(),
  setInterval: (run, ms) => setInterval(run, ms),
  clearInterval: (handle) => clearInterval(handle as ReturnType<typeof setInterval>),
})

/** Call once as the UI starts, before the watchdog and the renderer. */
export const startConsoleGuard = (): void => guard.start()
/** After a tool result or a turn's end: set the title again, repair the font. */
export const afterConsoleCommand = (): void => guard.afterConsoleCommand()
/** In the exit path, right before the exit message (the --continue hint). */
export const restoreConsoleAtExit = (): void => guard.restore()
