import { describe, expect, test } from 'bun:test'

import { continueHint } from '../use-exit-handler'

describe('continueHint', () => {
  test('names the chat to continue once something was asked in it', () => {
    const hint = continueHint('chat-7', [{ variant: 'user' }, { variant: 'ai' }])
    expect(hint).toContain('--continue chat-7')
  })

  test('is left out for a chat nothing was asked in', () => {
    expect(continueHint('chat-7', [])).toBeNull()
    // Notices and errors at startup don't make a chat worth continuing.
    expect(continueHint('chat-7', [{ variant: 'error' }, { variant: 'ai' }])).toBeNull()
  })

  test('is left out without a chat id', () => {
    expect(continueHint(undefined, [{ variant: 'user' }])).toBeNull()
  })
})
