import { beforeAll, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import { initializeThemeStore } from '../../hooks/use-theme'
import { BRAND } from '../../utils/logo'
import { ChatHeader } from '../chat-header'

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

/** The hex colour of each block character on the frame's first logo line, left to right. */
function blockColors(frame: CapturedFrame): string[] {
  const line = frame.lines.find((candidate) => candidate.spans.some((span) => span.text.includes('█')))
  const hex = (value: number) => Math.round(value * 255).toString(16).padStart(2, '0')
  return (line?.spans ?? [])
    .filter((span) => span.text.includes('█'))
    .map((span) => `#${hex(span.fg.r)}${hex(span.fg.g)}${hex(span.fg.b)}`.toUpperCase())
}

describe('ChatHeader logo', () => {
  test('the block logo draws its letters in NeuronCLI\'s colours', async () => {
    const colors = [...new Set(blockColors(await capture(90)))]
    expect(colors).toEqual([BRAND.blue, BRAND.red, BRAND.orange, BRAND.green])
  })

  test('the narrow logo is the blue N', async () => {
    expect([...new Set(blockColors(await capture(40)))]).toEqual([BRAND.blue])
  })
})
