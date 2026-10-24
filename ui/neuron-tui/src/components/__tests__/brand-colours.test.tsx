import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import { initializeThemeStore, useThemeStore } from '../../hooks/use-theme'
import { BRAND } from '../../utils/logo'
import { chatThemes } from '../../utils/theme-system'
import { MultilineInput } from '../multiline-input'

import type { CapturedFrame } from '@opentui/core'
import type { ThemeName } from '../../types/theme-system'

const hex = (color: { r: number; g: number; b: number }) =>
  `#${[color.r, color.g, color.b]
    .map((value) => Math.round(value * 255).toString(16).padStart(2, '0'))
    .join('')}`.toUpperCase()

async function renderInput(): Promise<CapturedFrame> {
  const setup = await createTestRenderer({ width: 40, height: 3 })
  const root = createRoot(setup.renderer)
  try {
    flushSync(() =>
      root.render(
        <MultilineInput
          value=""
          cursorPosition={0}
          onChange={() => {}}
          onSubmit={() => {}}
          onPaste={() => {}}
          shouldBlinkCursor={false}
        />,
      ),
    )
    await setup.renderOnce()
    return setup.captureSpans()
  } finally {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
}

/** The colour the input's cursor (▍) is drawn in. */
function cursorColour(frame: CapturedFrame): string | undefined {
  for (const line of frame.lines) {
    const span = line.spans.find((candidate) => candidate.text.includes('▍'))
    if (span) return hex(span.fg)
  }
  return undefined
}

let initialTheme: ThemeName = 'dark'
beforeAll(() => {
  initializeThemeStore()
  initialTheme = useThemeStore.getState().theme.name
})
afterAll(() => useThemeStore.getState().setThemeName(initialTheme))

describe("NeuronCLI's colours", () => {
  test.each(['dark', 'light'] as ThemeName[])(
    'the %s theme draws the cursor in its own colour, not lime',
    async (name) => {
      // Whatever COLORTERM says: it was lime wherever it was unset (every
      // Windows console).
      useThemeStore.getState().setThemeName(name)
      const colour = cursorColour(await renderInput())
      expect(colour).toBe(chatThemes[name].info.toUpperCase())
      expect(colour).not.toBe('#00FF00')
    },
  )

  test('borders are brand blue', () => {
    expect(chatThemes.dark.border).toBe(BRAND.blue)
    // A lighter tint of it, still 3:1 or more on white.
    expect(chatThemes.light.border).toBe('#5B7FD0')
  })
})
