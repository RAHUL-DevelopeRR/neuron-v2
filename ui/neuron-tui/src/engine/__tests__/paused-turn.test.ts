import { beforeEach, describe, expect, test } from 'bun:test'

import { useChatStore } from '../../state/chat-store'
import { useEngineStore } from '../../state/engine-store'
import { handleEngineEvent, setEngineTurnActive } from '../engine-session'
import {
  CONTINUE_PROMPT,
  clearPausedTurn,
  hasPausedTurn,
  takeContinuePrompt,
} from '../paused-turn'

const PAUSED =
  'Paused: the turn reached its limit of 200 model requests. Your progress is saved: press Enter or type continue to keep going, or tell it what to do differently.'

const chatLines = () =>
  useChatStore
    .getState()
    .messages.filter((m) => m.variant !== 'user')
    .map((m) => m.content)

beforeEach(() => {
  useEngineStore.getState().reset()
  useChatStore.getState().reset()
  setEngineTurnActive(false)
  clearPausedTurn()
})

describe('a paused turn', () => {
  test('shows a calm chat line, not an error, and arms Enter to continue', () => {
    handleEngineEvent({ type: 'notice', message: PAUSED, kind: 'paused' })
    expect(chatLines()).toEqual([PAUSED])
    expect(chatLines().some((line) => line.startsWith('Error'))).toBe(false)
    expect(chatLines().some((line) => line.includes('NEURON_MAX_ITERATIONS'))).toBe(false)
    expect(hasPausedTurn()).toBe(true)
    // Enter on an empty prompt sends "continue", once.
    expect(takeContinuePrompt()).toBe(CONTINUE_PROMPT)
    expect(CONTINUE_PROMPT).toBe('continue')
    expect(takeContinuePrompt()).toBeNull()
  })

  test('anything else sent answers the pause', () => {
    handleEngineEvent({ type: 'notice', message: PAUSED, kind: 'paused' })
    clearPausedTurn()
    expect(takeContinuePrompt()).toBeNull()
  })

  test('a plain notice (an older engine, or no turn_pause) arms nothing', () => {
    handleEngineEvent({
      type: 'notice',
      message: 'Paused: the turn reached its limit of 200 model requests. Your progress is saved: type continue to keep going, or tell it what to do differently.',
    })
    expect(chatLines()).toHaveLength(1)
    expect(hasPausedTurn()).toBe(false)
  })
})
