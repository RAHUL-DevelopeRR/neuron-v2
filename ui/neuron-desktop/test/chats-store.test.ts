/**
 * The window's chat store: notification dedupe, stepping over stopped chats and
 * closing chats that have a copy of their own.
 */
import { describe, expect, test } from 'bun:test'

import type { UiEvent } from '@neuron/client'

import { ChatsStore } from '../src/chats'
import { ChatError } from '../src/platform'
import { FakeChats, FakeEngine } from './fake-platform'

const permission = (id: number): UiEvent => ({
  type: 'permission_requested',
  id,
  request: { tool: 'bash', current_mode: 'workspace-write', required_mode: 'danger-full-access', reason: 'it deletes files', input: '{"command":"rm -rf target"}', session_prefixes: ['rm'] },
})

async function setup(focused = true) {
  const host = new FakeChats(new FakeEngine())
  const notes: Array<{ chat: string; body: string }> = []
  const toasts: string[] = []
  let inFocus = focused
  const store = new ChatsStore({
    host,
    notify: async (note) => {
      notes.push(note)
      return 'shown'
    },
    setAttention: () => {},
    onToast: (message) => toasts.push(message),
    isFocused: () => inFocus,
  })
  const first = await store.open({})
  const second = await store.open({})
  return { host, store, notes, toasts, first, second, focus: (value: boolean) => (inFocus = value) }
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 5))

describe('notifications', () => {
  test('a chat that stops needing you may be told again for the same request number', async () => {
    const { host, store, notes, first, second } = await setup()
    store.setActive(first.id)
    host.engines.get(second.id)!.emit(permission(5))
    await settle()
    expect(notes).toHaveLength(1)
    // Answered, and later a new request that happens to have the same number.
    host.controllers.get(second.id)!.answerPermission(5, { kind: 'allow_once' })
    await settle()
    host.engines.get(second.id)!.emit(permission(5))
    await settle()
    expect(notes).toHaveLength(2)
  })

  test('a restarted engine (a new generation) is told again for the same request number', async () => {
    const { host, store, notes, first, second } = await setup()
    store.setActive(first.id)
    host.engines.get(second.id)!.emit(permission(1))
    await settle()
    expect(notes).toHaveLength(1)
    // The engine goes and comes back with a request of the same number.
    host.emitStopped(second.id, 'exited')
    await store.resume(second.id)
    host.engines.get(second.id)!.emit(permission(1))
    await settle()
    expect(notes).toHaveLength(2)
  })

  test('the same request is told once', async () => {
    const { host, store, notes, first, second } = await setup()
    store.setActive(first.id)
    host.engines.get(second.id)!.emit(permission(3))
    host.engines.get(second.id)!.emit(permission(3))
    await settle()
    expect(notes).toHaveLength(1)
  })

  test('a request seen in view does not use up its notification', async () => {
    const { host, store, notes, first, second } = await setup()
    // The second chat is in view and the window has the focus.
    store.setActive(second.id)
    host.engines.get(second.id)!.emit(permission(9))
    await settle()
    expect(notes).toEqual([])
    // The user goes to the other chat without answering: now it is news.
    store.setActive(first.id)
    await settle()
    expect(notes).toHaveLength(1)
    expect(notes[0].chat).toBe(second.id)
  })
})

describe('stepping through chats', () => {
  test('Ctrl+Alt+Up or Down passes over a stopped chat without starting it', async () => {
    const { host, store, first, second } = await setup()
    store.setActive(first.id)
    host.emitStopped(second.id, 'idle')
    await settle()
    expect(store.step(1)).toBe(second.id)
    await settle()
    expect(host.resumed).toEqual([])
    expect(store.getSnapshot().activeId).toBe(second.id)
  })

  test('an explicit choice of the stopped chat starts it again', async () => {
    const { host, store, first, second } = await setup()
    store.setActive(first.id)
    host.emitStopped(second.id, 'idle')
    await settle()
    store.setActive(second.id)
    await settle()
    expect(host.resumed.map((entry) => entry.id)).toEqual([second.id])
  })
})

describe('closing a chat with a copy of its own', () => {
  test('is refused with has_worktree and the chat stays', async () => {
    const { host, store, second } = await setup()
    host.closeNeedsDiscard.add(second.id)
    const error = (await store.close(second.id).catch((failure: unknown) => failure)) as ChatError
    expect(error).toBeInstanceOf(ChatError)
    expect(error.code).toBe('has_worktree')
    expect(store.getSnapshot().items.map((item) => item.id)).toContain(second.id)
  })

  test('an ordinary close still removes it', async () => {
    const { store, second } = await setup()
    await store.close(second.id)
    expect(store.getSnapshot().items.map((item) => item.id)).not.toContain(second.id)
  })
})

describe('the digest of each chat', () => {
  const rowOf = (store: ChatsStore, id: string) => store.getSnapshot().items.find((item) => item.id === id)!

  test('a row carries its steps and a one-line status', async () => {
    const { host, store, first, second } = await setup()
    store.setActive(first.id)
    const engine = host.engines.get(second.id)!
    host.controllers.get(second.id)!.submit('fix the tests')
    engine.emit({
      type: 'todos',
      items: [
        { content: 'Read the failure', status: 'completed', active_form: null },
        { content: 'Fix it', status: 'in_progress', active_form: 'Fixing it' },
        { content: 'Run the tests', status: 'pending', active_form: null },
      ],
    })
    await settle()
    expect(rowOf(store, second.id).steps).toEqual({ done: 1, total: 3 })
    expect(rowOf(store, second.id).status).toBe('Fixing it')
    engine.emit({ type: 'tool_call_started', id: 't1', name: 'bash', input: '{"command":"bun test"}', cut_off: false })
    await settle()
    expect(rowOf(store, second.id).status).toMatch(/bun test/)
  })

  test('a turn that ended in an error is failed, and a new prompt clears it', async () => {
    const { host, store, first, second } = await setup()
    store.setActive(first.id)
    const engine = host.engines.get(second.id)!
    host.controllers.get(second.id)!.submit('go')
    engine.emit({ type: 'error', message: 'boom' })
    engine.emit({ type: 'turn_finished' })
    await settle()
    expect(rowOf(store, second.id).failed).toBe(true)
    expect(rowOf(store, second.id).status).toBe('Turn failed')
    // The taskbar count and the notifications stay for requests only.
    expect(store.getSnapshot().needsYouCount).toBe(0)
    // Ctrl+J goes to a chat that waits on a request, not to a failed one.
    expect(store.jumpToWaiting()).toBeNull()
    host.controllers.get(second.id)!.submit('again')
    await settle()
    expect(rowOf(store, second.id).failed).toBe(false)
  })
})
