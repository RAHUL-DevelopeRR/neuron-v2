import { beforeEach, describe, expect, mock, test } from 'bun:test'

import { clearPausedTurn, hasPausedTurn, notePausedTurn } from '../../engine/paused-turn'
import { useChatStore } from '../../state/chat-store'
import { routeUserPrompt } from '../router'

import type { RouterParams } from '../command-registry'

function params(inputValue: string) {
  const history: string[] = []
  const sendMessage = mock((_message: { content: string }) => {})
  return {
    history,
    sendMessage,
    params: {
      agentMode: 'DEFAULT',
      inputRef: { current: null },
      inputValue,
      isChainInProgressRef: { current: false },
      isStreaming: false,
      streamMessageIdRef: { current: null },
      saveToHistory: (text: string) => history.push(text),
      setInputValue: mock(() => {}),
      setInputFocused: mock(() => {}),
      setMessages: mock(() => {}),
      sendMessage,
      addToQueue: mock(() => {}),
      scrollToLatest: () => {},
    } as unknown as RouterParams,
  }
}

const sentContents = (send: ReturnType<typeof params>['sendMessage']) =>
  send.mock.calls.map(([message]) => message.content)

beforeEach(() => {
  clearPausedTurn()
  useChatStore.getState().reset()
})

describe('Enter after a paused turn', () => {
  test('an empty prompt sends "continue", once', async () => {
    notePausedTurn()
    const first = params('   ')
    await routeUserPrompt(first.params)
    expect(sentContents(first.sendMessage)).toEqual(['continue'])
    expect(first.history).toEqual(['continue'])
    expect(hasPausedTurn()).toBe(false)

    const second = params('')
    await routeUserPrompt(second.params)
    expect(second.sendMessage).not.toHaveBeenCalled()
  })

  test('without a paused turn an empty prompt does nothing', async () => {
    const empty = params('')
    await routeUserPrompt(empty.params)
    expect(empty.sendMessage).not.toHaveBeenCalled()
  })

  test('a typed prompt is sent as typed and answers the pause', async () => {
    notePausedTurn()
    const typed = params('use the other parser instead')
    await routeUserPrompt(typed.params)
    expect(sentContents(typed.sendMessage)).toEqual(['use the other parser instead'])
    expect(hasPausedTurn()).toBe(false)
  })
})
