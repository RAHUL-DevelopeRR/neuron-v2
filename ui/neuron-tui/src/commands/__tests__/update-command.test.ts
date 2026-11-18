import { describe, expect, test } from 'bun:test'

import { UPDATE_CHECKING_TEXT, runUpdateCommand } from '../update-command'
import { DEFAULT_MANIFEST_URL, type UpdateCheckResult } from '../../utils/update-check'

import type { ChatMessage } from '../../types/chat'

function chat(initial: ChatMessage[] = []) {
  let messages = initial
  const setMessages = (value: ChatMessage[] | ((prev: ChatMessage[]) => ChatMessage[])) => {
    messages = typeof value === 'function' ? value(messages) : value
  }
  return { setMessages, get: () => messages, set: (next: ChatMessage[]) => (messages = next) }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

const AVAILABLE: UpdateCheckResult = {
  status: 'available',
  current: '7.0.0-alpha.5',
  latest: '7.0.0-alpha.6',
  minimum: null,
  notesUrl: null,
  command: 'npm install -g @zero-x.live/neuron',
  method: 'npm',
  required: false,
  manifestUrl: DEFAULT_MANIFEST_URL,
}

const text = (message: ChatMessage) => message.content

describe('/update', () => {
  test('shows the command and a checking line at once, then the report in its place', async () => {
    const view = chat()
    const check = deferred<UpdateCheckResult>()
    const copied: string[] = []
    const running = runUpdateCommand('/update', {
      setMessages: view.setMessages,
      refresh: () => check.promise,
      copy: async (command) => {
        copied.push(command)
        return true
      },
    })

    // Before the check ends: the echo and the checking line are there.
    expect(view.get().map(text)).toEqual(['/update', UPDATE_CHECKING_TEXT])
    expect(view.get()[0]!.variant).toBe('user')

    // A message sent while the check runs stays below the /update lines.
    view.set([...view.get(), { ...view.get()[0]!, id: 'user-later', content: 'hello' }])

    check.resolve(AVAILABLE)
    await running
    const after = view.get()
    expect(after.map(text)[0]).toBe('/update')
    expect(after[1]!.content).toContain('Update available: Neuron 7.0.0-alpha.6.')
    expect(after[1]!.content).toContain('The command is copied.')
    expect(after[1]!.content).not.toContain(UPDATE_CHECKING_TEXT)
    expect(after[2]!.content).toBe('hello')
    expect(after).toHaveLength(3)
    expect(copied).toEqual([AVAILABLE.command])
  })

  test('two /update runs in the same millisecond each fill their own line', async () => {
    const view = chat()
    const first = deferred<UpdateCheckResult>()
    const second = deferred<UpdateCheckResult>()
    const runs = [
      runUpdateCommand('/update', { setMessages: view.setMessages, refresh: () => first.promise, copy: async () => false }),
      runUpdateCommand('/update', { setMessages: view.setMessages, refresh: () => second.promise, copy: async () => false }),
    ]
    const ids = view.get().map((message) => message.id)
    expect(new Set(ids.filter((id) => id.startsWith('sys-update-'))).size).toBe(2)

    second.resolve({ status: 'up-to-date', current: '7.0.0-alpha.6', latest: '7.0.0-alpha.6', notesUrl: null, manifestUrl: DEFAULT_MANIFEST_URL })
    first.resolve({ status: 'failed', current: '7.0.0-alpha.6', reason: 'timed out', manifestUrl: DEFAULT_MANIFEST_URL })
    await Promise.all(runs)
    const lines = view.get().map(text)
    expect(lines[1]).toContain('Could not check for updates: timed out.')
    expect(lines[3]).toContain('is up to date')
  })

  test('a chat cleared while checking gets no stray report', async () => {
    const view = chat()
    const check = deferred<UpdateCheckResult>()
    const running = runUpdateCommand('/update', { setMessages: view.setMessages, refresh: () => check.promise })
    view.set([])
    check.resolve(AVAILABLE)
    await running
    expect(view.get()).toEqual([])
  })

  test('a check that throws still replaces the checking line', async () => {
    const view = chat()
    await runUpdateCommand('/update', {
      setMessages: view.setMessages,
      refresh: () => Promise.reject(new Error('boom')),
    })
    expect(view.get().map(text)).toEqual(['/update', 'Could not check for updates: the check could not start.'])
  })

  test('a copy that throws reports the command without claiming it is copied', async () => {
    const view = chat()
    await runUpdateCommand('/update', {
      setMessages: view.setMessages,
      refresh: async () => AVAILABLE,
      copy: async () => {
        throw new Error('no clipboard')
      },
    })
    const report = view.get()[1]!.content
    expect(report).toContain(`Update with: ${AVAILABLE.command}`)
    expect(report).not.toContain('The command is copied.')
  })
})
