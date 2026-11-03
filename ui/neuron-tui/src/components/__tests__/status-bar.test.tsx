// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the
// Freebuff session and BYOK cases went with those features; a running turn
// reads as what it does (verb, target, tokens, a stalled stream) instead of
// "working...". See ui/neuron-tui/PORTING.md.
import { afterEach, beforeAll, beforeEach, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import { EngineFooter } from '../engine-footer'
import { StatusBar } from '../status-bar'
import { setLiveTurnClockForTesting } from '../../engine/live-turn'
import { initializeThemeStore } from '../../hooks/use-theme'
import { useEngineStore } from '../../state/engine-store'
import {
  INITIAL_LIVE_TURN,
  resetLiveTurnStore,
  useLiveTurnStore,
  type LiveTurnState,
} from '../../state/live-turn-store'
import {
  getStatusIndicatorState,
  type StatusIndicatorStateArgs,
} from '../../utils/status-indicator-state'

const NOW = 5_000_000

beforeAll(() => {
  initializeThemeStore()
})

beforeEach(() => {
  setLiveTurnClockForTesting(() => NOW)
  resetLiveTurnStore()
  useEngineStore.getState().reset()
})

afterEach(() => {
  setLiveTurnClockForTesting(null)
  resetLiveTurnStore()
  useEngineStore.getState().reset()
})

/** A turn that streams, as the live view holds it. */
const liveTurn = (patch: Partial<LiveTurnState>) =>
  useLiveTurnStore.setState(
    { ...INITIAL_LIVE_TURN, active: true, model: 'streaming', lastTokenAt: NOW, ...patch },
    true,
  )

async function frame(
  args: Partial<StatusIndicatorStateArgs> = {},
  options: { elapsedSeconds?: number; onStop?: () => void } = {},
): Promise<string> {
  const statusIndicatorState = getStatusIndicatorState({
    statusMessage: null,
    streamStatus: 'streaming',
    nextCtrlCWillExit: false,
    isConnected: true,
    ...args,
  })
  const setup = await createTestRenderer({ width: 120, height: 3 })
  const root = createRoot(setup.renderer)
  flushSync(() => {
    root.render(
      <StatusBar
        timerStartTime={
          options.elapsedSeconds ? Date.now() - options.elapsedSeconds * 1000 : null
        }
        isAtBottom
        scrollToLatest={() => {}}
        statusIndicatorState={statusIndicatorState}
        onStop={options.onStop}
      />,
    )
  })
  try {
    await setup.renderOnce()
    // The elapsed time is read in an effect: draw again once it has run.
    await new Promise((resolve) => setTimeout(resolve, 0))
    await setup.renderOnce()
    return setup.captureCharFrame()
  } finally {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
}

describe('StatusBar', () => {
  test('a file being written: verb, path, size, elapsed, tokens and the interrupt hint', async () => {
    liveTurn({
      calls: [
        { id: 'w', name: 'write_file', phase: 'input', target: 'grocery-shop/style.css', lines: 412, bytes: 12_595 },
      ],
      turnOutput: 800,
      streamingOutput: 400,
    })
    const shown = await frame({}, { elapsedSeconds: 112, onStop: () => {} })
    expect(shown).toContain('Writing grocery-shop/style.css · 412 lines · 12.3 KB')
    expect(shown).toContain('1m 52s · ↓ 1.2K tokens')
    expect(shown).toContain('esc to interrupt')
    expect(shown).not.toContain('working...')
  })

  test('a command that runs, however long, is not a stall', async () => {
    liveTurn({
      model: 'idle',
      lastTokenAt: NOW - 120_000,
      calls: [{ id: 'b', name: 'bash', phase: 'running', target: 'cargo test', lines: 0, bytes: 0 }],
    })
    const shown = await frame()
    expect(shown).toContain('Running cargo test')
    expect(shown).not.toContain('no tokens')
  })

  test('a stream silent for over 10 s says so, with the elapsed time and the hint', async () => {
    liveTurn({ stream: 'responding', lastTokenAt: NOW - 32_000 })
    const shown = await frame({}, { elapsedSeconds: 347, onStop: () => {} })
    expect(shown).toContain('Waiting for the model · no tokens for 32s')
    expect(shown).toContain('5m 47s')
    expect(shown).toContain('esc to interrupt')
  })

  test('a fresh stream reads as what the model writes', async () => {
    liveTurn({ stream: 'thinking', lastTokenAt: NOW - 2_000 })
    expect(await frame()).toContain('Thinking')
    liveTurn({ stream: 'responding', lastTokenAt: NOW - 2_000 })
    const shown = await frame()
    expect(shown).toContain('Responding')
    expect(shown).not.toContain('no tokens')
  })

  test('before the first token: waiting for the model', async () => {
    liveTurn({ model: 'waiting', lastTokenAt: null, requestSentAt: NOW - 3_000 })
    expect(await frame({ streamStatus: 'waiting' })).toContain('Waiting for the model')
  })

  test('the permission dialog: waiting for approval, never a stall', async () => {
    liveTurn({ stream: 'responding', lastTokenAt: NOW - 60_000 })
    useEngineStore.getState().enqueuePermission({
      type: 'permission_requested',
      id: 1,
      request: {
        tool: 'bash',
        current_mode: 'workspace-write',
        required_mode: 'danger-full-access',
        reason: null,
        input: '{"command":"rm -rf build"}',
        session_prefixes: [],
      },
    })
    const shown = await frame({ isAskUserActive: true })
    expect(shown).toContain('Waiting for approval')
    expect(shown).not.toContain('no tokens')
  })

  test('a question (not a permission) still shows nothing', async () => {
    liveTurn({})
    expect((await frame({ isAskUserActive: true })).trim()).toBe('')
  })

  test('a retry shows retrying', async () => {
    liveTurn({ lastTokenAt: NOW - 95_000 })
    const shown = await frame({ isRetrying: true })
    expect(shown).toContain('retrying...')
    expect(shown).not.toContain('no tokens')
  })

  test('an older engine: the verb, no token count until the turn ends', async () => {
    liveTurn({ stream: 'responding' })
    const shown = await frame({}, { elapsedSeconds: 4 })
    expect(shown).toContain('Responding')
    expect(shown).toContain('4s')
    expect(shown).not.toContain('↓')
  })
})

describe('EngineFooter while a turn streams', () => {
  const STATUS = {
    type: 'status' as const,
    model: 'nemotron-3-ultra',
    permission_mode: 'workspace-write',
    session_id: 's1',
    cwd: '/work',
    workspace_trusted: true,
    plan_mode: false,
    context_percent: 9,
    git: null,
  }

  async function footer(): Promise<string> {
    const setup = await createTestRenderer({ width: 120, height: 1 })
    const root = createRoot(setup.renderer)
    flushSync(() => root.render(<EngineFooter width={120} />))
    try {
      await setup.renderOnce()
      return setup.captureCharFrame()
    } finally {
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    }
  }

  test('adds the live estimate, marked ~, and settles on the real usage', async () => {
    const engine = useEngineStore.getState()
    engine.setStatus(STATUS)
    engine.setUsage({
      type: 'usage',
      turn: { input: 1, output: 1, cache_read: 0, cache_write: 0 },
      cumulative: { input: 10_000, output: 1_000, cache_read: 0, cache_write: 0 },
      estimated: false,
    })
    liveTurn({ unsettledInput: 5_000, unsettledOutput: 300, streamingOutput: 200 })
    expect(await footer()).toContain('↑~15K ↓~1.5K')

    useLiveTurnStore.setState({ unsettledInput: 0, unsettledOutput: 0, streamingOutput: 0 })
    engine.setUsage({
      type: 'usage',
      turn: { input: 5_000, output: 520, cache_read: 0, cache_write: 0 },
      cumulative: { input: 15_000, output: 1_520, cache_read: 0, cache_write: 0 },
      estimated: false,
    })
    expect(await footer()).toContain('↑15K ↓1.5K')
  })
})
