/**
 * The characters the UI draws its frames and markers with, chosen once at
 * start for the terminal it runs in.
 *
 * A classic Windows console window (conhost.exe: the default for Windows
 * PowerShell 5.1 and cmd on Windows 10) draws with Consolas or Lucida
 * Console, and neither has the rounded corners (╭╮╰╯), dashed lines (┄┆┈┊)
 * or most of the markers (⎘ ▍ ▸ ▾ ✓ ✗ ❯ ☐ ...) the UI uses: they show as "?".
 * There it gets the `console` set: square corners (┌┐└┘, which with ─│ both
 * fonts have) and markers from the old console character set (WGL4/CP437:
 * √ × ► ▼ ▌ ...) or plain ASCII. Windows Terminal, VS Code, WezTerm and the
 * other ConPTY terminals, and every terminal elsewhere, keep the `unicode` set.
 *
 * `NEURON_TUI_ASCII=1` forces the console set anywhere, `NEURON_TUI_ASCII=0`
 * the Unicode one (a classic console window with a font that has the glyphs,
 * such as Cascadia Mono).
 */

import { IS_TEST } from './runtime-env'

import type { BorderCharacters } from '@opentui/core'

export type GlyphMode = 'unicode' | 'console'

/** The env var that overrides the detection: `1` console glyphs, `0` Unicode. */
export const GLYPH_MODE_ENV = 'NEURON_TUI_ASCII'

type Env = Record<string, string | undefined>

/** `NEURON_TUI_ASCII` read as a choice, or `null` when it does not decide. */
export function glyphModeOverride(value: string | undefined): GlyphMode | null {
  const normalized = value?.trim().toLowerCase()
  if (!normalized) return null
  if (['1', 'true', 'yes', 'on'].includes(normalized)) return 'console'
  if (['0', 'false', 'no', 'off'].includes(normalized)) return 'unicode'
  return null
}

/**
 * Variables only a terminal other than a classic console window sets for the
 * shells it runs. A console window opened from one of them (`start cmd`)
 * inherits them, so they decide only when the console window itself can't be
 * asked (see {@link isClassicWindowsConsole}).
 */
const MODERN_TERMINAL_VARS = [
  'WT_SESSION', // Windows Terminal
  'TERM_PROGRAM', // VS Code, WezTerm, Hyper, Tabby, mintty, Warp...
  'ConEmuPID', // ConEmu, Cmder
  'ConEmuANSI',
  'WEZTERM_EXECUTABLE',
  'WEZTERM_PANE',
  'ALACRITTY_WINDOW_ID',
  'ALACRITTY_LOG',
  'KITTY_WINDOW_ID',
  'TERMINAL_EMULATOR', // JetBrains IDEs
  'SSH_CONNECTION', // the client's terminal draws
  'SSH_TTY',
] as const

/**
 * The window class of this process's console window: `ConsoleWindowClass` for
 * a classic console window, `PseudoConsoleWindow` for a ConPTY (Windows
 * Terminal, VS Code and the like, also when Windows Terminal took a console
 * launch over as the default terminal, which sets no WT_SESSION). `null` when
 * there is no console window, it is hidden (ConEmu draws a hidden console
 * window itself), or the question failed.
 */
export type ConsoleWindowProbe = () => string | null

export const probeConsoleWindowClass: ConsoleWindowProbe = () => {
  if (process.platform !== 'win32') return null
  try {
    // Loaded only here: bun:ffi and the Win32 DLLs exist only on Windows.
    const { dlopen, FFIType, ptr } = require('bun:ffi') as typeof import('bun:ffi')
    const kernel32 = dlopen('kernel32.dll', {
      GetConsoleWindow: { args: [], returns: FFIType.ptr },
    })
    try {
      const hwnd = kernel32.symbols.GetConsoleWindow()
      if (!hwnd) return null
      const user32 = dlopen('user32.dll', {
        GetClassNameW: {
          args: [FFIType.ptr, FFIType.ptr, FFIType.i32],
          returns: FFIType.i32,
        },
        IsWindowVisible: { args: [FFIType.ptr], returns: FFIType.i32 },
      })
      try {
        const name = new Uint16Array(256)
        const length = user32.symbols.GetClassNameW(hwnd, ptr(name), name.length)
        if (length <= 0) return null
        const windowClass = String.fromCharCode(...name.subarray(0, length))
        // A pseudo console's window is never shown; a console window that
        // is hidden is drawn by someone else.
        if (
          windowClass === 'ConsoleWindowClass' &&
          user32.symbols.IsWindowVisible(hwnd) === 0
        ) {
          return null
        }
        return windowClass
      } finally {
        user32.close()
      }
    } finally {
      kernel32.close()
    }
  } catch {
    return null
  }
}

