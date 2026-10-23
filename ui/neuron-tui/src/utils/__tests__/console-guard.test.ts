import { afterEach, describe, expect, test } from 'bun:test'

import {
  BLIND_RESET_AT_MS,
  GUARD_DURATION_MS,
  createConsoleGuard,
  fontToRepair,
} from '../console-guard'
import { setTerminalTitleHostForTesting } from '../terminal-title'
import {
  CONSOLE_FONT_INFOEX_SIZE,
  decodeConsoleFont,
  encodeConsoleFont,
  isTrueTypeConsoleFont,
  type ConsoleFont,
  type Win32Console,
} from '../win32-console'

// What GetCurrentConsoleFontEx reported in a classic console window before
// and after a powershell.exe started in it (Windows 11, HKCU\Console FaceName
// __DefaultTTFont__).
const CONSOLAS: ConsoleFont = { index: 0, width: 7, height: 16, family: 54, weight: 400, face: 'Consolas' }
const RASTER: ConsoleFont = { index: 0, width: 8, height: 12, family: 48, weight: 400, face: 'Terminal' }
const CASCADIA: ConsoleFont = { index: 0, width: 0, height: 16, family: 54, weight: 400, face: 'Cascadia Mono' }

/** A classic console window: a title and a font that others change. */
function fakeConsole(platform: NodeJS.Platform = 'win32') {
  const state = {
    title: 'Windows PowerShell' as string | null,
    font: { ...CONSOLAS } as ConsoleFont | null,
    fontSets: [] as ConsoleFont[],
  }
  const win32: Win32Console = {
    getTitle: () => state.title,
    setTitle: (title) => {
      state.title = title
      return true
    },
    getFont: () => (state.font ? { ...state.font } : null),
    setFont: (font) => {
      state.fontSets.push(font)
      state.font = { ...font }
      return true
    },
  }
  setTerminalTitleHostForTesting({
    platform,
    write: (sequence) => {
      const osc = /^\x1b\]0;(.*)\x07$/.exec(sequence)
      if (osc) state.title = osc[1]!
      return true
    },
    getConsoleTitle: () => (platform === 'win32' ? state.title : null),
    setConsoleTitle: win32.setTitle,
  })
  let now = 0
  let tick: (() => void) | null = null
  const guard = createConsoleGuard({
    console: win32,
    isClassicConsole: () => true,
    now: () => now,
    setInterval: (run) => {
      tick = run
      return 1
    },
    clearInterval: () => {
      tick = null
    },
  })
  const advance = (ms: number) => {
    now += ms
    tick?.()
  }
  return { state, guard, advance, polling: () => tick !== null }
}

afterEach(() => setTerminalTitleHostForTesting(null))

describe('console fonts', () => {
  test('CONSOLE_FONT_INFOEX round-trips', () => {
    const bytes = encodeConsoleFont(CONSOLAS)
    expect(bytes.length).toBe(CONSOLE_FONT_INFOEX_SIZE)
    expect(new DataView(bytes.buffer).getUint32(0, true)).toBe(CONSOLE_FONT_INFOEX_SIZE)
    expect(decodeConsoleFont(bytes)).toEqual(CONSOLAS)
  })

  test('only a switch from a TrueType font to a raster one is repaired', () => {
    expect(isTrueTypeConsoleFont(CONSOLAS)).toBe(true)
    expect(isTrueTypeConsoleFont(RASTER)).toBe(false)
    expect(fontToRepair(CONSOLAS, RASTER)).toEqual(CONSOLAS)
    expect(fontToRepair(CONSOLAS, CONSOLAS)).toBeNull()
    // A font the user picked themselves stays.
    expect(fontToRepair(CONSOLAS, CASCADIA)).toBeNull()
    // A window that was on the raster font already stays on it.
    expect(fontToRepair(RASTER, RASTER)).toBeNull()
    expect(fontToRepair(null, RASTER)).toBeNull()
  })
})

describe('the console guard', () => {
  test('saves the title and font, says "Neuron", and undoes what a PowerShell changed', () => {
    const { state, guard, advance, polling } = fakeConsole()
    guard.start()
    expect(state.title).toBe('Neuron')

    // The watchdog's powershell.exe starts in the console.
    state.title = 'Windows PowerShell'
    state.font = { ...RASTER }
    advance(250)
    expect(state.title).toBe('Neuron')
    expect(state.font).toEqual(CONSOLAS)

    // The checks stop after a while.
    advance(GUARD_DURATION_MS)
    expect(polling()).toBe(false)

    // At exit both come back as they were before the UI.
    state.font = { ...RASTER }
    guard.restore()
    expect(state.title).toBe('Windows PowerShell')
    expect(state.font).toEqual(CONSOLAS)
  })

  test('after a tool result the title is set again and the font repaired', () => {
    const { state, guard } = fakeConsole()
    guard.start()
    state.title = 'npm test'
    state.font = { ...RASTER }
    guard.afterConsoleCommand()
    expect(state.title).toBe('Neuron')
    expect(state.font).toEqual(CONSOLAS)
  })

  test('where the title cannot be read back it is set again at 1 s and 3 s', () => {
    const { state, guard, advance } = fakeConsole('linux')
    guard.start()
    expect(state.title).toBe('Neuron')
    state.title = 'something else'
    advance(BLIND_RESET_AT_MS[0] - 1)
    expect(state.title).toBe('something else')
    advance(1)
    expect(state.title).toBe('Neuron')
    state.title = 'something else'
    advance(BLIND_RESET_AT_MS[1] - BLIND_RESET_AT_MS[0])
    expect(state.title).toBe('Neuron')
    state.title = 'something else'
    advance(1_000)
    expect(state.title).toBe('something else')
  })

  test('after the restore nothing is set again', () => {
    const { state, guard, advance } = fakeConsole()
    guard.start()
    guard.restore()
    expect(state.title).toBe('Windows PowerShell')
    state.font = { ...RASTER }
    guard.afterConsoleCommand()
    advance(250)
    guard.restore()
    expect(state.title).toBe('Windows PowerShell')
    expect(state.font).toEqual(RASTER)
    expect(state.fontSets).toEqual([])
  })

  test('a user font change during the session is left alone', () => {
    const { state, guard } = fakeConsole()
    guard.start()
    state.font = { ...CASCADIA }
    guard.afterConsoleCommand()
    guard.restore()
    expect(state.font).toEqual(CASCADIA)
    expect(state.fontSets).toEqual([])
  })
})
