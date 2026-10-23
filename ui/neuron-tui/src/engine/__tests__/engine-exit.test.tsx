import { beforeAll, beforeEach, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import { StatusBar } from '../../components/status-bar'
import { initializeThemeStore } from '../../hooks/use-theme'
import { useChatStore } from '../../state/chat-store'
import { useEngineStore } from '../../state/engine-store'
import { exitCodeAfterSession } from '../../utils/exit-cleanly'
import { getStatusIndicatorState } from '../../utils/status-indicator-state'
import {
  engineCrashedThisSession,
  handleEngineExit,
  resetEngineCrashForTesting,
  setEngineTurnActive,
} from '../engine-session'

const STOPPED = 'The Neuron engine stopped (exit code 255).'

const chatLines = () =>
  useChatStore
    .getState()
    .messages.map((message) => message.content)
    .filter((content) => content.includes('engine'))

/** The status bar's line under the chat, as the user sees it. */
async function statusLine(): Promise<string> {
  const setup = await createTestRenderer({ width: 100, height: 3 })
  const root = createRoot(setup.renderer)
  try {
    flushSync(() =>
      root.render(
        <StatusBar
          timerStartTime={null}
          isAtBottom
          scrollToLatest={() => {}}
          statusIndicatorState={getStatusIndicatorState({
            statusMessage: null,
            streamStatus: 'idle',
            nextCtrlCWillExit: false,
            isConnected: false,
          })}
        />,
      ),
    )
    await setup.renderOnce()
    return setup.captureCharFrame()
  } finally {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
}

beforeAll(() => initializeThemeStore())

beforeEach(() => {
  useEngineStore.getState().reset()
  useChatStore.getState().reset()
  setEngineTurnActive(false)
  resetEngineCrashForTesting()
})

describe('an engine that stops by itself', () => {
  test('says so once: a chat line, and the status bar only says it is not running', async () => {
    useEngineStore.getState().setReady('test')
    handleEngineExit({ code: 255, signal: null }, { lastError: null, stopRequested: false })

    expect(chatLines()).toEqual([STOPPED])
    expect(useEngineStore.getState().connection).toBe('exited')
    const shown = await statusLine()
    // Was the same "stopped (exit code 255)" line again, under the chat line.
    expect(shown).not.toContain('exit code 255')
    expect(shown).toContain('The Neuron engine is not running.')
    expect(engineCrashedThisSession()).toBe(true)
  })

  test('during a turn the turn says it, not another chat line', () => {
    useEngineStore.getState().setReady('test')
    setEngineTurnActive(true, 'ai-1')
    handleEngineExit({ code: 255, signal: null }, { lastError: null, stopRequested: false })
    expect(chatLines()).toEqual([])
    expect(engineCrashedThisSession()).toBe(true)
  })

  test('a stop the UI asked for (quitting) is no crash and no message', async () => {
    useEngineStore.getState().setReady('test')
    handleEngineExit({ code: 1, signal: null }, { lastError: null, stopRequested: true })
    expect(chatLines()).toEqual([])
    expect(engineCrashedThisSession()).toBe(false)
  })

  test('a clean exit of its own is no crash; the status bar gives the reason', async () => {
    useEngineStore.getState().setReady('test')
    handleEngineExit({ code: 0, signal: null }, { lastError: null, stopRequested: false })
    expect(chatLines()).toEqual([])
    expect(engineCrashedThisSession()).toBe(false)
    expect(await statusLine()).toContain('The Neuron engine stopped (exit code 0).')
  })

  test('a failed start is no crash of the session', () => {
    handleEngineExit({ code: 1, signal: null }, { lastError: 'bad flag', stopRequested: false })
    expect(chatLines()).toEqual([])
    expect(engineCrashedThisSession()).toBe(false)
  })

  test('after a crash the UI exits 1, also when quit normally', () => {
    expect(exitCodeAfterSession(0, true)).toBe(1)
    expect(exitCodeAfterSession(0, false)).toBe(0)
    // An exit code of its own is kept.
    expect(exitCodeAfterSession(130, true)).toBe(130)
  })
})
