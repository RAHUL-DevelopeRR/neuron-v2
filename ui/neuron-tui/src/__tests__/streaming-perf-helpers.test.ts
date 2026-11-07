import { afterEach, describe, expect, test } from 'bun:test'
import { isDraft } from 'immer'

import { liveActivityInput } from '../components/live-activity'
import { sameStatusBarProps, type StatusBarProps } from '../components/status-bar'
import { useChatStore } from '../state/chat-store'
import { INITIAL_LIVE_TURN } from '../state/live-turn-store'
import { DEFAULT_FLUSH_INTERVAL_MS } from '../utils/message-updater'
import { buildMessageTree, reuseMessageTree } from '../utils/message-tree-utils'

import type { ChatMessage } from '../types/chat'

const message = (id: string, parentId?: string): ChatMessage => ({
  id,
  variant: parentId ? 'agent' : 'ai',
  content: id,
  timestamp: 'now',
  ...(parentId && { parentId }),
})

afterEach(() => {
  useChatStore.getState().setMessages([])
})

describe('the message tree a streamed update builds', () => {
  const root = message('root')
  const child = message('child', 'root')

  test('is the same object when no sub-agent message changed', () => {
    const before = buildMessageTree([root, child]).tree
    // The streaming top-level message changed: the children did not.
    const after = reuseMessageTree(before, buildMessageTree([{ ...root, content: 'more' }, child]).tree)
    expect(after).toBe(before)
  })

  test('is new when a child changed, keeping the unchanged lists', () => {
    const other = message('other-child', 'other')
    const before = buildMessageTree([root, child, other]).tree
    const changed = { ...child, content: 'more' }
    const after = reuseMessageTree(before, buildMessageTree([root, changed, other]).tree)
    expect(after).not.toBe(before)
    expect(after.get('root')).toEqual([changed])
    expect(after.get('other')).toBe(before.get('other'))
  })

  test('is new when a parent gains or loses children', () => {
    const before = buildMessageTree([root]).tree
    expect(reuseMessageTree(before, buildMessageTree([root, child]).tree)).not.toBe(before)
    expect(reuseMessageTree(null, before)).toBe(before)
  })
})

describe('what the status line reads of the live turn', () => {
  test('moves with the second of the last token, not every token', () => {
    const at = (lastTokenAt: number | null) => liveActivityInput({ ...INITIAL_LIVE_TURN, lastTokenAt })
    expect(at(10_001).lastTokenAt).toBe(11_000)
    expect(at(10_999).lastTokenAt).toBe(11_000)
    expect(at(11_000).lastTokenAt).toBe(11_000)
    expect(at(null).lastTokenAt).toBeNull()
    const exact = { ...INITIAL_LIVE_TURN, lastTokenAt: 12_000 }
    expect(liveActivityInput(exact)).toBe(exact)
  })
})

describe('the status bar under a chat that draws often', () => {
  const props = (kind: 'streaming' | 'idle' | 'clipboard', message?: string): StatusBarProps => ({
    timerStartTime: 1,
    isAtBottom: true,
    scrollToLatest: noop,
    statusIndicatorState: (kind === 'clipboard' ? { kind, message: message ?? '' } : { kind }) as StatusBarProps['statusIndicatorState'],
    onStop: noop,
  })
  function noop() {}

  test('is not drawn again for an equal state in a new object', () => {
    expect(sameStatusBarProps(props('streaming'), props('streaming'))).toBe(true)
    expect(sameStatusBarProps(props('clipboard', 'Copied'), props('clipboard', 'Copied'))).toBe(true)
  })

  test('is drawn again when the state or another prop changes', () => {
    expect(sameStatusBarProps(props('streaming'), props('idle'))).toBe(false)
    expect(sameStatusBarProps(props('clipboard', 'Copied'), props('clipboard', 'Pasted'))).toBe(false)
    expect(sameStatusBarProps(props('streaming'), { ...props('streaming'), isAtBottom: false })).toBe(false)
    expect(sameStatusBarProps(props('streaming'), { ...props('streaming'), onStop: () => {} })).toBe(false)
  })
})

describe('the chat store message updates', () => {
  test('hand updaters the messages themselves, not drafts', () => {
    const first = message('a')
    useChatStore.getState().setMessages([first])
    const stored = useChatStore.getState().messages[0]
    let seen: ChatMessage[] = []
    useChatStore.getState().setMessages((previous) => {
      seen = previous
      return previous.map((entry) => (entry.id === 'a' ? { ...entry, content: 'b' } : entry))
    })
    expect(isDraft(seen)).toBe(false)
    expect(seen[0]).toBe(stored)
    expect(useChatStore.getState().messages[0]!.content).toBe('b')
  })

  test('keep the state when an updater changes nothing', () => {
    useChatStore.getState().setMessages([message('a')])
    const before = useChatStore.getState()
    useChatStore.getState().setMessages((previous) => previous)
    expect(useChatStore.getState()).toBe(before)
  })
})

test('streamed text is drawn about 30 times a second', () => {
  expect(DEFAULT_FLUSH_INTERVAL_MS).toBeLessThanOrEqual(34)
  expect(DEFAULT_FLUSH_INTERVAL_MS).toBeGreaterThanOrEqual(16)
})
