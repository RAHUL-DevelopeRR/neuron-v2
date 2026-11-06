import { describe, expect, test } from 'bun:test'

import { DESKTOP_CAPABILITIES } from '@neuron/client'

import { chatChatsHost, FakeIpc, summary } from './fake-ipc'
import { ChatError, type LaunchOptions } from '../src/platform'
import { createChatHost } from '../src/platform/chat-host'

const ready = JSON.stringify({ type: 'ready', protocol: 2, version: '1.0.0', features: [] })
const status = (session: string) => JSON.stringify({ type: 'status', model: 'm', permission_mode: 'workspace-write', session_id: session, cwd: '/work/app', workspace_trusted: true })

describe('the chat host over IPC', () => {
  test('lines that arrive before chat_open returns are kept and delivered', async () => {
    const ipc = new FakeIpc()
    // The engine says ready while the host call is still returning.
    ipc.on('chat_open', () => {
      ipc.emit('neuron://chat-line', { chat: 'c1aaaaaa', generation: 1, line: ready })
      ipc.emit('neuron://chat-line', { chat: 'c1aaaaaa', generation: 1, line: status('s-1') })
      return summary('c1aaaaaa', 1)
    })
    const host = chatChatsHost(ipc)
    const { info, controller } = await host.open({ path: '/work/app' })
    expect(info.id).toBe('c1aaaaaa')
    expect(controller.getState().connection).toBe('ready')
    expect(controller.getState().status?.session_id).toBe('s-1')
    expect(ipc.calls('chat_open')[0]).toMatchObject({ path: '/work/app', resume: null, worktree: null })
  })

  test('lines for a chat that is not registered yet are kept however long it takes', async () => {
    let now = 1000
    const ipc = new FakeIpc()
    const host = chatChatsHost(ipc, () => now)
    ipc.on('chat_open', () => {
      ipc.emit('neuron://chat-line', { chat: 'c2bbbbbb', generation: 1, line: status('kept') })
      now += 20_000
      ipc.emit('neuron://chat-line', { chat: 'c2bbbbbb', generation: 1, line: ready })
      return summary('c2bbbbbb', 1)
    })
    const { controller } = await host.open({})
    expect(controller.getState().connection).toBe('ready')
    expect(controller.getState().status?.session_id).toBe('kept')
  })

  test('the buffer for a chat that is not registered has a size cap: the oldest lines go', async () => {
    const ipc = new FakeIpc()
    const host = chatChatsHost(ipc)
    ipc.on('chat_open', () => {
      ipc.emit('neuron://chat-line', { chat: 'c2bbbbbb', generation: 1, line: status('oldest') })
      for (let i = 0; i < 2000; i++) ipc.emit('neuron://chat-line', { chat: 'c2bbbbbb', generation: 1, line: status(`s-${i}`) })
      ipc.emit('neuron://chat-line', { chat: 'c2bbbbbb', generation: 1, line: ready })
      return summary('c2bbbbbb', 1)
    })
    const { controller } = await host.open({})
    expect(controller.getState().status?.session_id).toBe('s-1999')
  })

  test('lines for a chat nobody registered are pruned after a while', async () => {
    let now = 1000
    const ipc = new FakeIpc()
    const host = chatChatsHost(ipc, () => now)
    // The first chat starts the listeners.
    ipc.on('chat_open', () => {
      ipc.emit('neuron://chat-line', { chat: 'c1aaaaaa', generation: 1, line: ready })
      return summary('c1aaaaaa', 1)
    })
    await host.open({})
    ipc.emit('neuron://chat-line', { chat: 'ghost', generation: 1, line: status('ghost-old') })
    now += 10 * 60 * 1000
    ipc.emit('neuron://chat-line', { chat: 'other', generation: 1, line: status('other') })
    ipc.on('chat_open', () => {
      ipc.emit('neuron://chat-line', { chat: 'ghost', generation: 1, line: ready })
      return summary('ghost', 1)
    })
    const { controller } = await host.open({})
    expect(controller.getState().connection).toBe('ready')
    expect(controller.getState().status).toBeNull()
  })

  test('events for a closed chat are dropped', async () => {
    const ipc = new FakeIpc()
    const host = chatChatsHost(ipc)
    ipc.on('chat_open', () => {
      ipc.emit('neuron://chat-line', { chat: 'c1aaaaaa', generation: 1, line: ready })
      return summary('c1aaaaaa', 1)
    })
    const stopped: string[] = []
    host.onStopped((id) => stopped.push(id))
    const { controller } = await host.open({})
    await host.close('c1aaaaaa')
    // Late events of the closed engine reach nothing: not the controller, not the stop listeners.
    ipc.emit('neuron://chat-line', { chat: 'c1aaaaaa', generation: 1, line: status('late') })
    ipc.emit('neuron://chat-exit', { chat: 'c1aaaaaa', generation: 1, code: 1, error: 'late', reason: 'exited' })
    expect(controller.getState().status).toBeNull()
    expect(controller.getState().connection).toBe('ready')
    expect(stopped).toEqual([])
  })

  test('each chat gets only its own lines, and an old generation is ignored', async () => {
    const ipc = new FakeIpc()
    let count = 0
    ipc.on('chat_open', () => {
      const id = count++ === 0 ? 'c1aaaaaa' : 'c2bbbbbb'
      ipc.emit('neuron://chat-line', { chat: id, generation: count, line: ready })
      return summary(id, count)
    })
    const host = chatChatsHost(ipc)
    const first = await host.open({ path: '/a' })
    const second = await host.open({ path: '/b' })
    ipc.emit('neuron://chat-line', { chat: 'c2bbbbbb', generation: 2, line: status('only-second') })
    ipc.emit('neuron://chat-line', { chat: 'c2bbbbbb', generation: 99, line: status('stale') })
    expect(second.controller.getState().status?.session_id).toBe('only-second')
    expect(first.controller.getState().status).toBeNull()
  })

  test('messages go out through chat_send with the chat id', async () => {
    const ipc = new FakeIpc()
    ipc.on('chat_open', () => {
      ipc.emit('neuron://chat-line', { chat: 'c1aaaaaa', generation: 1, line: ready })
      return summary('c1aaaaaa', 1)
    })
    const host = chatChatsHost(ipc)
    const { controller } = await host.open({})
    controller.interrupt()
    const sent = ipc.calls('chat_send')
    expect(sent.length).toBeGreaterThan(0)
    expect(sent[0].chat).toBe('c1aaaaaa')
    expect(typeof sent[0].line).toBe('string')
  })

  test('the host\'s own spawn error reaches the caller unchanged', async () => {
    const ipc = new FakeIpc()
    ipc.on('chat_open', () => Promise.reject('[engine:quarantined] Could not start neuron.exe'))
    const host = chatChatsHost(ipc)
    await expect(host.open({})).rejects.toThrow('[engine:quarantined] Could not start neuron.exe')
  })

  test('a full house is a ChatError with a plain message', async () => {
    const ipc = new FakeIpc()
    ipc.on('chat_open', () => Promise.reject('cap_reached'))
    const host = chatChatsHost(ipc)
    const error = (await host.open({}).catch((failure: unknown) => failure)) as ChatError
    expect(error).toBeInstanceOf(ChatError)
    expect(error.code).toBe('cap_reached')
    expect(error.message).toBe('4 chats are working. Stop one to open another.')
  })

  test('an engine that ends before it is ready fails the open and the chat is closed', async () => {
    const ipc = new FakeIpc()
    ipc.on('chat_open', () => {
      ipc.emit('neuron://chat-exit', { chat: 'c1aaaaaa', generation: 1, code: 1, error: 'bad launch', reason: 'exited' })
      return summary('c1aaaaaa', 1)
    })
    const host = chatChatsHost(ipc)
    await expect(host.open({})).rejects.toThrow('bad launch')
    expect(ipc.calls('chat_close')[0]).toEqual({ chat: 'c1aaaaaa' })
  })

  test('onStopped reports why an engine ended, and the controller shows it stopped', async () => {
    const ipc = new FakeIpc()
    ipc.on('chat_open', () => {
      ipc.emit('neuron://chat-line', { chat: 'c1aaaaaa', generation: 1, line: ready })
      return summary('c1aaaaaa', 1)
    })
    const host = chatChatsHost(ipc)
    const stopped: Array<[string, string]> = []
    host.onStopped((id, reason) => stopped.push([id, reason]))
    const { controller } = await host.open({})
    ipc.emit('neuron://chat-exit', { chat: 'c1aaaaaa', generation: 1, code: null, error: null, reason: 'idle' })
    expect(stopped).toEqual([['c1aaaaaa', 'idle']])
    expect(controller.getState().connection).toBe('exited')
  })

  test('resume starts a new generation; a fresh session is asked for with an empty id', async () => {
    const ipc = new FakeIpc()
    ipc.on('chat_open', () => {
      ipc.emit('neuron://chat-line', { chat: 'c1aaaaaa', generation: 1, line: ready })
      return summary('c1aaaaaa', 1)
    })
    ipc.on('chat_resume', () => {
      ipc.emit('neuron://chat-line', { chat: 'c1aaaaaa', generation: 2, line: ready })
      return summary('c1aaaaaa', 2)
    })
    const host = chatChatsHost(ipc)
    const { controller } = await host.open({})
    await host.resume('c1aaaaaa')
    await host.resume('c1aaaaaa', { resume: null, trustOnce: true })
    const [first, second] = ipc.calls('chat_resume')
    expect(first).toMatchObject({ chat: 'c1aaaaaa', resume: null, trustOnce: false })
    expect(second).toMatchObject({ resume: '', trustOnce: true })
    expect(controller.getState().connection).toBe('ready')
  })

  test('discarding a copy maps the host\'s questions to codes and counts', async () => {
    const ipc = new FakeIpc()
    ipc.on('chat_open', () => {
      ipc.emit('neuron://chat-line', { chat: 'c1aaaaaa', generation: 1, line: ready })
      return summary('c1aaaaaa', 1)
    })
    ipc.on('worktree_discard', () => Promise.reject('unmerged:2'))
    const host = chatChatsHost(ipc)
    await host.open({ worktree: { name: 'x' } })
    const error = (await host.discardWorktree('c1aaaaaa', { force: false, deleteUnmerged: false }).catch((failure: unknown) => failure)) as ChatError
    expect(error.code).toBe('unmerged')
    expect(error.count).toBe(2)
    expect(ipc.calls('worktree_discard')[0]).toEqual({ chat: 'c1aaaaaa', force: false, deleteUnmerged: false })
  })

  test('the active chat is tracked and sent to the host', async () => {
    const ipc = new FakeIpc()
    const host = chatChatsHost(ipc)
    host.activate('c1aaaaaa')
    expect(host.activeId()).toBe('c1aaaaaa')
    host.activate(null)
    expect(ipc.calls('chat_activate')).toEqual([{ chat: 'c1aaaaaa' }, { chat: null }])
  })
})

