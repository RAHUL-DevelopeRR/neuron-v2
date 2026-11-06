// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: Freebuff's
// cli/src/components/progress-bar.tsx (a block-character bar coloured by how
// much is left) became a meter of circles coloured by how full the context
// window is, with partly filled cells (◔◑◕), a marker for the auto-compaction
// threshold, and a console-font fallback. See ui/neuron-tui/PORTING.md.
import React from 'react'

import { useTheme } from '../hooks/use-theme'
import { getGlyphMode, type GlyphMode } from '../utils/glyphs'

/** The theme colours the meter and its percentage are drawn in. */
export type MeterTone = 'success' | 'foreground' | 'warning' | 'error' | 'muted' | 'border'

export interface MeterCell {
  char: string
  tone: MeterTone
}

/** Where the engine starts auto-compaction, as a share of the usable window. */
export const DEFAULT_COMPACT_THRESHOLD_PERCENT = 80

/** `value` as a 0–100 fill: NaN and negatives are empty, anything over 100 is full. */
export function clampPercent(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 0
  return Math.min(100, value)
}

/**
 * The colour of a fill: green while there is room, the normal text colour
 * from half full, warning from where auto-compaction starts (80%), and error
 * from 95%.
 */
export function meterTone(percent: number): MeterTone {
  const fill = clampPercent(percent)
  if (fill >= 95) return 'error'
  if (fill >= 80) return 'warning'
  if (fill >= 50) return 'foreground'
  return 'success'
}

/** The percentage as shown: rounded, but never "0%" while anything is used. */
export function displayPercent(percent: number): number {
  const fill = clampPercent(percent)
  if (fill === 0) return 0
  return Math.max(1, Math.round(fill))
}

const UNICODE_PARTS = { full: '●', empty: '○', quarter: '◔', half: '◑', threeQuarters: '◕' }

/**
 * The meter's cells for a fill of `percent` (0–100): each is worth
 * 100/`cells` percent, and is full (●), empty (○), or partly used (◔ up to
 * 37.5%, ◑ up to 62.5%, ◕ below 100%). A console font has only ● and ○, so
 * there each cell is rounded to one of them. Filled cells take the fill's
 * tone; empty ones are muted, except the cell where auto-compaction starts.
 */
export function meterCells(
  percent: number,
  cells: number,
  mode: GlyphMode,
  thresholdPercent: number = DEFAULT_COMPACT_THRESHOLD_PERCENT,
): MeterCell[] {
  if (cells <= 0) return []
  const fill = clampPercent(percent)
  const tone = meterTone(fill)
  const width = 100 / cells
  const thresholdCell = Math.min(cells - 1, Math.floor(clampPercent(thresholdPercent) / width))
  const result: MeterCell[] = []
  for (let index = 0; index < cells; index++) {
    const share = Math.max(0, Math.min(1, (fill - index * width) / width))
    let char: string
    if (mode === 'console') {
      char = share >= 0.5 ? UNICODE_PARTS.full : UNICODE_PARTS.empty
    } else if (share >= 1) {
      char = UNICODE_PARTS.full
    } else if (share <= 0) {
      char = UNICODE_PARTS.empty
    } else if (share <= 0.375) {
      char = UNICODE_PARTS.quarter
    } else if (share <= 0.625) {
      char = UNICODE_PARTS.half
    } else {
      char = UNICODE_PARTS.threeQuarters
    }
    const empty = char === UNICODE_PARTS.empty
    result.push({
      char,
      tone: empty ? (index === thresholdCell ? 'border' : 'muted') : tone,
    })
  }
  return result
}

/** The meter and its percentage, for the context popover. */
export const ContextMeter = ({
  percent,
  cells = 10,
  thresholdPercent,
  estimated = false,
}: {
  percent: number
  cells?: number
  thresholdPercent?: number
  /** The engine only sent its old estimate: the percentage reads "~35%". */
  estimated?: boolean
}) => {
  const theme = useTheme()
  const parts = meterCells(percent, cells, getGlyphMode(), thresholdPercent)
  const tone = meterTone(percent)
  return (
    <text style={{ wrapMode: 'none', flexShrink: 0 }}>
      {parts.map((cell, index) => (
        <span key={index} fg={theme[cell.tone]}>
          {cell.char}
        </span>
      ))}
      <span fg={theme[tone]}>{` ${estimated ? '~' : ''}${displayPercent(percent)}%`}</span>
    </text>
  )
}
