import { describe, expect, test } from 'bun:test'

import {
  BORDERS,
  GLYPHS,
  bordersFor,
  consoleSafeText,
  detectGlyphMode,
  getGlyphMode,
  glyphModeOverride,
  glyphsFor,
  isClassicWindowsConsole,
} from '../glyphs'

const classicWindow = () => 'ConsoleWindowClass'
const pseudoConsole = () => 'PseudoConsoleWindow'
const noWindow = () => null

describe('classic Windows console detection', () => {
  test('the console window decides when it can be asked', () => {
    // A console window opened from Windows Terminal (`start cmd`) inherits
    // WT_SESSION, but it is a classic window all the same.
    expect(isClassicWindowsConsole({ WT_SESSION: 'x' }, 'win32', classicWindow)).toBe(true)
    // Windows Terminal as the default terminal sets no WT_SESSION.
    expect(isClassicWindowsConsole({}, 'win32', pseudoConsole)).toBe(false)
  })

  test('without a console window to ask, a known terminal says no', () => {
    for (const env of [
      { WT_SESSION: 'abc' },
      { TERM_PROGRAM: 'vscode' },
      { ConEmuPID: '1234' },
      { WEZTERM_EXECUTABLE: 'C:\\wezterm.exe' },
      { ALACRITTY_WINDOW_ID: '1' },
      { TERMINAL_EMULATOR: 'JetBrains-JediTerm' },
      { SSH_CONNECTION: '10.0.0.2 5000 10.0.0.1 22' },
      { TERM: 'xterm-256color' },
    ]) {
      expect(isClassicWindowsConsole(env, 'win32', noWindow)).toBe(false)
    }
  })

  test('without either, a console window is the likely host', () => {
    expect(isClassicWindowsConsole({}, 'win32', noWindow)).toBe(true)
    expect(isClassicWindowsConsole({ WT_SESSION: '  ' }, 'win32', noWindow)).toBe(true)
  })

  test('only Windows has one', () => {
    expect(isClassicWindowsConsole({}, 'linux', classicWindow)).toBe(false)
    expect(isClassicWindowsConsole({}, 'darwin', noWindow)).toBe(false)
  })
})

describe('glyph mode', () => {
  test('NEURON_TUI_ASCII overrides the detection both ways', () => {
    expect(glyphModeOverride('1')).toBe('console')
    expect(glyphModeOverride('true')).toBe('console')
    expect(glyphModeOverride('0')).toBe('unicode')
    expect(glyphModeOverride('off')).toBe('unicode')
    expect(glyphModeOverride('')).toBeNull()
    expect(glyphModeOverride('maybe')).toBeNull()
    expect(glyphModeOverride(undefined)).toBeNull()

    expect(detectGlyphMode({ NEURON_TUI_ASCII: '1' }, 'darwin', noWindow)).toBe('console')
    expect(detectGlyphMode({ NEURON_TUI_ASCII: '0' }, 'win32', classicWindow)).toBe('unicode')
  })

  test('a classic console window gets the console set; other terminals Unicode', () => {
    expect(detectGlyphMode({}, 'win32', classicWindow)).toBe('console')
    expect(detectGlyphMode({}, 'win32', pseudoConsole)).toBe('unicode')
    expect(detectGlyphMode({ TERM: 'xterm-256color' }, 'linux', noWindow)).toBe('unicode')
    // The Linux virtual console's fonts lack the same glyphs.
    expect(detectGlyphMode({ TERM: 'linux' }, 'linux', noWindow)).toBe('console')
  })

  test('tests draw with the Unicode set', () => {
    expect(getGlyphMode()).toBe('unicode')
    expect(GLYPHS).toBe(glyphsFor('unicode'))
    expect(BORDERS).toBe(bordersFor('unicode'))
  })
})

// What both default fonts of a classic console window (Consolas and Lucida
// Console) draw: ASCII, Latin-1 and the WGL4 symbols the UI uses.
const CONSOLE_SAFE = new Set([
  ...'─│┌┐└┘├┤┬┴┼═║╔╗╚╝╠╣╦╩╬█▀▄▌▐░▒▓■□▲►▼◄○●◦•·…—–←↑→↓√≡×«»‹›',
])
const isConsoleSafe = (text: string) =>
  [...text].every((char) => char.charCodeAt(0) < 0x100 || CONSOLE_SAFE.has(char))

describe('the console set', () => {
  test('every marker is one the console fonts have', () => {
    for (const [role, glyph] of Object.entries(glyphsFor('console'))) {
      expect({ role, safe: isConsoleSafe(glyph) }).toEqual({ role, safe: true })
    }
  })

  test('every border is square, from characters the console fonts have', () => {
    for (const [style, chars] of Object.entries(bordersFor('console'))) {
      expect({ style, corners: [chars.topLeft, chars.topRight, chars.bottomLeft, chars.bottomRight] })
        .toEqual({ style, corners: ['┌', '┐', '└', '┘'] })
      for (const char of Object.values(chars) as string[]) {
        expect({ style, char, safe: isConsoleSafe(char) }).toEqual({ style, char, safe: true })
      }
    }
  })

  test('the Unicode set keeps the rounded corners and markers', () => {
    expect(bordersFor('unicode').rounded.topLeft).toBe('╭')
    expect(bordersFor('unicode').dashed.horizontal).toBe('┄')
    expect(glyphsFor('unicode').copy).toBe('⎘')
    expect(glyphsFor('unicode').cursor).toBe('▍')
  })

  test('single-column markers stay single-column, so rows line up the same', () => {
    const unicode = glyphsFor('unicode')
    const console = glyphsFor('console')
    for (const role of ['cursor', 'copy', 'check', 'cross', 'collapsed', 'expanded', 'pointer', 'todo'] as const) {
      expect({ role, length: [...console[role]].length }).toEqual({
        role,
        length: [...unicode[role]].length,
      })
    }
  })

  test("the engine's notices are made console-safe in the console set only", () => {
    const notice = '⚠ The response stream was interrupted — retrying once…'
    expect(consoleSafeText(notice, 'console')).toBe(
      '! The response stream was interrupted — retrying once…',
    )
    expect(consoleSafeText('✓ done ✗ failed ╭─╮', 'console')).toBe('√ done × failed ┌─┐')
    expect(consoleSafeText(notice, 'unicode')).toBe(notice)
  })
})
