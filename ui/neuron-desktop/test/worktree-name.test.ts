import { describe, expect, test } from 'bun:test'

import { chatChatsHost, FakeIpc, summary } from './fake-ipc'
import { copyNameFromPrompt } from '../src/worktreeName'

const ready = JSON.stringify({ type: 'ready', protocol: 2, version: '1.0.0', features: [] })
const copy = (branch: string, named: boolean) => ({ worktree: { path: '/wt/chat-abc123', branch, base: 'main', named }, branch, cwd: '/wt/chat-abc123' })

// Cases from Freebuff's project-name.test.ts (commit 0dc95cb, Apache-2.0), for the ported function.
describe('copyNameFromPrompt', () => {
  test('names what the first message asks for', () => {
    expect(copyNameFromPrompt('Make a classic Snake game I can play in the browser with the arrow keys.')).toBe('Classic Snake Game')
    expect(copyNameFromPrompt('Build me a modern personal website with a home page.')).toBe('Modern Personal Website')
    expect(copyNameFromPrompt('Build a simple expense tracker where I can add expenses')).toBe('Expense Tracker')
    expect(copyNameFromPrompt('can you please create a todo app for my team')).toBe('Todo App')
    expect(copyNameFromPrompt("I'd like to build a Discord bot that posts the weather")).toBe('Discord Bot')
    expect(copyNameFromPrompt('fix the login page')).toBe('Fix the Login Page')
    expect(copyNameFromPrompt('hello')).toBe('Hello')
    expect(copyNameFromPrompt('A landing page for my SaaS startup')).toBe('Landing Page')
  })
  test('no words, no name', () => {
    expect(copyNameFromPrompt('   ')).toBeNull()
    expect(copyNameFromPrompt('!!!')).toBeNull()
  })
  test('stays short', () => {
    expect(copyNameFromPrompt('Supercalifragilisticexpialidocious-extraordinarily-long-hyphenated-thing')!.length).toBeLessThanOrEqual(40)
  })
  test('a prompt about a fix makes a branch-worthy name', () => {
    expect(copyNameFromPrompt('Please fix login redirect after sign-in\nIt loops.')).toBe('Fix Login Redirect After')
  })
})

describe('the chat host names a copy after its first prompt', () => {
  const openCopy = (ipc: FakeIpc, named: boolean) =>
    ipc.on('chat_open', () => {
      ipc.emit('neuron://chat-line', { chat: 'c1aaaaaa', generation: 1, line: ready })
      return summary('c1aaaaaa', 1, copy('neuron/chat-abc123', named))
    })

  test('the first prompt renames the branch once, and the chat info follows', async () => {
    const ipc = new FakeIpc()
    openCopy(ipc, false)
    ipc.on('worktree_name', (args) => summary('c1aaaaaa', 1, copy(`neuron/${String(args.name).toLowerCase().replace(/ /g, '-')}`, true)))
    const host = chatChatsHost(ipc)
    const { info, controller } = await host.open({ worktree: {} })
    controller.send('Fix the login redirect for admins')
    await Promise.resolve()
    expect(ipc.calls('worktree_name')).toEqual([{ chat: 'c1aaaaaa', name: 'Fix the Login Redirect' }])
    await new Promise((resolve) => setTimeout(resolve, 0))
    // The prompt itself still went to the engine.
    expect(ipc.calls('chat_send').map((call) => JSON.parse(String(call.line)).text)).toEqual(['Fix the login redirect for admins'])
    expect(host.info('c1aaaaaa')?.worktree?.branch).toBe('neuron/fix-the-login-redirect')
    expect(info.worktree?.branch).toBe('neuron/fix-the-login-redirect')
    expect(info.branch).toBe('neuron/fix-the-login-redirect')
    // A later prompt does not rename it again.
    controller.apply({ type: 'turn_finished' })
    controller.send('Now add a test')
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(ipc.calls('worktree_name')).toHaveLength(1)
  })

  test('a copy that has a name, or a chat in the project folder, is not renamed', async () => {
    for (const extra of [copy('neuron/mine', true), {}]) {
      const ipc = new FakeIpc()
      ipc.on('chat_open', () => {
        ipc.emit('neuron://chat-line', { chat: 'c1aaaaaa', generation: 1, line: ready })
        return summary('c1aaaaaa', 1, extra)
      })
      const host = chatChatsHost(ipc)
      const { controller } = await host.open({})
      controller.send('Fix the login redirect')
      await new Promise((resolve) => setTimeout(resolve, 0))
      expect(ipc.calls('worktree_name')).toHaveLength(0)
    }
  })

  test('a prompt with no words waits for one that has them; a refusal is quiet', async () => {
    const ipc = new FakeIpc()
    openCopy(ipc, false)
    ipc.on('worktree_name', () => Promise.reject('discarding'))
    const host = chatChatsHost(ipc)
    const { controller } = await host.open({ worktree: {} })
    controller.send('???')
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(ipc.calls('worktree_name')).toHaveLength(0)
    controller.apply({ type: 'turn_finished' })
    controller.send('add dark mode')
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(ipc.calls('worktree_name')).toEqual([{ chat: 'c1aaaaaa', name: 'Dark Mode' }])
    expect(host.info('c1aaaaaa')?.worktree?.branch).toBe('neuron/chat-abc123')
  })

  test('a copy can start from a chosen branch, and the branches are listed', async () => {
    const ipc = new FakeIpc()
    openCopy(ipc, false)
    ipc.on('worktree_branches', () => ({ branches: ['main', 'feature', 'origin/main'], default: 'main' }))
    const host = chatChatsHost(ipc)
    expect(await host.branches!('/work/app')).toEqual({ branches: ['main', 'feature', 'origin/main'], default: 'main' })
    expect(ipc.calls('worktree_branches')).toEqual([{ path: '/work/app' }])
    await host.open({ path: '/work/app', worktree: { start: 'feature' } })
    expect(ipc.calls('chat_open')[0]).toMatchObject({ worktree: { start: 'feature' } })
  })
})
