// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the UI
// titles the window "Neuron" from the start, keeps the title it set so it can
// set it again when something else in the console retitles the window (a
// PowerShell that starts there, a command's cmd.exe), and saves the console's
// own title at start to restore it at exit (GetConsoleTitleW/SetConsoleTitleW
// on Windows, the xterm title stack elsewhere) instead of blanking it.
// See ui/neuron-tui/PORTING.md.
/**
 * Terminal title utilities using OSC (Operating System Command) escape sequences.
 *
 * OSC sequence format for setting title:
 * - `\x1b]0;${title}\x07` - Sets both window title and icon name
 * - `\x1b` is ESC, `]0;` starts the title command, `\x07` (BEL) ends it
 *
 * We write directly to /dev/tty to bypass OpenTUI's stdout capture,
 * similar to how clipboard.ts handles OSC52 sequences.
 */

import { getCliEnv } from './env'
import { writeTerminalControlSync } from './terminal-io'
import { win32Console } from './win32-console'

const MAX_TITLE_LENGTH = 60
/** The UI's own title, before the first prompt names the chat. */
export const NEURON_TITLE = 'Neuron'
const TITLE_PREFIX = `${NEURON_TITLE}: `
const OSC_TERMINATOR = '\x07' // BEL
/** xterm window ops: push the window title on the title stack, and pop it. */
export const PUSH_TITLE = '\x1b[22;0t'
export const POP_TITLE = '\x1b[23;0t'

function isInTmux(env: ReturnType<typeof getCliEnv>): boolean {
  return Boolean(env.TMUX)
}

function isInScreen(env: ReturnType<typeof getCliEnv>): boolean {
  if (env.STY) return true
  const term = env.TERM ?? ''
  return term.startsWith('screen') && !isInTmux(env)
}

/**
 * Build the OSC title sequence with tmux/screen passthrough if needed
 */
function buildTitleSequence(title: string, env: ReturnType<typeof getCliEnv>): string {
  const osc = `\x1b]0;${title}${OSC_TERMINATOR}`

  // tmux passthrough: wrap in DCS and double ESC characters
  if (isInTmux(env)) {
    const escaped = osc.replace(/\x1b/g, '\x1b\x1b')
    return `\x1bPtmux;${escaped}\x1b\\`
  }

  // GNU screen passthrough: wrap in DCS
  if (isInScreen(env)) {
    return `\x1bP${osc}\x1b\\`
  }

  return osc
}

/** Where titles go and come from; injectable for tests. */
export interface TerminalTitleHost {
  platform: NodeJS.Platform
  /** Write control bytes to the terminal. */
  write: (sequence: string) => boolean
  /** The console's title (Windows), or `null` when it can't be read. */
  getConsoleTitle: () => string | null
  setConsoleTitle: (title: string) => boolean
}

const defaultHost: TerminalTitleHost = {
  platform: process.platform,
  write: (sequence) => writeTerminalControlSync(sequence),
  getConsoleTitle: () => win32Console.getTitle(),
  setConsoleTitle: (title) => win32Console.setTitle(title),
}

let host: TerminalTitleHost = defaultHost

type SavedTitle = { kind: 'console'; title: string } | { kind: 'stack' }

/** The title the UI set last ("Neuron", "Neuron: <prompt>"); `null` before it set one. */
let currentTitle: string | null = null
/** The terminal's own title, saved at start. */
let savedTitle: SavedTitle | null = null
let saveAttempted = false
/** The title was given back at exit: the UI sets none after that. */
let restored = false

/** Test seam: another host, and a fresh start. `null` puts the real one back. */
export function setTerminalTitleHostForTesting(next: TerminalTitleHost | null): void {
  host = next ?? defaultHost
  currentTitle = null
  savedTitle = null
  saveAttempted = false
  restored = false
}

function sanitize(title: string): string {
  // Remove control characters and newlines
  return title.replace(/[\x00-\x1f\x7f]/g, ' ').trim()
}

function writeTitle(title: string): void {
  if (restored) return
  currentTitle = title
  host.write(buildTitleSequence(title, getCliEnv()))
}

/** "Neuron: <prompt>", truncated to fit a window title; `null` for an empty prompt. */
export function formatPromptTitle(prompt: string): string | null {
  const sanitized = sanitize(prompt)
  if (!sanitized) return null

  // Truncate to reasonable length
  const maxInputLength = MAX_TITLE_LENGTH - TITLE_PREFIX.length
  const truncated =
    sanitized.length > maxInputLength
      ? sanitized.slice(0, maxInputLength - 1) + '…'
      : sanitized
  return `${TITLE_PREFIX}${truncated}`
}

/**
 * Set the terminal window title.
 * Works on most modern terminal emulators, including through tmux and screen.
 *
 * @param title - The title to set (will be truncated if too long)
 */
export function setTerminalTitle(title: string): void {
  const fullTitle = formatPromptTitle(title)
  if (fullTitle) writeTitle(fullTitle)
}

/** Title the window "Neuron", as the UI starts. */
export function setNeuronTitle(): void {
  writeTitle(NEURON_TITLE)
}

/** The title the UI set last, or `null`. */
export function getCurrentTerminalTitle(): string | null {
  return currentTitle
}

/**
 * Set the UI's title again: something else in the console retitled the
 * window (a PowerShell starting there, a command run through cmd.exe).
 */
export function reassertTerminalTitle(): void {
  if (currentTitle !== null) writeTitle(currentTitle)
}

/**
 * Whether the window no longer shows the UI's title. Only a Windows console
 * can be asked; elsewhere this is `null` (unknown).
 */
export function terminalTitleDrifted(): boolean | null {
  if (host.platform !== 'win32' || currentTitle === null || restored) return null
  const shown = host.getConsoleTitle()
  if (shown === null) return null
  return shown !== currentTitle
}

/**
 * Remember the terminal's own title before the UI sets one: read on Windows,
 * pushed on the title stack elsewhere. Once; call it before the first title.
 */
export function saveTerminalTitle(): void {
  if (saveAttempted) return
  saveAttempted = true
  if (host.platform === 'win32') {
    const title = host.getConsoleTitle()
    if (title !== null) savedTitle = { kind: 'console', title }
    return
  }
  if (host.write(PUSH_TITLE)) savedTitle = { kind: 'stack' }
}

/**
 * Give the terminal its own title back, once, at exit. After this the UI
 * sets no title. Without a saved title, the UI's own title is cleared (as
 * Freebuff did at exit).
 */
export function restoreTerminalTitle(): void {
  if (restored) return
  restored = true
  const saved = savedTitle
  savedTitle = null
  if (saved?.kind === 'console') {
    if (!host.setConsoleTitle(saved.title)) {
      host.write(buildTitleSequence(sanitize(saved.title), getCliEnv()))
    }
    return
  }
  if (saved?.kind === 'stack') {
    host.write(POP_TITLE)
    return
  }
  if (currentTitle !== null) host.write(buildTitleSequence('', getCliEnv()))
}
