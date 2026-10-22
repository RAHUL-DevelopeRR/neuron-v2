import { describe, expect, test } from 'bun:test'

import { continueHint } from '../use-exit-handler'

describe('continueHint', () => {
  test('names the chat to continue once something was asked in it', () => {
    const hint = continueHint('chat-7', [
      { variant: 'user', content: 'fix the login bug' },
      { variant: 'ai', content: 'Done.' },
    ])
    expect(hint).toContain('--continue chat-7')
  })

  test('is left out for a chat nothing was asked in', () => {
    expect(continueHint('chat-7', [])).toBeNull()
    // Notices and errors at startup don't make a chat worth continuing.
    expect(
      continueHint('chat-7', [
        { variant: 'error', content: 'The Neuron engine stopped.' },
        { variant: 'ai', content: 'Compacted 12 messages.' },
      ]),
    ).toBeNull()
  })

  test('slash commands alone are not something asked', () => {
    // `/status` and `/model x` are stored as user messages, but the engine's
    // session has no messages, so `--continue` would reopen an empty chat.
    expect(
      continueHint('chat-7', [
        { variant: 'user', content: '/status' },
        { variant: 'ai', content: 'Status\n  Model  auto' },
        { variant: 'user', content: '/model nvidia/openai/gpt-oss-20b' },
        { variant: 'ai', content: 'Model updated' },
      ]),
    ).toBeNull()
    // A prompt after them is.
    expect(
      continueHint('chat-7', [
        { variant: 'user', content: '/status' },
        { variant: 'user', content: 'Is 221 prime?' },
      ]),
    ).toContain('--continue chat-7')
  })

  test('is left out without a chat id', () => {
    expect(continueHint(undefined, [{ variant: 'user', content: 'hi' }])).toBeNull()
  })
})
