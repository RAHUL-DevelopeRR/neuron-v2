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
    // A prompt after them, once the agent answered it, is.
    expect(
      continueHint('chat-7', [
        { variant: 'user', content: '/status' },
        { variant: 'user', content: 'Is 221 prime?' },
        {
          variant: 'ai',
          id: 'ai-1',
          content: '',
          blocks: [{ type: 'text', content: 'No: 13 × 17.' }],
        },
      ]),
    ).toContain('--continue chat-7')
  })

  test('a prompt the engine refused (signed out) is not something asked', () => {
    // The turn's message holds only the error; the engine saved nothing.
    const refused = [
      { variant: 'user' as const, content: 'fix the login bug' },
      { variant: 'ai' as const, id: 'ai-1', content: '', blocks: [] },
      { variant: 'ai' as const, id: 'sys-1', content: 'Error: Sign in to Zero-X first: /login.' },
    ]
    expect(continueHint('chat-7', refused)).toBeNull()
    // Signed in, the next prompt is answered: a tool call is an answer too.
    expect(
      continueHint('chat-7', [
        ...refused,
        { variant: 'user', content: '/login' },
        { variant: 'user', content: 'fix the login bug' },
        {
          variant: 'ai',
          id: 'ai-2',
          content: '',
          blocks: [{ type: 'tool', toolCallId: 't1', toolName: 'read_files', input: {} }],
        },
      ]),
    ).toContain('--continue chat-7')
  })

  test('is left out without a chat id', () => {
    expect(
      continueHint(undefined, [
        { variant: 'user', content: 'hi' },
        { variant: 'ai', content: 'Hello.' },
      ]),
    ).toBeNull()
  })
})