/**
 * Whether the UI draws in a classic Windows console window. The console
 * window's class decides when it can be read; otherwise the environment of a
 * known terminal says no, and without one a console window is the likely
 * host (Windows PowerShell 5.1 or cmd started from the Start menu).
 */
export function isClassicWindowsConsole(
  env: Env = process.env,
  platform: NodeJS.Platform = process.platform,
  probe: ConsoleWindowProbe = probeConsoleWindowClass,
): boolean {
  if (platform !== 'win32') return false
  const windowClass = probe()
  if (windowClass === 'ConsoleWindowClass') return true
  if (windowClass === 'PseudoConsoleWindow') return false
  if (MODERN_TERMINAL_VARS.some((name) => (env[name] ?? '').trim() !== '')) return false
  // mintty, Cygwin and MSYS2 terminals set TERM; a console window does not.
  if ((env.TERM ?? '').trim() !== '') return false
  return true
}

/**
 * The glyph set for this terminal: the override first, then the console
 * check. The Linux virtual console's fonts lack the same glyphs.
 */
export function detectGlyphMode(
  env: Env = process.env,
  platform: NodeJS.Platform = process.platform,
  probe: ConsoleWindowProbe = probeConsoleWindowClass,
): GlyphMode {
  const override = glyphModeOverride(env[GLYPH_MODE_ENV])
  if (override) return override
  if (platform !== 'win32' && env.TERM === 'linux') return 'console'
  return isClassicWindowsConsole(env, platform, probe) ? 'console' : 'unicode'
}

/** The markers the UI draws, by role. */
export interface Glyphs {
  /** The input's cursor. */
  cursor: string
  /** The copy button, and its hover and done labels. */
  copy: string
  copyHover: string
  copied: string
  /** Done (a todo, a question, a clicked follow-up, an agent). */
  check: string
  /** Failed (a tool row, a command, an agent). */
  cross: string
  /** A close button. */
  close: string
  /** A collapsed row, and an expanded one. */
  collapsed: string
  expanded: string
  /** "collapse" buttons. */
  collapseUp: string
  /** An accordion header, closed and open. */
  disclosureClosed: string
  disclosureOpen: string
  /** The selected row, and the shell-mode prompt. */
  pointer: string
  /** Before an answer to a question. */
  answer: string
  /** Multi-select boxes. */
  checkboxOn: string
  checkboxOff: string
  /** A todo not done yet. */
  todo: string
  /** The todo item in progress. */
  todoActive: string
  /** The busy spinner's frames, one character each. */
  spinnerFrames: string
  /** A cancelled agent. */
  cancelled: string
  /** A paused queue. */
  paused: string
  /** The Shift key in key hints. */
  shift: string
  /** An external link. */
  external: string
  /** Before a warning or an error line. */
  warning: string
  error: string
  /** Attachments, folders, files, images and pasted text. */
  attachment: string
  folder: string
  folderOpen: string
  file: string
  image: string
  pastedText: string
}

const UNICODE_GLYPHS: Glyphs = {
  cursor: '▍',
  copy: '⎘',
  copyHover: '[⎘ copy]',
  copied: '[✔ copied]',
  check: '✓',
  cross: '✗',
  close: '✕',
  collapsed: '▸',
  expanded: '▾',
  collapseUp: '▴',
  disclosureClosed: '▶',
  disclosureOpen: '▼',
  pointer: '❯',
  answer: '↳',
  checkboxOn: '☑',
  checkboxOff: '☐',
  todo: '☐',
  todoActive: '▸',
  spinnerFrames: '⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏',
  cancelled: '⊘',
  paused: '⏸',
  shift: '⇧',
  external: '↗',
  warning: '⚠️',
  error: '❌',
  attachment: '📎',
  folder: '📁',
  folderOpen: '📂',
  file: '📄',
  image: '📷',
  pastedText: '📋',
}

/**
 * What a classic console window's fonts (Consolas, Lucida Console) have: the
 * WGL4/CP437 characters, else ASCII. Kept to one column where the Unicode
 * marker is one, so rows line up the same.
 */
