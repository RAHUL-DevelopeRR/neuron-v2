import { afterAll, beforeAll, expect, mock, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import { bordersFor } from '../../utils/glyphs'
import * as uiConstants from '../../utils/ui-constants'

import type { StatusEvent } from '../../engine/protocol'

// A classic console window draws the panel with the console set's borders.
mock.module('../../utils/ui-constants', () => ({
  ...uiConstants,
  BORDER_CHARS: bordersFor('console').rounded,
}))

const { initializeThemeStore } = await import('../../hooks/use-theme')
const { useEngineStore } = await import('../../state/engine-store')
const { SignInPanel } = await import('../sign-in-panel')

beforeAll(() => {
  initializeThemeStore()
})

afterAll(() => {
  useEngineStore.getState().reset()
  mock.restore()
})

test("the Can't reach Zero-X panel in a classic console (ASCII mode) at 80x24", async () => {
  useEngineStore.getState().setStatus({
    type: 'status',
    model: 'auto',
    permission_mode: 'workspace-write',
    session_id: '',
    cwd: '/work',
    workspace_trusted: true,
    plan_mode: false,
    context_percent: 0,
    git: null,
    signed_in: false,
    sign_in_required: true,
  } as StatusEvent)
  useEngineStore.getState().setSignInProblem('Could not reach Zero-X: connection refused')
  const setup = await createTestRenderer({ width: 80, height: 24 })
  const root = createRoot(setup.renderer)
  flushSync(() => root.render(<SignInPanel width={78} />))
  try {
    await setup.renderOnce()
    const frame = setup
      .captureCharFrame()
      .split('\n')
      .map((line) => line.trimEnd())
      .filter((line) => line)
      .join('\n')
    // Square corners from the console fonts, no rounded ones.
    expect(frame).not.toMatch(/[╭╮╰╯]/)
    expect(frame).toMatchSnapshot()
  } finally {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
})
