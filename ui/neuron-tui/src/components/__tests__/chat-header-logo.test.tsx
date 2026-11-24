import { beforeAll, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import { initializeThemeStore } from '../../hooks/use-theme'
import { BRAND, LOGO, parseLogoLines } from '../../utils/logo'
import { MASCOT_COLORS, MASCOT_HEIGHT } from '../../utils/mascot'
import { ChatHeader, showsMascot } from '../chat-header'

import type { CapturedFrame } from '@opentui/core'

beforeAll(() => {
  initializeThemeStore()
})

async function capture(width: number): Promise<CapturedFrame> {
  const setup = await createTestRenderer({ width, height: 16 })
  const root = createRoot(setup.renderer)
  flushSync(() => {
    root.render(<ChatHeader projectRoot="/tmp/project" animationEnabled={false} />)
  })
  try {
    await setup.renderOnce()
    return setup.captureSpans()
  } finally {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
}

const hex = (value: number) => Math.round(value * 255).toString(16).padStart(2, '0')
const spanFg = (span: CapturedFrame['lines'][number]['spans'][number]) =>
  `#${hex(span.fg.r)}${hex(span.fg.g)}${hex(span.fg.b)}`.toUpperCase()
const mascotColors = new Set(Object.values(MASCOT_COLORS).map((color) => color.toUpperCase()))

/**
 * The hex colour of each block character on the frame's first logo line,
 * left to right; the mascot's blocks beside it are not the logo's.
 */
function blockColors(frame: CapturedFrame): string[] {
  const line = frame.lines.find((candidate) => candidate.spans.some((span) => span.text.includes('█')))
  return (line?.spans ?? [])
    .filter((span) => span.text.includes('█'))
    .map(spanFg)
    .filter((color) => !mascotColors.has(color))
}

const frameText = (frame: CapturedFrame) =>
  frame.lines.map((line) => line.spans.map((span) => span.text).join(''))

type Span = CapturedFrame['lines'][number]['spans'][number]

/** Whether a span draws the mascot: its half blocks, or a full block in its colours. */
const isMascotSpan = (span: Span) =>
  /[▀▄]/.test(span.text) || (span.text.includes('█') && mascotColors.has(spanFg(span)))

/** The columns a line's mascot spans cover. */
function mascotColumns(line: CapturedFrame['lines'][number]): number[] {
  const columns: number[] = []
  let column = 0
  for (const span of line.spans) {
    if (isMascotSpan(span)) {
      for (let offset = 0; offset < span.width; offset++) columns.push(column + offset)
    }
    column += span.width
  }
  return columns
}

/** The frame's lines that draw the mascot. */
const mascotRows = (frame: CapturedFrame) => frame.lines.filter((line) => line.spans.some(isMascotSpan))

describe('ChatHeader logo', () => {
  test('the block logo draws its letters in NeuronCLI\'s colours', async () => {
    const colors = [...new Set(blockColors(await capture(90)))]
    expect(colors).toEqual([BRAND.blue, BRAND.red, BRAND.orange, BRAND.green])
  })

  test('the narrow logo is the blue N', async () => {
    expect([...new Set(blockColors(await capture(40)))]).toEqual([BRAND.blue])
  })
})

describe('ChatHeader mascot', () => {
  const logoLines = parseLogoLines(LOGO)

  test('a wide terminal shows Neura to the left of the full logo, centred against it', async () => {
    const frame = await capture(90)
    const rows = mascotRows(frame)
    expect(rows.length).toBe(MASCOT_HEIGHT)
    const text = frameText(frame)
    const firstLogoRow = text.findIndex((line) => line.includes(logoLines[0].trim()))
    const firstMascotRow = frame.lines.indexOf(rows[0])
    // Seven mascot lines against six logo lines: they start on the same
    // line, so the logo is within half a line of centred.
    expect(firstLogoRow).toBe(firstMascotRow)
    const logoStart = text[firstLogoRow].indexOf(logoLines[0].trim())
    for (const row of rows) {
      expect(Math.max(...mascotColumns(row))).toBeLessThan(logoStart)
    }
    // The full logo, unchanged, in its letters' colours.
    logoLines.forEach((logoLine, index) => {
      expect(text[firstLogoRow + index]).toContain(logoLine.trim())
    })
    expect([...new Set(blockColors(frame))]).toEqual([BRAND.blue, BRAND.red, BRAND.orange, BRAND.green])
  })

  test('the mascot paints no background of its own around its shape', async () => {
    const rows = mascotRows(await capture(90))
    const spanBg = (span: Span) =>
      `#${hex(span.bg.r)}${hex(span.bg.g)}${hex(span.bg.b)}`.toUpperCase()
    // Neura has cells of two colours stacked (the eyes, the highlight).
    expect(rows.some((row) => row.spans.some((span) => span.bg.a !== 0))).toBe(true)
    for (const row of rows) {
      for (const span of row.spans) {
        if (span.text.trim() === '' || /[▄█]/.test(span.text)) {
          // Blank cells and bottom halves leave the theme's background.
          expect(span.bg.a).toBe(0)
        } else if (span.bg.a !== 0) {
          // Only a top half over another colour paints one: that colour.
          expect(span.text).toMatch(/^▀+$/)
          expect(mascotColors.has(spanBg(span))).toBe(true)
        }
      }
    }
  })

  test('a narrow terminal keeps the logo alone, as before', async () => {
    for (const width of [40, 60, 70, 79]) {
      const frame = await capture(width)
      expect(mascotRows(frame)).toEqual([])
    }
    expect(showsMascot(79, 75)).toBe(false)
    expect(showsMascot(80, 76)).toBe(true)
  })
})
