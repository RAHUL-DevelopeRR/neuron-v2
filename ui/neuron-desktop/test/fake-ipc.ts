import { DESKTOP_CAPABILITIES } from '@neuron/client'

import { createChatHost, type Ipc, type TauriChats } from '../src/platform/chat-host'

type Handler = (args: Record<string, unknown>) => unknown

/** Tauri's invoke and listen in memory: tests script the commands and emit events. */
export class FakeIpc implements Ipc {
  private handlers = new Map<string, Handler>()
  private listeners = new Map<string, Array<(payload: never) => void>>()
  private log: Array<{ command: string; args: Record<string, unknown> }> = []

  on(command: string, handler: Handler): void {
    this.handlers.set(command, handler)
  }

  calls(command: string): Array<Record<string, any>> {
    return this.log.filter((entry) => entry.command === command).map((entry) => entry.args)
  }

  async invoke<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
    this.log.push({ command, args })
    const handler = this.handlers.get(command)
    return (handler ? await handler(args) : undefined) as T
  }

  async listen<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
    const list = this.listeners.get(event) ?? []
    list.push(handler as (payload: never) => void)
    this.listeners.set(event, list)
    return () => undefined
  }

  emit(event: string, payload: unknown): void {
    for (const listener of this.listeners.get(event) ?? []) listener(payload as never)
  }
}

/** What the host returns for a chat. */
export function summary(chat: string, generation: number, extra: Record<string, unknown> = {}) {
  return {
    chat,
    generation,
    project: { name: 'app', path: '/work/app' },
    cwd: '/work/app',
    branch: 'main',
    worktree: null,
    running: true,
    session: null,
    ...extra,
  }
}

export function chatChatsHost(ipc: FakeIpc, now?: () => number): TauriChats {
  return createChatHost(ipc, { capabilities: DESKTOP_CAPABILITIES, launchOptions: () => ({}), now })
}