describe('every lane through the chat host', () => {
  const opened = (ipc: FakeIpc) =>
    ipc.on('chat_open', () => {
      ipc.emit('neuron://chat-line', { chat: 'c1aaaaaa', generation: 1, line: ready })
      return summary('c1aaaaaa', 1)
    })

  test('L3: the reasoning effort goes with every chat_open and chat_resume', async () => {
    const ipc = new FakeIpc()
    let launch: LaunchOptions = { model: 'sonnet', reasoningEffort: 'high' }
    const host = createChatHost(ipc, { capabilities: DESKTOP_CAPABILITIES, launchOptions: () => launch })
    opened(ipc)
    ipc.on('chat_resume', () => {
      ipc.emit('neuron://chat-line', { chat: 'c1aaaaaa', generation: 2, line: ready })
      return summary('c1aaaaaa', 2)
    })
    await host.open({})
    expect(ipc.calls('chat_open')[0].options).toEqual({ model: 'sonnet', reasoningEffort: 'high' })
    // A change of effort applies to the next engine: the restart carries it.
    launch = { model: 'sonnet', reasoningEffort: 'low' }
    await host.resume('c1aaaaaa', { resume: 's-1' })
    expect(ipc.calls('chat_resume')[0]).toMatchObject({ chat: 'c1aaaaaa', resume: 's-1', options: { reasoningEffort: 'low' } })
  })

  test('L5: a coded problem from chat_resume reaches the caller and the chat unchanged', async () => {
    const ipc = new FakeIpc()
    const host = chatChatsHost(ipc)
    opened(ipc)
    const quarantined = '[engine:quarantined] Windows security blocked the Neuron engine. Restore it from Protection history, or reinstall Neuron.'
    ipc.on('chat_resume', () => Promise.reject(quarantined))
    const { controller } = await host.open({})
    const error = (await host.resume('c1aaaaaa').catch((failure: unknown) => failure)) as ChatError
    expect(error).toBeInstanceOf(ChatError)
    expect(error.message).toBe(quarantined)
    expect(error.code).toBeUndefined()
    expect(controller.getState().connection).toBe('exited')
    expect(controller.getState().connectionError).toBe(quarantined)
  })

  test("L5: an engine that crashes after it was ready shows the host's coded problem", async () => {
    const ipc = new FakeIpc()
    const host = chatChatsHost(ipc)
    opened(ipc)
    const { controller } = await host.open({})
    const crashed = '[engine:crashed] The Neuron engine stopped unexpectedly (exit code 3). Last message: out of memory'
    ipc.emit('neuron://chat-exit', { chat: 'c1aaaaaa', generation: 1, code: 3, error: crashed, reason: 'exited' })
    expect(controller.getState().connection).toBe('exited')
    expect(controller.getState().connectionError).toBe(crashed)
  })

  test('L5: an engine that is missing fails the first open with its coded problem', async () => {
    const ipc = new FakeIpc()
    const host = chatChatsHost(ipc)
    const missing = "[engine:missing] The Neuron engine was not found. Reinstall Neuron, or set NEURON_BIN to the engine's path."
    ipc.on('chat_open', () => Promise.reject(missing))
    const error = (await host.open({}).catch((failure: unknown) => failure)) as Error
    expect(error.message).toBe(missing)
    expect(ipc.calls('chat_close')).toEqual([])
  })
})

