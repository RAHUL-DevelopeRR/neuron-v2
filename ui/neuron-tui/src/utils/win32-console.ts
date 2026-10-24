/**
 * The few Win32 console calls the UI makes itself, through bun:ffi (as
 * glyphs.ts does for the console window's class): the console's title, its
 * font and its input mode. Everything here is a no-op returning `null`/`false`
 * off Windows, or when a call fails; the UI never depends on it.
 */

/**
 * The input mode bit that has the console turn keys into escape sequences as
 * they are typed (ENABLE_VIRTUAL_TERMINAL_INPUT). The renderer's raw mode
 * turns it on; a shell reading keys typed under it gets those sequences.
 */
export const ENABLE_VIRTUAL_TERMINAL_INPUT = 0x200

/** A console font, as `GetCurrentConsoleFontEx` reports it (CONSOLE_FONT_INFOEX). */
export interface ConsoleFont {
  /** Index in the console's font table; 0 when setting by face and size. */
  index: number
  /** Cell width (0 when the font picks it) and height, in pixels. */
  width: number
  height: number
  /** TMPF_* pitch and family bits (TMPF_TRUETYPE is 4). */
  family: number
  weight: number
  /** "Consolas", "Lucida Console", "Terminal" (the raster font)... */
  face: string
}

/** sizeof(CONSOLE_FONT_INFOEX): cbSize, nFont, dwFontSize, FontFamily, FontWeight, FaceName[32]. */
export const CONSOLE_FONT_INFOEX_SIZE = 84
const FACE_OFFSET = 20
const FACE_CHARS = 32
const TMPF_TRUETYPE = 0x04

/** A CONSOLE_FONT_INFOEX in `bytes` (little-endian, as Windows lays it out). */
export function decodeConsoleFont(bytes: Uint8Array): ConsoleFont {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  let face = ''
  for (let i = 0; i < FACE_CHARS; i++) {
    const code = view.getUint16(FACE_OFFSET + i * 2, true)
    if (code === 0) break
    face += String.fromCharCode(code)
  }
  return {
    index: view.getUint32(4, true),
    width: view.getInt16(8, true),
    height: view.getInt16(10, true),
    family: view.getUint32(12, true),
    weight: view.getUint32(16, true),
    face,
  }
}

/** `font` as a CONSOLE_FONT_INFOEX, with `cbSize` filled in. */
export function encodeConsoleFont(font: ConsoleFont): Uint8Array {
  const bytes = new Uint8Array(CONSOLE_FONT_INFOEX_SIZE)
  const view = new DataView(bytes.buffer)
  view.setUint32(0, CONSOLE_FONT_INFOEX_SIZE, true)
  view.setUint32(4, font.index, true)
  view.setInt16(8, font.width, true)
  view.setInt16(10, font.height, true)
  view.setUint32(12, font.family, true)
  view.setUint32(16, font.weight, true)
  const face = font.face.slice(0, FACE_CHARS - 1)
  for (let i = 0; i < face.length; i++) {
    view.setUint16(FACE_OFFSET + i * 2, face.charCodeAt(i), true)
  }
  return bytes
}

/** A TrueType font (Consolas, Lucida Console), not the raster "Terminal" one. */
export function isTrueTypeConsoleFont(font: ConsoleFont): boolean {
  return (font.family & TMPF_TRUETYPE) !== 0
}

/** The console calls, injectable so callers are testable on any OS. */
export interface Win32Console {
  getTitle(): string | null
  setTitle(title: string): boolean
  getFont(): ConsoleFont | null
  setFont(font: ConsoleFont): boolean
  /** The console input buffer's mode (CONIN$, so it works whatever stdin is). */
  getInputMode(): number | null
  setInputMode(mode: number): boolean
}

const GENERIC_READ_WRITE = 0xc0000000
const FILE_SHARE_READ_WRITE = 0x3
const OPEN_EXISTING = 3
const TITLE_CHARS = 4096

function wide(text: string): Uint16Array {
  const buffer = new Uint16Array(text.length + 1)
  for (let i = 0; i < text.length; i++) buffer[i] = text.charCodeAt(i)
  return buffer
}

type Kernel32 = {
  GetConsoleTitleW: (title: unknown, size: number) => number
  SetConsoleTitleW: (title: unknown) => number
  CreateFileW: (...args: unknown[]) => unknown
  CloseHandle: (handle: unknown) => number
  GetCurrentConsoleFontEx: (handle: unknown, max: number, info: unknown) => number
  SetCurrentConsoleFontEx: (handle: unknown, max: number, info: unknown) => number
  GetConsoleMode: (handle: unknown, mode: unknown) => number
  SetConsoleMode: (handle: unknown, mode: number) => number
}

let kernel32: { symbols: Kernel32; ptr: (view: ArrayBufferView) => unknown } | null | undefined