const CONSOLE_GLYPHS: Glyphs = {
  cursor: '▌',
  copy: '≡',
  copyHover: '[≡ copy]',
  copied: '[√ copied]',
  check: '√',
  cross: '×',
  close: '×',
  collapsed: '►',
  expanded: '▼',
  collapseUp: '▲',
  disclosureClosed: '►',
  disclosureOpen: '▼',
  pointer: '>',
  answer: '└',
  checkboxOn: '[x]',
  checkboxOff: '[ ]',
  todo: '○',
  todoActive: '►',
  spinnerFrames: '|/-\\',
  cancelled: '-',
  paused: '||',
  shift: 'Shift+',
  external: '->',
  warning: '!',
  error: '×',
  attachment: '+',
  folder: '+',
  folderOpen: '-',
  file: '=',
  image: '#',
  pastedText: '+',
}

export function glyphsFor(mode: GlyphMode): Glyphs {
  return mode === 'console' ? CONSOLE_GLYPHS : UNICODE_GLYPHS
}

/** The frames the UI draws: rounded by default, dashed for ghost rows. */
export interface BorderSets {
  /** Most boxes: the input, dialogs, panels. */
  rounded: BorderCharacters
  /** Ghost/ephemeral rows (a pending shell command). */
  dashed: BorderCharacters
  /** Proposal cards. */
  dotted: BorderCharacters
  /** Image cards. */
  square: BorderCharacters
}

const JOINTS = {
  topT: '┬',
  bottomT: '┴',
  leftT: '├',
  rightT: '┤',
  cross: '┼',
} as const

const ROUNDED_CORNERS = {
  topLeft: '╭',
  topRight: '╮',
  bottomLeft: '╰',
  bottomRight: '╯',
} as const

const SQUARE_CORNERS = {
  topLeft: '┌',
  topRight: '┐',
  bottomLeft: '└',
  bottomRight: '┘',
} as const

const SQUARE: BorderCharacters = {
  ...SQUARE_CORNERS,
  horizontal: '─',
  vertical: '│',
  ...JOINTS,
}

const UNICODE_BORDERS: BorderSets = {
  rounded: { ...ROUNDED_CORNERS, horizontal: '─', vertical: '│', ...JOINTS },
  dashed: { ...ROUNDED_CORNERS, horizontal: '┄', vertical: '┆', ...JOINTS },
  dotted: { ...ROUNDED_CORNERS, horizontal: '┈', vertical: '┊', ...JOINTS },
  square: SQUARE,
}

// The console fonts have no dashed or dotted lines either.
const CONSOLE_BORDERS: BorderSets = {
  rounded: SQUARE,
  dashed: SQUARE,
  dotted: SQUARE,
  square: SQUARE,
}

export function bordersFor(mode: GlyphMode): BorderSets {
  return mode === 'console' ? CONSOLE_BORDERS : UNICODE_BORDERS
}

/** Markers the engine's notices use, and what a console window can draw instead. */
const CONSOLE_TEXT_REPLACEMENTS: ReadonlyArray<readonly [RegExp, string]> = [
  [/⚠️?/g, '!'], // ⚠
  [/[✓✔]/g, '√'], // ✓ ✔
  [/[✗✘✕]/g, '×'], // ✗ ✘ ✕
  [/╭/g, '┌'],
  [/╮/g, '┐'],
  [/╰/g, '└'],
  [/╯/g, '┘'],
  [/▸/g, '►'], // ▸
  [/▾/g, '▼'], // ▾
  [/❯/g, '>'], // ❯
  [/↳/g, '└'], // ↳
]

/**
 * Text from elsewhere (the engine's notices) in characters this terminal's
 * font has: unchanged outside the console set.
 */
export function consoleSafeText(text: string, mode: GlyphMode = getGlyphMode()): string {
  if (mode !== 'console') return text
  let safe = text
  for (const [pattern, replacement] of CONSOLE_TEXT_REPLACEMENTS) {
    safe = safe.replace(pattern, replacement)
  }
  return safe
}

let resolvedMode: GlyphMode | null = null

/**
 * The glyph mode, detected once. Tests get the Unicode set unless they choose
 * (their renders and snapshots must not depend on the console running them).
 */
export function getGlyphMode(): GlyphMode {
  resolvedMode ??= IS_TEST ? 'unicode' : detectGlyphMode()
  return resolvedMode
}

/** The markers for this terminal. */
export const GLYPHS: Glyphs = glyphsFor(getGlyphMode())

/** The frames for this terminal. */
export const BORDERS: BorderSets = bordersFor(getGlyphMode())