describe('the chat host: resilience', () => {
  const opened = (ipc: FakeIpc) =>
    ipc.on('chat_open', () => {
      ipc.emit('neuron://chat-line', { chat: 'c1aaaaaa', generation: 1, line: ready })
      return summary('c1aaaaaa', 1)
    })

  test('reattach relaunches only the chats that run, marks the others stopped quietly, and does not wait for engines', async () => {
    const ipc = new FakeIpc()
    ipc.on('chat_list', () => [summary('c1aaaaaa', 1, { running: true }), summary('c2bbbbbb', 1, { running: false })])
    // The running chat's engine never says ready.
    ipc.on('chat_resume', () => summary('c1aaaaaa', 2))
    const host = chatChatsHost(ipc)
    const found = await host.reattach()
    expect(found.map(({ info }) => info.id)).toEqual(['c1aaaaaa', 'c2bbbbbb'])
    expect(ipc.calls('chat_resume').map((call) => call.chat)).toEqual(['c1aaaaaa'])
    const [running, stopped] = found.map(({ controller }) => controller.getState())
    expect(running.connection).toBe('connecting')
    expect(stopped.connection).toBe('exited')
    expect(stopped.connectionError).toBeNull()
  })

  test('start() and relaunch() time out with a clear message', async () => {
    const ipc = new FakeIpc()
    const host = createChatHost(ipc, { capabilities: DESKTOP_CAPABILITIES, launchOptions: () => ({}), startTimeoutMs: 30 })
    ipc.on('chat_open', () => summary('c1aaaaaa', 1))
    await expect(host.open({})).rejects.toThrow(/did not start within/)
    // A resume whose engine never says ready.
    const ipc2 = new FakeIpc()
    const host2 = createChatHost(ipc2, { capabilities: DESKTOP_CAPABILITIES, launchOptions: () => ({}), startTimeoutMs: 30 })
    opened(ipc2)
    ipc2.on('chat_resume', () => summary('c1aaaaaa', 2))
    const { controller } = await host2.open({})
    await expect(host2.resume('c1aaaaaa')).rejects.toThrow(/did not start within/)
    expect(controller.getState().connection).toBe('exited')
  })

  test('a failed chat_send marks the chat disconnected with the error', async () => {
    const ipc = new FakeIpc()
    opened(ipc)
    ipc.on('chat_send', () => Promise.reject('the engine pipe is closed'))
    const host = chatChatsHost(ipc)
    const { controller } = await host.open({})
    controller.interrupt()
    await new Promise((resolve) => setTimeout(resolve, 10))
    expect(controller.getState().connection).toBe('exited')
    expect(controller.getState().connectionError).toContain('the engine pipe is closed')
  })

  test('closing a chat that has its own copy fails with has_worktree and keeps the chat', async () => {
    const ipc = new FakeIpc()
    opened(ipc)
    ipc.on('chat_close', () => Promise.reject('has_worktree'))
    const host = chatChatsHost(ipc)
    const { controller } = await host.open({ worktree: {} })
    const error = (await host.close('c1aaaaaa').catch((failure: unknown) => failure)) as ChatError
    expect(error).toBeInstanceOf(ChatError)
    expect(error.code).toBe('has_worktree')
    // Still routed: the chat is alive.
    ipc.emit('neuron://chat-line', { chat: 'c1aaaaaa', generation: 1, line: status('still-here') })
    expect(controller.getState().status?.session_id).toBe('still-here')
    expect(host.info('c1aaaaaa')).not.toBeNull()
  })

  test('another failure of chat_close is not a reason to keep the chat', async () => {
    const ipc = new FakeIpc()
    opened(ipc)
    ipc.on('chat_close', () => Promise.reject('gone already'))
    const host = chatChatsHost(ipc)
    await host.open({})
    await host.close('c1aaaaaa')
    expect(host.info('c1aaaaaa')).toBeNull()
  })
})

