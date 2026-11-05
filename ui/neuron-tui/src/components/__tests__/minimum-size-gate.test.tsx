import { beforeAll, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import { initializeThemeStore } from '../../hooks/use-theme'
import { cutToDisplayWidth, displayWidth, padToDisplayWidth } from '../../utils/display-width'
import { fitParts, type FooterPart } from '../engine-footer'
import {
  MIN_TERMINAL_COLUMNS,
  MIN_TERMINAL_ROWS,
  MinimumSizeGate,
  isTerminalTooSmall,
  tooSmallLines,
} from '../minimum-size-gate'

beforeAll(() => {
  initializeThemeStore()
})

async function draw(width: number, height: number): Promise<string> {
  const setup = await createTestRenderer({ width, height })
  const root = createRoot(setup.renderer)
  flushSync(() =>
    root.render(
      <MinimumSizeGate>
        <text>the chat</text>
      </MinimumSizeGate>,
    ),
  )
  try {
    await setup.renderOnce()
    return setup.captureCharFrame()
  } finally {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
}

describe('display width', () => {
  test('wide characters take two columns, combining marks none', () => {
    expect(displayWidth('main')).toBe(4)
    expect(displayWidth('功能分支')).toBe(8)
    expect(displayWidth('🚀')).toBe(2)
    expect(displayWidth('é')).toBe(1)
  })

  test('cutting counts columns and never splits a wide character', () => {
    expect(cutToDisplayWidth('feat/功能分支', 9)).toBe('feat/功…')
    expect(displayWidth(cutToDisplayWidth('feat/功能分支', 9))).toBeLessThanOrEqual(9)
    expect(cutToDisplayWidth('功能分支', 4)).toBe('功…')
    expect(cutToDisplayWidth('main', 4)).toBe('main')
    expect(cutToDisplayWidth('main', 1)).toBe('…')
    expect(cutToDisplayWidth('main', 0)).toBe('')
    expect(padToDisplayWidth('功', 4)).toBe('功  ')
  })

  test('footer parts with wide characters are fitted by their columns', () => {
    const parts: FooterPart[] = [
      { text: 'zx-fast', tone: 'normal', priority: 0 },
      { text: '⎇ 功能分支', tone: 'muted', priority: 6 },
      { text: 'Zero-X free', tone: 'muted', priority: 1 },
    ]
    // By length the branch (6) would fit in 30 columns with the rest; it takes 10.
    const fitted = fitParts(parts, 30)
    const line = fitted.map((part) => part.text).join(' · ')
    expect(displayWidth(line)).toBeLessThanOrEqual(30)
    expect(fitParts(parts, 24).map((part) => part.text)).toEqual(['zx-fast', 'Zero-X free'])
  })
})

describe('the minimum terminal size', () => {
  test('below 60x16 the terminal is too small', () => {
    expect(isTerminalTooSmall(MIN_TERMINAL_COLUMNS, MIN_TERMINAL_ROWS)).toBe(false)
    expect(isTerminalTooSmall(MIN_TERMINAL_COLUMNS - 1, 40)).toBe(true)
    expect(isTerminalTooSmall(200, MIN_TERMINAL_ROWS - 1)).toBe(true)
  })

  test('the screen says the size and fits a tiny terminal', () => {
    expect(tooSmallLines(40, 10)).toEqual([
      'Terminal too small',
      '40x10, needs 60x16',
      'Make the window bigger,',
      'or run neuron --no-tui.',
    ])
    const tiny = tooSmallLines(12, 2)
    expect(tiny).toHaveLength(2)
    for (const line of tiny) expect(displayWidth(line)).toBeLessThanOrEqual(11)
  })

  test('a small terminal shows the notice instead of the chat; a big one the chat', async () => {
    const small = await draw(40, 10)
    expect(small).toContain('Terminal too small')
    expect(small).toContain('40x10, needs 60x16')
    expect(small).not.toContain('the chat')

    const big = await draw(80, 24)
    expect(big).toContain('the chat')
    expect(big).not.toContain('Terminal too small')
  })
})
