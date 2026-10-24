import { afterEach, describe, expect, test } from 'bun:test'

import {
  BLIND_RESET_AT_MS,
  GUARD_DURATION_MS,
  GUARD_POLL_MS,
  createConsoleGuard,
  fontToRepair,
} from '../console-guard'
import { setTerminalTitleHostForTesting } from '../terminal-title'
import {
  CONSOLE_FONT_INFOEX_SIZE,
  ENABLE_VIRTUAL_TERMINAL_INPUT,
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
// The console's input mode in Windows PowerShell 5.1 as it ran `neuron`, and
// under the renderer's raw mode (window input and virtual-terminal input).
const SHELL_INPUT_MODE = 0x1e4
const RAW_INPUT_MODE = 0x208

/** A classic console window: a title and a font that others change. */
function fakeConsole(platform: NodeJS.Platform = 'win32') {
  const state = {
    title: 'Windows PowerShell' as string | null,
    font: { ...CONSOLAS } as ConsoleFont | null,
    fontSets: [] as ConsoleFont[],
    // What PowerShell 5.1 left the console in while it ran `neuron`.
    inputMode: SHELL_INPUT_MODE as number | null,
    inputModeSets: [] as number[],
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
    getInputMode: () => state.inputMode,
    setInputMode: (mode) => {
      state.inputModeSets.push(mode)
      state.inputMode = mode
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
  const intervals: number[] = []
  const guard = createConsoleGuard({
    console: win32,
    isClassicConsole: () => true,
    now: () => now,
    setInterval: (run, ms) => {
      tick = run
      intervals.push(ms)
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
  return { state, guard, advance, intervals, polling: () => tick !== null }
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

  test('a PowerShell that starts late is undone within one 25 ms check', () => {
    const { state, guard, advance, intervals } = fakeConsole()
    guard.start()
    expect(intervals).toEqual([GUARD_POLL_MS])
    expect(GUARD_POLL_MS).toBeLessThanOrEqual(25)
    advance(8_000)
    state.title = 'Windows PowerShell'
    advance(GUARD_POLL_MS)
    expect(state.title).toBe('Neuron')
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

describe("the console's input mode", () => {
  test('on quit only virtual-terminal input goes off; at exit the saved mode comes back', () => {
    const { state, guard } = fakeConsole()
    guard.start()
    // The renderer's raw mode.
    state.inputMode = RAW_INPUT_MODE
    guard.endVirtualTerminalInput()
    // Still raw (a Ctrl-C stays a key), but keys are stored as keys.
    expect(state.inputMode).toBe(RAW_INPUT_MODE & ~ENABLE_VIRTUAL_TERMINAL_INPUT)
    guard.restore()
    expect(state.inputMode).toBe(SHELL_INPUT_MODE)
  })

  test('a mode without virtual-terminal input is left as it is on quit', () => {
    const { state, guard } = fakeConsole()
    guard.start()
    state.inputMode = 0x8
    guard.endVirtualTerminalInput()
    expect(state.inputModeSets).toEqual([])
  })

  test('without a console nothing is set', () => {
    const { state, guard } = fakeConsole('linux')
    state.inputMode = null
    guard.start()
    guard.endVirtualTerminalInput()
    guard.restore()
    expect(state.inputModeSets).toEqual([])
  })

  test('the mode is restored once', () => {
    const { state, guard } = fakeConsole()
    guard.start()
    state.inputMode = RAW_INPUT_MODE
    guard.restore()
    state.inputMode = RAW_INPUT_MODE
    guard.restore()
    expect(state.inputModeSets).toEqual([SHELL_INPUT_MODE])
  })
})
