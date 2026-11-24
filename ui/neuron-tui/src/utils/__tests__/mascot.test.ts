import { describe, expect, test } from 'bun:test'
import fs from 'fs'
import path from 'path'

import {
  MASCOT_COLORS,
  MASCOT_GRID,
  MASCOT_HEIGHT,
  MASCOT_WIDTH,
  mascotLinesFor,
  renderMascotLines,
} from '../mascot'

// The script that draws the SVG forms of Neura is the grid's source of truth.
// It is read as text, not imported: running it writes the SVG files.
const BUILD_SCRIPT = path.resolve(import.meta.dir, '../../../../../assets/mascot/build-mascot.mjs')
const SYNC_HINT = `keep MASCOT_GRID and MASCOT_COLORS in ui/neuron-tui/src/utils/mascot.ts in sync with GRID and COLORS in ${BUILD_SCRIPT}`

function parseBuildScript(): { grid: string[]; colors: Record<string, string> } {
  const source = fs.readFileSync(BUILD_SCRIPT, 'utf8')
  const gridBody = /export const GRID = \[([\s\S]*?)\];/.exec(source)?.[1]
  const colorsBody = /export const COLORS = \{([^}]*)\}/.exec(source)?.[1]
  if (gridBody == null || colorsBody == null) {
    throw new Error(`could not find GRID and COLORS in ${BUILD_SCRIPT}; ${SYNC_HINT}`)
  }
  const grid = [...gridBody.matchAll(/'([^']*)'/g)].map((match) => match[1])
  const colors = Object.fromEntries(
    [...colorsBody.matchAll(/(\w+)\s*:\s*'([^']*)'/g)].map((match) => [match[1], match[2]]),
  )
  return { grid, colors }
}

describe('mascot source', () => {
  test('the grid and colours match assets/mascot/build-mascot.mjs', () => {
    const { grid, colors } = parseBuildScript()
    expect(grid.length).toBeGreaterThan(0)
    if (JSON.stringify(grid) !== JSON.stringify(MASCOT_GRID)) {
      throw new Error(`the mascot grid differs from the build script's; ${SYNC_HINT}`)
    }
    if (JSON.stringify(colors) !== JSON.stringify(MASCOT_COLORS)) {
      throw new Error(`the mascot colours differ from the build script's; ${SYNC_HINT}`)
    }
  })

  test('every row is as wide as the first, and every cell has a colour', () => {
    for (const row of MASCOT_GRID) {
      expect(row.length).toBe(MASCOT_WIDTH)
      for (const cell of row) {
        if (cell !== '.') expect(MASCOT_COLORS[cell]).toBeDefined()
      }
    }
  })
})

describe('half-block rendering', () => {
  const colors = { A: '#111111', B: '#222222', C: '#111111' }

  test('each pair of cells becomes one character', () => {
    // Columns: empty over empty, empty over A, A over empty, A over A,
    // A over B, and A over C (another key with A's colour).
    const lines = renderMascotLines(['..AAAA', '.A.ABC'], colors)
    expect(lines).toEqual([
      [
        { char: ' ' },
        { char: '▄', fg: '#111111' },
        { char: '▀', fg: '#111111' },
        { char: '█', fg: '#111111' },
        { char: '▀', fg: '#111111', bg: '#222222' },
        { char: '█', fg: '#111111' },
      ],
    ])
  })

  test('an empty cell never paints a background', () => {
    const [first, last] = renderMascotLines(['.A.B', 'A..B', 'AB..'], colors)
    // Only a top half over a bottom half of another colour has a background.
    expect(first).toEqual([
      { char: '▄', fg: '#111111' },
      { char: '▀', fg: '#111111' },
      { char: ' ' },
      { char: '█', fg: '#222222' },
    ])
    // An odd last row is drawn as top halves over nothing.
    expect(last).toEqual([
      { char: '▀', fg: '#111111' },
      { char: '▀', fg: '#222222' },
      { char: ' ' },
      { char: ' ' },
    ])
  })

  test('a cell with no colour is an error, not a blank', () => {
    expect(() => renderMascotLines(['Z'], colors)).toThrow('"Z"')
  })

  test('Neura is 18 columns by 7 lines', () => {
    const lines = renderMascotLines()
    expect(lines.length).toBe(MASCOT_HEIGHT)
    expect(MASCOT_HEIGHT).toBe(7)
    for (const line of lines) expect(line.length).toBe(MASCOT_WIDTH)
    expect(MASCOT_WIDTH).toBe(18)
  })

  test('the console glyph set leaves the mascot out', () => {
    expect(mascotLinesFor('console')).toBeNull()
    expect(mascotLinesFor('unicode')).toEqual(renderMascotLines())
  })
})