function loadKernel32() {
  if (kernel32 !== undefined) return kernel32
  kernel32 = null
  if (process.platform !== 'win32') return kernel32
  try {
    // Loaded only here: bun:ffi and kernel32 exist only on Windows. Kept open
    // for the life of the process (kernel32 is always loaded anyway).
    const { dlopen, FFIType, ptr } = require('bun:ffi') as typeof import('bun:ffi')
    const library = dlopen('kernel32.dll', {
      GetConsoleTitleW: { args: [FFIType.ptr, FFIType.u32], returns: FFIType.u32 },
      SetConsoleTitleW: { args: [FFIType.ptr], returns: FFIType.i32 },
      CreateFileW: {
        args: [
          FFIType.ptr,
          FFIType.u32,
          FFIType.u32,
          FFIType.ptr,
          FFIType.u32,
          FFIType.u32,
          FFIType.ptr,
        ],
        returns: FFIType.ptr,
      },
      CloseHandle: { args: [FFIType.ptr], returns: FFIType.i32 },
      GetCurrentConsoleFontEx: {
        args: [FFIType.ptr, FFIType.i32, FFIType.ptr],
        returns: FFIType.i32,
      },
      SetCurrentConsoleFontEx: {
        args: [FFIType.ptr, FFIType.i32, FFIType.ptr],
        returns: FFIType.i32,
      },
      GetConsoleMode: { args: [FFIType.ptr, FFIType.ptr], returns: FFIType.i32 },
      SetConsoleMode: { args: [FFIType.ptr, FFIType.u32], returns: FFIType.i32 },
    })
    kernel32 = {
      symbols: library.symbols as unknown as Kernel32,
      ptr: (view) => ptr(view as never),
    }
  } catch {
    kernel32 = null
  }
  return kernel32
}

/**
 * Run `use` with a handle to the console's active screen buffer (CONOUT$) or
 * its input buffer (CONIN$), so it works whatever stdout and stdin are. `null`
 * when there is no console.
 */
function withConsole<T>(device: 'CONOUT$' | 'CONIN$', use: (handle: unknown) => T): T | null {
  const k = loadKernel32()
  if (!k) return null
  const name = wide(device)
  const handle = k.symbols.CreateFileW(
    k.ptr(name),
    GENERIC_READ_WRITE,
    FILE_SHARE_READ_WRITE,
    null,
    OPEN_EXISTING,
    0,
    null,
  )
  // INVALID_HANDLE_VALUE comes back as 2^64 - 1.
  if (!handle || (typeof handle === 'number' && handle > Number.MAX_SAFE_INTEGER)) {
    return null
  }
  try {
    return use(handle)
  } finally {
    k.symbols.CloseHandle(handle)
  }
}

/** The real console, on Windows; every call fails soft elsewhere. */
export const win32Console: Win32Console = {
  getTitle() {
    const k = loadKernel32()
    if (!k) return null
    try {
      const buffer = new Uint16Array(TITLE_CHARS)
      const length = k.symbols.GetConsoleTitleW(k.ptr(buffer), buffer.length)
      // 0 is also an empty title; both read as "nothing to restore".
      if (length <= 0) return null
      return String.fromCharCode(...buffer.subarray(0, Math.min(length, buffer.length)))
    } catch {
      return null
    }
  },
  setTitle(title) {
    const k = loadKernel32()
    if (!k) return false
    try {
      return k.symbols.SetConsoleTitleW(k.ptr(wide(title))) !== 0
    } catch {
      return false
    }
  },
  getFont() {
    try {
      return withConsole('CONOUT$', (handle) => {
        const k = loadKernel32()!
        const bytes = new Uint8Array(CONSOLE_FONT_INFOEX_SIZE)
        new DataView(bytes.buffer).setUint32(0, CONSOLE_FONT_INFOEX_SIZE, true)
        if (k.symbols.GetCurrentConsoleFontEx(handle, 0, k.ptr(bytes)) === 0) return null
        return decodeConsoleFont(bytes)
      })
    } catch {
      return null
    }
  },
  setFont(font) {
    try {
      return (
        withConsole('CONOUT$', (handle) => {
          const k = loadKernel32()!
          // By face and size: a saved table index may name another font now.
          const bytes = encodeConsoleFont({ ...font, index: 0 })
          return k.symbols.SetCurrentConsoleFontEx(handle, 0, k.ptr(bytes)) !== 0
        }) ?? false
      )
    } catch {
      return false
    }
  },
  getInputMode() {
    try {
      return withConsole('CONIN$', (handle) => {
        const k = loadKernel32()!
        const mode = new Uint32Array(1)
        if (k.symbols.GetConsoleMode(handle, k.ptr(mode)) === 0) return null
        return mode[0]!
      })
    } catch {
      return null
    }
  },
  setInputMode(mode) {
    try {
      return (
        withConsole('CONIN$', (handle) => {
          const k = loadKernel32()!
          return k.symbols.SetConsoleMode(handle, mode >>> 0) !== 0
        }) ?? false
      )
    } catch {
      return false
    }
  },
}
