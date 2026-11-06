/**
 * The Tauri platform with Tauri itself replaced: what `tauri.ts` sends the
 * app's Rust side. Every lane's host calls go to the chat in view (L4), so a
 * chat in its own copy of the project reverts, opens and restarts there.
 */
import { beforeAll, describe, expect, mock, test } from 'bun:test'

import type { Platform } from '../src/platform'

type Handler = (args: Record<string, unknown>) => unknown

const calls: Array<{ command: string; args: Record<string, unknown> }> = []
const handlers = new Map<string, Handler>()
const listeners = new Map<string, Array<(event: { payload: unknown }) => void>>()

const emit = (event: string, payload: unknown) => {
  for (const listener of listeners.get(event) ?? []) listener({ payload })
}
const callsOf = (command: string) => calls.filter((call) => call.command === command).map((call) => call.args)

mock.module('@tauri-apps/api/core', () => ({
  invoke: async (command: string, args: Record<string, unknown> = {}) => {
    calls.push({ command, args })
    const handler = handlers.get(command)
    return handler ? handler(args) : undefined
  },
}))
mock.module('@tauri-apps/api/event', () => ({
  listen: async (event: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(event, [...(listeners.get(event) ?? []), handler])
    return () => {}
  },
}))
mock.module('@tauri-apps/api/app', () => ({ getVersion: async () => '0.2.0' }))
mock.module('@tauri-apps/plugin-dialog', () => ({ open: async () => null }))
mock.module('@tauri-apps/plugin-store', () => ({ load: async () => ({ get: async () => undefined, set: async () => {} }) }))
mock.module('@tauri-apps/plugin-updater', () => ({ check: async () => null }))
mock.module('@tauri-apps/plugin-notification', () => ({
  isPermissionGranted: async () => true,
  requestPermission: async () => 'granted',
  sendNotification: () => {},
}))

const ready = JSON.stringify({ type: 'ready', protocol: 2, version: '1.0.0', features: [] })
const summary = (chat: string, generation: number, cwd: string) => ({
  chat,
  generation,
  project: { name: 'app', path: '/work/app' },
  cwd,
  branch: 'main',
  worktree: null,
  running: true,
  session: null,
})

let platform: Platform

beforeAll(async () => {
  let opens = 0
  handlers.set('chat_open', () => {
    opens += 1
    const chat = opens === 1 ? 'c1aaaaaa' : 'c2bbbbbb'
    emit('neuron://chat-line', { chat, generation: opens, line: ready })
    return summary(chat, opens, opens === 1 ? '/work/app' : '/wt/fix-login')
  })
  handlers.set('chat_resume', (args) => {
    emit('neuron://chat-line', { chat: args.chat, generation: 10, line: ready })
    return summary(String(args.chat), 10, '/wt/fix-login')
  })
  const { tauriPlatform } = await import('../src/platform/tauri')
  platform = tauriPlatform()
  platform.setLaunchOptions({ model: 'sonnet', reasoningEffort: 'high' })
  await platform.connect()
})

describe('the Tauri platform with several chats', () => {
  test('the first chat starts with the launch options, effort included (L3)', () => {
    expect(callsOf('chat_open')[0]).toMatchObject({ path: null, resume: null, options: { model: 'sonnet', reasoningEffort: 'high' } })
    expect(callsOf('chat_activate').at(-1)).toEqual({ chat: 'c1aaaaaa' })
  })

  test('reverting and opening in the editor go to the chat in view (L2)', async () => {
    await platform.gitRevert('src/main.rs')
    await platform.openInEditor('.')
    expect(callsOf('git_revert').at(-1)).toEqual({ path: 'src/main.rs', chat: 'c1aaaaaa' })
    expect(callsOf('open_in_editor').at(-1)).toEqual({ path: '.', chat: 'c1aaaaaa' })

    // A second chat, in its own copy, comes into view: the same calls follow it.
    const { info } = await platform.chats!.open({ worktree: { name: 'fix login' } })
    platform.chats!.activate(info.id)
    await platform.gitRevert('src/main.rs')
    await platform.openInEditor('.')
    await platform.list('.')
    await platform.gitStatus()
    expect(callsOf('git_revert').at(-1)).toEqual({ path: 'src/main.rs', chat: 'c2bbbbbb' })
    expect(callsOf('open_in_editor').at(-1)).toEqual({ path: '.', chat: 'c2bbbbbb' })
    expect(callsOf('fs_list').at(-1)).toEqual({ path: '.', chat: 'c2bbbbbb' })
    expect(callsOf('git_status').at(-1)).toEqual({ chat: 'c2bbbbbb' })
  })

  test('a restart (effort change, Try again, trust) is the chat in view\'s', async () => {
    platform.setLaunchOptions({ model: 'sonnet', reasoningEffort: 'low' })
    await platform.restart('s-7', { trustOnce: true })
    expect(callsOf('chat_resume').at(-1)).toEqual({
      chat: 'c2bbbbbb',
      resume: 's-7',
      options: { model: 'sonnet', reasoningEffort: 'low' },
      trustOnce: true,
    })
    // `null` asks for a fresh session.
    await platform.restart(null)
    expect(callsOf('chat_resume').at(-1)).toMatchObject({ chat: 'c2bbbbbb', resume: '', trustOnce: false })
  })

  test('L5\'s host calls are unchanged', async () => {
    handlers.set('engine_info', () => ({ source: 'bundled', path: '/app/neuron', bundledPath: '/app/neuron', bundledPresent: true }))
    handlers.set('diagnostics', () => 'App version: 0.2.0')
    expect((await platform.engineInfo())?.source).toBe('bundled')
    expect(await platform.hostDiagnostics()).toBe('App version: 0.2.0')
  })
})
