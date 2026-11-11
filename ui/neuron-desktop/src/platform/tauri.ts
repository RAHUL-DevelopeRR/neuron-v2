/**
 * The desktop UI inside Tauri: the app's Rust side (src-tauri) runs one
 * `neuron engine --stdio` per chat and passes its lines through (chat-host.ts).
 * Commands go out with `invoke`; engine lines and terminal output come back
 * as events. Each engine the app starts has a generation, so the last lines of
 * one that was restarted never reach the next.
 */

import { getVersion } from '@tauri-apps/api/app'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { open } from '@tauri-apps/plugin-dialog'
import { load, type Store } from '@tauri-apps/plugin-store'
import { check } from '@tauri-apps/plugin-updater'
import { isPermissionGranted, requestPermission, sendNotification } from '@tauri-apps/plugin-notification'
import { DESKTOP_CAPABILITIES, type SessionController } from '@neuron/client'

import { createChatHost } from './chat-host'
import { createUpdateChecker } from './updates'
import type { DirListing, EngineInfo, FileContent, GitStatusView, KeyValueStore, LaunchOptions, Platform, Project, QuitRequest, TerminalSession } from './types'

interface TerminalOutput {
  id: number
  data: string
}

interface TerminalExit {
  id: number
  code: number | null
}

/** The app's own store file, loaded once. */
function tauriStore(): KeyValueStore {
  let loading: Promise<Store> | null = null
  const store = () => (loading ??= load('neuron-desktop.json', { autoSave: 200 }))
  return {
    get: async <T,>(key: string) => (await store()).get<T>(key),
    set: async (key, value) => (await store()).set(key, value),
  }
}

export function tauriPlatform(): Platform {
  let launchOptions: LaunchOptions = {}
  const chats = createChatHost(
    { invoke, listen: (event, handler) => listen(event, ({ payload }) => handler(payload as never)) },
    { capabilities: DESKTOP_CAPABILITIES, launchOptions: () => launchOptions },
  )
  // Once per window: React may run the effect that connects twice.
  let connecting: Promise<SessionController> | null = null
  // The updater needs a public key; builds without one point to the releases page.
  const updates = createUpdateChecker({ signed: import.meta.env.VITE_NEURON_UPDATER_SIGNED === 'true', check })
  // The host calls that read a folder take the chat in view.
  const chat = () => chats.activeId()
  let permission: Promise<boolean> | null = null
  const allowed = () =>
    (permission ??= isPermissionGranted().then(async (granted) => granted || (await requestPermission()) === 'granted'))
  return {
    kind: 'tauri',
    label: 'this device',
    can: { openProject: true, restart: true, terminal: true, git: true, pickFolder: true, updates: true, multiChat: true },
    store: tauriStore(),
    setLaunchOptions(options) {
      launchOptions = options
    },
    // The first chat, in the folder the app opened with; later ones come through `chats`.
    connect() {
      connecting ??= chats.open({}).then(({ info, controller }) => {
        chats.activate(info.id)
        return controller
      })
      return connecting
    },
    project: () => (chat() ? (chats.info(chat()!)?.project ?? null) : null),
    recentProjects: () => invoke<Project[]>('projects_recent'),
    async openProject(path, resume = null) {
      const { info } = await chats.open({ path, resume })
      chats.activate(info.id)
    },
    async pickFolder() {
      const picked = await open({ directory: true, multiple: false, title: 'Open a project folder' })
      return typeof picked === 'string' ? picked : null
    },
    appVersion: () => getVersion(),
    manualUpdates: updates.manual,
    checkForUpdate: () => updates.check(),
    async restart(resume, options) {
      const id = chat()
      if (!id) throw new Error('No chat is open.')
      // Until the new engine is ready, nothing can be sent (the host marks the chat as connecting).
      await chats.resume(id, { resume, trustOnce: options?.trustOnce === true })
    },
    list: (path) => invoke<DirListing>('fs_list', { path, chat: chat() }),
    read: (path) => invoke<FileContent>('fs_read', { path, chat: chat() }),
    searchFiles: (query) => invoke<string[]>('fs_search', { query, chat: chat() }),
    gitStatus: () => invoke<GitStatusView | null>('git_status', { chat: chat() }),
    gitDiff: (path) => invoke<string>('git_diff', { path, chat: chat() }),
    async openTerminal(cols, rows): Promise<TerminalSession> {
      const id = await invoke<number>('term_open', { cols, rows, chat: chat() })
      const dataListeners = new Set<(data: string) => void>()
      const exitListeners = new Set<(code: number | null) => void>()
      const unlisten = [
        await listen<TerminalOutput>('neuron://term-output', ({ payload }) => {
          if (payload.id === id) for (const listener of dataListeners) listener(payload.data)
        }),
        await listen<TerminalExit>('neuron://term-exit', ({ payload }) => {
          if (payload.id === id) for (const listener of exitListeners) listener(payload.code)
        }),
      ]
      return {
        write: (data) => void invoke('term_write', { id, data }),
        resize: (cols, rows) => void invoke('term_resize', { id, cols, rows }),
        close: () => {
          for (const stop of unlisten) stop()
          void invoke('term_close', { id })
        },
        onData: (listener) => {
          dataListeners.add(listener)
          return () => dataListeners.delete(listener)
        },
        onExit: (listener) => {
          exitListeners.add(listener)
          return () => exitListeners.delete(listener)
        },
      }
    },
    openExternal: (url) => void invoke('open_external', { url }),
    // L5: extensions and resilience
    engineInfo: () => invoke<EngineInfo>('engine_info'),
    hostDiagnostics: () => invoke<string>('diagnostics'),
    // L3: composer
    stageAttachment: (name, data, mediaType) =>
      invoke<{ path: string; media_type: string }>('attachment_stage', data, {
        headers: { 'x-attachment-name': encodeURIComponent(name), 'x-media-type': mediaType },
      }),
    exportSave: (suggestedName, content) => invoke<string | null>('export_save', { suggestedName, content }),
    // L2: review and git, in the chat in view's folder (its own copy for a chat in a worktree)
    gitRevert: (path) => invoke<void>('git_revert', { path, chat: chat() }),
    openInEditor: (path) => invoke<void>('open_in_editor', { path, chat: chat() }),
    // L4: multi-chat
    chats,
    async notify({ title, body }) {
      try {
        if (!(await allowed())) return 'denied'
        sendNotification({ title, body })
        return 'shown'
      } catch {
        return 'unsupported'
      }
    },
    setAttention: (count) => void invoke('attention_set', { count }).catch(() => {}),
    // v1.1: the Files tab's previews come as raw bytes, from the chat in view's folder.
    readMedia: async (path) => new Uint8Array(await invoke<ArrayBuffer>('fs_read_media', { path, chat: chat() })),
    quit: {
      onRequest(listener) {
        let stop: UnlistenFn | null = null
        let gone = false
        void listen<QuitRequest>('neuron://quit-request', ({ payload }) => listener(payload)).then((unlisten) => {
          if (gone) unlisten()
          else stop = unlisten
        })
        return () => {
          gone = true
          stop?.()
        }
      },
      answer: (id, quit) => void invoke('quit_answer', { id, quit }).catch(() => {}),
    },
  }
}
