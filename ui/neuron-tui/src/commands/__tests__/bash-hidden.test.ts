import { describe, expect, mock, test } from 'bun:test'

import { helpSections } from '../../components/help-banner'
import { SLASH_COMMANDS } from '../../data/slash-commands'
import { useChatStore } from '../../state/chat-store'
import { USER_SHELL_AVAILABLE } from '../../utils/user-shell'
import { routeUserPrompt } from '../router'

import type { RouterParams } from '../command-registry'

const params = (inputValue: string): RouterParams =>
  ({
    agentMode: 'DEFAULT',
    inputRef: { current: null },
    inputValue,
    isChainInProgressRef: { current: false },
    isStreaming: false,
    streamMessageIdRef: { current: null },
    addToQueue: mock(() => {}),
    hasQueuedMessages: () => false,
    clearMessages: mock(() => {}),
    saveToHistory: mock(() => {}),
    scrollToLatest: mock(() => {}),
    sendMessage: mock(async () => {}),
    setCanProcessQueue: mock(() => {}),
    setInputFocused: mock(() => {}),
    setInputValue: mock(() => {}),
    setMessages: mock(() => {}),
  }) as RouterParams

const helpRows = (platform: NodeJS.Platform) =>
  helpSections(platform).flatMap((section) => section.rows.map(([key]) => key))

describe('shell commands are not offered while they do not work', () => {
  test('shell commands are not implemented in this build', () => {
    expect(USER_SHELL_AVAILABLE).toBe(false)
  })

  test('the slash menu has no /bash', () => {
    expect(SLASH_COMMANDS.some((command) => command.id === 'bash')).toBe(false)
    expect(SLASH_COMMANDS.some((command) => command.aliases?.includes('!'))).toBe(false)
  })

  test('/help does not list /bash', () => {
    expect(helpRows('win32')).not.toContain('/bash')
    expect(helpRows('linux')).not.toContain('/bash')
    // It comes back with them.
    expect(
      helpSections('linux', true).flatMap((section) => section.rows.map(([key]) => key)),
    ).toContain('/bash')
  })

  test('a "!" prompt goes to the agent, not to a shell that fails', async () => {
    const route = params('!ls -la')
    await routeUserPrompt(route)
    expect(route.sendMessage).toHaveBeenCalledTimes(1)
    expect((route.sendMessage as any).mock.calls[0][0].content).toBe('!ls -la')
    expect(useChatStore.getState().pendingBashMessages).toEqual([])
    expect(useChatStore.getState().inputMode).toBe('default')
  })
})

describe('/help says the keys of this keyboard', () => {
  test('Alt+Enter on Windows and Linux, Opt+Enter on macOS', () => {
    expect(helpRows('win32')).toContain('Alt+Enter')
    expect(helpRows('win32')).not.toContain('Opt+Enter')
    expect(helpRows('linux')).toContain('Alt+Enter')
    expect(helpRows('darwin')).toContain('Opt+Enter')
  })
})
