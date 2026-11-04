import { beforeAll, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import { HelpBanner, helpContentRows, helpSections } from '../help-banner'
import { initializeThemeStore } from '../../hooks/use-theme'

beforeAll(initializeThemeStore)

test.each([120, 42])('help lays out readable shortcut groups at %s columns', async (width) => {
  const setup = await createTestRenderer({ width, height: 60 })
  const root = createRoot(setup.renderer)
  try {
    flushSync(() => root.render(<HelpBanner />))
    await setup.renderOnce()
    const frame = setup.captureCharFrame()
    const headings = frame.split('\n').find((line) => line.includes('Compose'))!
    if (width === 120) {
      expect(headings).toContain('Chat')
      expect(headings).toContain('Conversation')
    }
    expect(frame).toContain('History (empty input)')
    expect(frame).toContain('Export chat')
    expect(frame).toContain('/skills')
    expect(frame).toContain('List and manage plugins')
  } finally {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
})

test('the help is as tall as its longest group in each row of groups', () => {
  const sections = helpSections('linux')
  const tallest = Math.max(...sections.map((section) => section.rows.length + 1))
  expect(helpContentRows(sections, sections.length)).toBe(tallest)
  const stacked = sections.reduce((sum, section) => sum + section.rows.length + 1, 0)
  expect(helpContentRows(sections, 1)).toBe(stacked + sections.length - 1)
})
