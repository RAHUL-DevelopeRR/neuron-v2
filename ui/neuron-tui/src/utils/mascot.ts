/**
 * Neura, Neuron's mascot, for the terminal: a small pixel neuron cell drawn
 * with half-block characters beside the logo in the welcome header.
 *
 * The grid and its colours are a copy of the ones in
 * assets/mascot/build-mascot.mjs, which also draws the SVG forms of Neura;
 * utils/__tests__/mascot.test.ts fails when the two differ.
 */

import type { GlyphMode } from './glyphs'

/** B body, L body highlight, E eye, S spark (the signal), `.` empty. */
export const MASCOT_GRID: readonly string[] = [
  '........S.........',
  '..S.....B.....S...',
  '...B....B....B....',
  '....B...B...B.....',
  '.....BBBBBBB......',
  '....BBBBBBBBB.....',
  '...BBLBBBBBBBB....',
  '...BLBEBBBEBBB....',
  '...BBBEBBBEBBB....',
  '...BBBBBBBBBBB....',
  '....BBBBBBBBBBBB..',
  '.....BBBBBBB...B..',
  '..............SSS.',
]

export const MASCOT_COLORS: Readonly<Record<string, string>> = {
  B: '#3B82F6',
  L: '#7AA9F9',
  E: '#0B1220',
  S: '#5AC8FA',
}

/** The empty cell: it paints nothing, so the terminal's own background shows. */
const EMPTY = '.'

/**
 * One character of the drawn mascot. `fg` and `bg` are left out where a cell
 * is empty, so nothing is painted over the theme's background.
 */
export interface MascotCell {
  char: string
  fg?: string
  bg?: string
}

export type MascotLine = MascotCell[]

/**
 * The grid as terminal lines, two grid rows to a line: `▀` takes the top
 * cell's colour as its foreground and the bottom cell's as its background,
 * `▄` draws a bottom cell under an empty top one, `█` two cells of one
 * colour, and a space two empty cells. An odd last row has an empty row
 * under it. Neighbouring cells that look the same are not merged, so every
 * line has one cell per grid column.
 */
export function renderMascotLines(
  grid: readonly string[] = MASCOT_GRID,
  colors: Readonly<Record<string, string>> = MASCOT_COLORS,
): MascotLine[] {
  const width = Math.max(0, ...grid.map((row) => row.length))
  const colorOf = (key: string): string | undefined => {
    if (key === EMPTY) return undefined
    const color = colors[key]
    if (!color) throw new Error(`mascot grid uses "${key}", which has no colour`)
    return color
  }
  const lines: MascotLine[] = []
  for (let y = 0; y < grid.length; y += 2) {
    const top = grid[y] ?? ''
    const bottom = grid[y + 1] ?? ''
    const line: MascotLine = []
    for (let x = 0; x < width; x++) {
      const upper = colorOf(top[x] ?? EMPTY)
      const lower = colorOf(bottom[x] ?? EMPTY)
      if (!upper && !lower) {
        line.push({ char: ' ' })
      } else if (!upper) {
        line.push({ char: '▄', fg: lower })
      } else if (!lower) {
        line.push({ char: '▀', fg: upper })
      } else if (upper === lower) {
        line.push({ char: '█', fg: upper })
      } else {
        line.push({ char: '▀', fg: upper, bg: lower })
      }
    }
    lines.push(line)
  }
  return lines
}

/** The mascot's size on screen, in columns and lines. */
export const MASCOT_WIDTH = Math.max(...MASCOT_GRID.map((row) => row.length))
export const MASCOT_HEIGHT = Math.ceil(MASCOT_GRID.length / 2)

/**
 * The mascot for a glyph mode, or `null` where it is not drawn. The console
 * glyph set (a classic Windows console window, the Linux virtual console)
 * leaves it out: half blocks need a foreground and a background colour per
 * character, which those terminals cannot be relied on to show.
 */
export function mascotLinesFor(mode: GlyphMode): MascotLine[] | null {
  return mode === 'console' ? null : renderMascotLines()
}
