import { afterEach, beforeAll, beforeEach, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import { ContextPopover } from '../context-popover'
import { PromptStatusBar } from '../prompt-status-bar'
import { initializeThemeStore } from '../../hooks/use-theme'
import { useEngineStore } from '../../state/engine-store'
import { resetLiveTurnStore } from '../../state/live-turn-store'
import {
  STATUS_BAR_ACCOUNT,
  STATUS_BAR_MODELS,
  STATUS_BAR_STATUS,
} from '../../testing/status-bar-fixtures'

import type { GlyphMode } from '../../utils/glyphs'

beforeAll(() => {
  initializeThemeStore()
})

beforeEach(() => {
  resetLiveTurnStore()
  const engine = useEngineStore.getState()
  engine.reset()
  engine.setStatus(STATUS_BAR_STATUS)
  engine.setModels(STATUS_BAR_MODELS)
  engine.setAccount(STATUS_BAR_ACCOUNT)
})

afterEach(() => {
  useEngineStore.getState().reset()
})

async function draw(element: React.ReactNode, width: number, height: number): Promise<string> {
  const setup = await createTestRenderer({ width, height })
  const root = createRoot(setup.renderer)
  flushSync(() => root.render(element))
  try {
    await setup.renderOnce()
    return setup
      .captureCharFrame()
      .split('\n')
      .map((line) => line.trimEnd())
      .join('\n')
      .trimEnd()
  } finally {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
}

/** Three hours before the fixture's token window resets. */
const NOW = Date.parse('2026-11-03T11:22:00Z')

describe('PromptStatusBar', () => {
  for (const mode of ['unicode', 'console'] as GlyphMode[]) {
    for (const width of [80, 120, 160]) {
      test(`${width} columns, ${mode} glyphs`, async () => {
        const frame = await draw(
          <PromptStatusBar width={width} glyphMode={mode} now={NOW} timeZone="UTC" />,
          width,
          1,
        )
        expect(frame).toMatchSnapshot()
        expect(frame).toContain(mode === 'unicode' ? '✎ Edit' : '+ Edit')
        expect(frame).toContain('35%')
        expect(frame).not.toMatch(/claude|anthropic/i)
      })
    }
  }

  test('nothing before the first status', async () => {
    useEngineStore.getState().reset()
    expect(await draw(<PromptStatusBar width={80} />, 80, 1)).toBe('')
  })
})

describe('ContextPopover', () => {
  const now = Date.parse('2026-11-03T11:22:00Z')

  test('the breakdown and the account', async () => {
    const frame = await draw(<ContextPopover onClose={() => {}} width={100} now={now} />, 100, 20)
    expect(frame).toContain('Context · Gemini 2.5 Flash · 1M window')
    expect(frame).toContain('System prompt')
    expect(frame).toContain('Tools (23)')
    expect(frame).toContain('Free')
    expect(frame).toContain('Auto-compacts at 80%')
    expect(frame).toContain('Account · Free (dev@example.com)')
    expect(frame).toContain('5-hour window')
    expect(frame).toContain('37 / 100 requests')
    // The reset times are local; the rest is stable.
    expect(frame.replace(/resets [^\n]*/g, 'resets …')).toMatchSnapshot()
  })

  test('a short terminal keeps the border, esc close and the status bar on screen', async () => {
    // 48x14: a typical embedded IDE terminal. The chat, the popover, the input, the bar.
    const frame = await draw(
      <box style={{ flexDirection: 'column', width: 48, height: 14 }}>
        <box style={{ flexGrow: 1, flexShrink: 1, minHeight: 0 }}>
          <text>chat line</text>
        </box>
        <ContextPopover onClose={() => {}} width={48} now={now} compact maxRows={5} />
        <text style={{ flexShrink: 0 }}>{'> input'}</text>
        <PromptStatusBar width={48} />
      </box>,
      48,
      14,
    )
    const lines = frame.split('\n')
    expect(lines.length).toBeLessThanOrEqual(14)
    expect(frame).toContain('esc close')
    expect(frame).toContain('35%')
    expect(frame).toContain('Free')
    expect(frame).toContain('Account · Free')
    expect(frame).not.toContain('System prompt')
    // The bottom border is drawn, under the help line.
    const help = lines.findIndex((line) => line.includes('esc close'))
    expect(lines[help + 1]).toMatch(/^[└╰+]/)
    expect(lines[lines.length - 1]).toContain('35%')
  })

  test('a row cap folds the rest into a pointer at /context', async () => {
    const frame = await draw(<ContextPopover onClose={() => {}} width={100} now={now} maxRows={6} />, 100, 20)
    expect(frame).toContain('more lines: /context')
    expect(frame).toContain('esc close')
    expect(frame).not.toContain('5-hour window')
  })

  test('without an account or a breakdown', async () => {
    const engine = useEngineStore.getState()
    engine.setAccount(null)
    engine.setStatus({ ...STATUS_BAR_STATUS, context: undefined, signed_in: false, account: null })
    const frame = await draw(<ContextPopover onClose={() => {}} width={70} now={now} />, 70, 12)
    expect(frame).toContain('Conversation')
    expect(frame).toContain('~')
    expect(frame).not.toContain('Account')
    expect(frame).toMatchSnapshot()
  })
})