describe('the chat host: the host\'s newer refusals', () => {
  const opened = (ipc: FakeIpc) =>
    ipc.on('chat_open', () => {
      ipc.emit('neuron://chat-line', { chat: 'c1aaaaaa', generation: 1, line: ready })
      return summary('c1aaaaaa', 1)
    })

  test('discarding and starting are coded, in plain words', async () => {
    const ipc = new FakeIpc()
    opened(ipc)
    const host = chatChatsHost(ipc)
    await host.open({})
    ipc.on('chat_resume', () => Promise.reject('discarding'))
    const gone = (await host.resume('c1aaaaaa').catch((failure: unknown) => failure)) as ChatError
    expect(gone.code).toBe('discarding')
    expect(gone.message).toBe("This chat's copy is being removed.")
    ipc.on('worktree_discard', () => Promise.reject('starting'))
    const early = (await host.discardWorktree('c1aaaaaa', { force: false, deleteUnmerged: false }).catch((failure: unknown) => failure)) as ChatError
    expect(early.code).toBe('starting')
    expect(early.message).toBe('This chat is still starting. Try again in a moment.')
  })

  test('a full input queue is an error line, and the chat stays connected', async () => {
    const ipc = new FakeIpc()
    opened(ipc)
    ipc.on('chat_send', () => Promise.reject('The Neuron engine is not keeping up. Try again in a moment.'))
    const host = chatChatsHost(ipc)
    const { controller } = await host.open({})
    controller.submit('hello')
    await new Promise((resolve) => setTimeout(resolve, 10))
    const state = controller.getState()
    expect(state.connection).toBe('ready')
    const last = state.items.at(-1)
    expect(last).toMatchObject({ kind: 'error', text: 'The Neuron engine is not keeping up. Try again in a moment.' })
  })

  test('a failed discard that leaves the chat stopped can be resumed', async () => {
    const ipc = new FakeIpc()
    opened(ipc)
    ipc.on('worktree_discard', () => Promise.reject('boom'))
    ipc.on('chat_resume', () => {
      ipc.emit('neuron://chat-line', { chat: 'c1aaaaaa', generation: 2, line: ready })
      return summary('c1aaaaaa', 2)
    })
    const host = chatChatsHost(ipc)
    const { controller } = await host.open({})
    await expect(host.discardWorktree('c1aaaaaa', { force: true, deleteUnmerged: false })).rejects.toThrow('boom')
    ipc.emit('neuron://chat-exit', { chat: 'c1aaaaaa', generation: 1, code: 0, error: null, reason: 'exited' })
    expect(controller.getState().connection).toBe('exited')
    await host.resume('c1aaaaaa')
    expect(controller.getState().connection).toBe('ready')
  })
})
