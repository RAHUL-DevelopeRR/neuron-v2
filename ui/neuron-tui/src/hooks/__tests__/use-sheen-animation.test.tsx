import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import { describe, expect, test } from 'bun:test'
import React, { useState } from 'react'

import { SHEEN_INTERVAL_MS, SHEEN_MAX_PHASES, SHEEN_STEP } from '../../utils/logo'
import { useSheenAnimation } from '../use-sheen-animation'

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

describe('useSheenAnimation', () => {
  test('rests after its phases instead of redrawing the logo forever', async () => {
    // A 14-column terminal: the sweep is 10 columns, 5 ticks a phase.
    const ticksPerPhase = 10 / SHEEN_STEP
    let renders = 0
    // Times the sweep went back to the start: once per finished phase.
    let resets = 0
    let lastPosition = 0
    let setEnabled: (value: boolean) => void = () => {}
    const Harness = () => {
      const [enabled, set] = useState(true)
      setEnabled = set
      const [sheenPosition, setSheenPosition] = useState(0)
      useSheenAnimation({ enabled, themeName: 'dark', terminalWidth: 14, sheenPosition, setSheenPosition })
      if (sheenPosition === 0 && lastPosition > 0) resets++
      lastPosition = sheenPosition
      renders++
      return <text>{String(sheenPosition)}</text>
    }
    const setup = await createTestRenderer({ width: 14, height: 1 })
    const root = createRoot(setup.renderer)
    try {
      flushSync(() => root.render(<Harness />))
      // Wait for the phases on the clock, not on a fixed sleep: a busy machine delays ticks.
      const deadline = Date.now() + SHEEN_INTERVAL_MS * ticksPerPhase * SHEEN_MAX_PHASES * 10
      while (resets < SHEEN_MAX_PHASES && Date.now() < deadline) await sleep(SHEEN_INTERVAL_MS)
      expect(resets).toBe(SHEEN_MAX_PHASES)
      const afterSweep = renders

      await sleep(SHEEN_INTERVAL_MS * 4)
      expect(renders).toBe(afterSweep)
      expect(resets).toBe(SHEEN_MAX_PHASES)

      // Enabled again (the header scrolled back into view): it sweeps again.
      flushSync(() => setEnabled(false))
      flushSync(() => setEnabled(true))
      await sleep(SHEEN_INTERVAL_MS * 3)
      expect(renders).toBeGreaterThan(afterSweep + 2)
    } finally {
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    }
  })
})
