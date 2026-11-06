/**
 * The window's chats: each has its own engine and controller, and the store
 * keeps what the sidebar needs of them (what each is doing, which finished
 * while out of view, which wait for the user), asks the host to tell the user
 * when a background chat needs them, and keeps the taskbar count.
 */

import { useCallback, useEffect, useState, useSyncExternalStore } from 'react'

import { chatActivity, openQuestionItem, type SessionController, type SessionState } from '@neuron/client'

import type { ChatError, ChatHost, ChatInfo, Platform } from './platform'

export type Activity = ReturnType<typeof chatActivity>

/** A chat as the sidebar lists it. */
export interface ChatRow {
  id: string
  title: string
  branch: string | null
  /** Has a copy of the project of its own. */
  worktree: boolean
  activity: Activity
  /** Finished a turn while out of view, and not looked at since. */
  unread: boolean
  projectName: string
  /** When it started needing the user (ms), to put the longest wait first. */
  needsSince: number | null
  /** Why a stopped engine stopped, in words, when the host said. */
  stopNote: string | null
}

export interface ChatItem extends ChatRow {
  info: ChatInfo
  controller: SessionController
  /** The engine's session, once it said. */
  sessionId: string | null
}

export interface ChatsSnapshot {
  items: ChatItem[]
  activeId: string | null
  needsYouCount: number
}

/**
 * Which chats finished while nobody looked: one that goes from working to idle
 * out of view is unread; opening it clears that. Returns `prev` itself when
 * nothing changed.
 */
export function nextUnread(prev: ReadonlySet<string>, id: string, from: Activity | null, to: Activity, isActive: boolean): ReadonlySet<string> {
  if (isActive) {
    if (!prev.has(id)) return prev
    const next = new Set(prev)
    next.delete(id)
    return next
  }
  if (from === 'running' && to === 'idle' && !prev.has(id)) return new Set(prev).add(id)
  return prev
}

/** The sidebar's four groups. A chat is in exactly one: needs you first, then working, then finished and unseen, then the rest. */
export function triage(rows: readonly ChatRow[]): { needsYou: ChatRow[]; running: ChatRow[]; doneUnread: ChatRow[]; idle: ChatRow[] } {
  const groups = { needsYou: [] as ChatRow[], running: [] as ChatRow[], doneUnread: [] as ChatRow[], idle: [] as ChatRow[] }
  for (const row of rows) {
    if (row.activity === 'needs_you') groups.needsYou.push(row)
    else if (row.activity === 'running') groups.running.push(row)
    else if (row.unread) groups.doneUnread.push(row)
    else groups.idle.push(row)
  }
  // The one waiting longest first.
  groups.needsYou.sort((a, b) => (a.needsSince ?? 0) - (b.needsSince ?? 0))
  return groups
}

const STOP_NOTE: Record<string, string> = {
  idle: 'Stopped after sitting idle',
  evicted: 'Stopped to make room for another chat',
}

function titleOf(state: SessionState, info: ChatInfo): string {
  const first = state.items.find((item) => item.kind === 'user')
  const text = first && first.kind === 'user' ? first.text.split('\n')[0].trim() : ''
  if (text) return text.length > 60 ? `${text.slice(0, 57)}…` : text
  return info.worktree ? info.worktree.branch.replace(/^neuron\//, '') : 'New chat'
}

export interface ChatsOptions {
  host: ChatHost | null
  notify: Platform['notify']
  setAttention: (count: number) => void
  /** A message for the user that is not an error of an action they took. */
  onToast: (message: string) => void
  /** Whether the user is looking at the window. */
  isFocused?: () => boolean
  now?: () => number
}

/** A notification that fired this recently makes the next window focus jump to the chat that needs the user. */
const FOCUS_JUMP_MS = 2 * 60 * 1000

interface Tracked {
  info: ChatInfo
  controller: SessionController
  activity: Activity | null
  needsSince: number | null
  stopNote: string | null
  reported: string
  unsubscribe: () => void
}

export class ChatsStore {
  private tracked = new Map<string, Tracked>()
  private order: string[] = []
  private unread: ReadonlySet<string> = new Set()
  private activeId: string | null = null
  private listeners = new Set<() => void>()
  private snapshot: ChatsSnapshot = { items: [], activeId: null, needsYouCount: 0 }
  private notified = new Set<string>()
  private notifiedAt = 0
  private attention = 0
  private toldDenied = false
  private stopListener: (() => void) | null = null
  /** Where messages for the user go; the app points it at its toast. */
  toast: (message: string) => void

  constructor(private readonly options: ChatsOptions) {
    this.toast = options.onToast
    this.stopListener = options.host?.onStopped((id, reason) => this.stopped(id, reason)) ?? null
  }

  private now = () => (this.options.now ?? Date.now)()

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  getSnapshot = (): ChatsSnapshot => this.snapshot

  get active(): ChatItem | null {
    return this.snapshot.items.find((item) => item.id === this.activeId) ?? null
  }

  /** Takes a chat the host has started (or, without a host, the one engine of the window). */
  add(info: ChatInfo, controller: SessionController, activate = true): void {
    const tracked: Tracked = {
      info,
      controller,
      activity: null,
      needsSince: null,
      stopNote: null,
      reported: '',
      unsubscribe: controller.subscribe(() => this.refresh()),
    }
    this.tracked.set(info.id, tracked)
    this.order.push(info.id)
    if (activate || this.activeId === null) this.setActive(info.id, false)
    else this.refresh()
  }

  /** Starts a chat through the host and puts it in view. */
  async open(request: { path?: string; resume?: string | null; worktree?: { name?: string } }): Promise<ChatItem> {
    const host = this.options.host
    if (!host) throw new Error('This window runs one chat.')
    const { info, controller } = await host.open(request)
    this.add(info, controller)
    return this.snapshot.items.find((item) => item.id === info.id)!
  }

  /** Chats a reloaded window found already open. */
  async reattach(): Promise<number> {
    const found = (await this.options.host?.reattach()) ?? []
    for (const { info, controller } of found) this.add(info, controller, false)
    return found.length
  }

  /** The chat that holds this session, if one is open. */
  findSession(sessionId: string): ChatItem | null {
    return this.snapshot.items.find((item) => item.controller.getState().status?.session_id === sessionId) ?? null
  }

  /** A chat in the project's main folder, if one is open. */
  findProject(path: string): ChatItem | null {
    return this.snapshot.items.find((item) => !item.info.worktree && item.info.project.path === path) ?? null
  }

  setActive(id: string, resumeStopped = true): void {
    const tracked = this.tracked.get(id)
    if (!tracked) return
    this.activeId = id
    this.unread = nextUnread(this.unread, id, tracked.activity, tracked.activity ?? 'idle', true)
    this.options.host?.activate(id)
    this.refresh()
    // Looking at a stopped chat starts it again where it left off.
    if (resumeStopped && tracked.activity === 'stopped') void this.resume(id)
  }

  /** A new engine for the chat; a refusal (the 4 working chats) is told to the user. */
  async resume(id: string, options?: { resume?: string | null; trustOnce?: boolean }): Promise<void> {
    try {
      await this.options.host?.resume(id, options)
    } catch (failure) {
      this.toast((failure as ChatError).message)
    }
  }

  async close(id: string): Promise<void> {
    const tracked = this.tracked.get(id)
    if (!tracked) return
    const index = this.order.indexOf(id)
    tracked.unsubscribe()
    this.tracked.delete(id)
    this.order = this.order.filter((other) => other !== id)
    const unread = new Set(this.unread)
    unread.delete(id)
    this.unread = unread
    if (this.activeId === id) {
      const next = this.order[Math.min(index, this.order.length - 1)] ?? null
      this.activeId = null
      if (next) this.setActive(next)
      else this.options.host?.activate(null)
    }
    this.refresh()
    await this.options.host?.close(id).catch(() => {})
  }

  /** The chat that needs the user longest, in view. Returns its id. */
  jumpToWaiting(): string | null {
    const waiting = triage(this.snapshot.items).needsYou[0]
    if (!waiting) return null
    if (waiting.id !== this.activeId) this.setActive(waiting.id)
    return waiting.id
  }

  /** The window came back into focus: if a notification just went out, go to the chat it was about. */
  onWindowFocus(): void {
    if (this.now() - this.notifiedAt > FOCUS_JUMP_MS) return
    const active = this.active
    if (active?.activity === 'needs_you') return
    this.notifiedAt = 0
    this.jumpToWaiting()
  }

  /** `delta` chats along, wrapping; returns the chat's id. */
  step(delta: number): string | null {
    if (this.order.length < 2) return null
    const index = Math.max(0, this.order.indexOf(this.activeId ?? ''))
    const next = this.order[(index + delta + this.order.length) % this.order.length]
    this.setActive(next)
    return next
  }

  dispose(): void {
    this.stopListener?.()
    for (const tracked of this.tracked.values()) tracked.unsubscribe()
    this.tracked.clear()
    this.listeners.clear()
  }

  private stopped(id: string, reason: string): void {
    const tracked = this.tracked.get(id)
    if (!tracked) return
    tracked.stopNote = STOP_NOTE[reason] ?? null
    this.refresh()
  }

  /** Reads every controller again: what changed is the activity, the title, the unread set, the waits and the count. */
  private refresh(): void {
    const now = this.now()
    const toNotify: Array<{ id: string; kind: 'permission' | 'question'; key: string }> = []
    const items: ChatItem[] = []
    for (const id of this.order) {
      const tracked = this.tracked.get(id)
      if (!tracked) continue
      const state = tracked.controller.getState()
      const activity = chatActivity(state)
      const before = tracked.activity
      if (activity !== before) {
        this.unread = nextUnread(this.unread, id, before, activity, id === this.activeId)
        if (activity === 'needs_you') tracked.needsSince = now
        else tracked.needsSince = null
        if (activity !== 'stopped') tracked.stopNote = null
      }
      tracked.activity = activity
      if (activity === 'needs_you') {
        const permission = state.permissions[0]
        const question = permission ? null : openQuestionItem(state)
        const key = permission ? `p${permission.id}` : question ? `q${question.id}` : ''
        if (key && !this.notified.has(`${id}:${key}`)) {
          this.notified.add(`${id}:${key}`)
          toNotify.push({ id, kind: permission ? 'permission' : 'question', key })
        }
      }
      const report = `${state.busy}:${activity === 'needs_you'}`
      if (report !== tracked.reported) {
        tracked.reported = report
        this.options.host?.reportActivity(id, { busy: state.busy, waiting: activity === 'needs_you' })
      }
      items.push({
        id,
        info: tracked.info,
        controller: tracked.controller,
        title: titleOf(state, tracked.info),
        branch: tracked.info.branch,
        worktree: tracked.info.worktree !== null,
        activity,
        unread: this.unread.has(id),
        projectName: tracked.info.project.name,
        needsSince: tracked.needsSince,
        stopNote: activity === 'stopped' ? tracked.stopNote : null,
        sessionId: state.status?.session_id || null,
      })
    }
    const needsYouCount = items.filter((item) => item.activity === 'needs_you').length
    if (needsYouCount !== this.attention) {
      this.attention = needsYouCount
      this.options.setAttention(needsYouCount)
    }
    const same =
      this.snapshot.activeId === this.activeId &&
      this.snapshot.items.length === items.length &&
      this.snapshot.items.every((old, index) => {
        const next = items[index]
        return (
          old.id === next.id &&
          old.controller === next.controller &&
          old.title === next.title &&
          old.activity === next.activity &&
          old.unread === next.unread &&
          old.needsSince === next.needsSince &&
          old.stopNote === next.stopNote &&
          old.sessionId === next.sessionId
        )
      })
    if (!same) {
      this.snapshot = { items, activeId: this.activeId, needsYouCount }
      for (const listener of [...this.listeners]) listener()
    }
    for (const waiting of toNotify) void this.tell(waiting, items)
  }

  /** A system notification, when the user is not looking at that chat. */
  private async tell({ id, kind }: { id: string; kind: 'permission' | 'question' }, items: ChatItem[]): Promise<void> {
    const looking = (this.options.isFocused?.() ?? true) && id === this.activeId
    if (looking) return
    const item = items.find((entry) => entry.id === id)
    if (!item) return
    this.notifiedAt = this.now()
    const result = await this.options.notify({
      chat: id,
      title: item.title,
      body: kind === 'permission' ? `${item.projectName}: needs your OK to continue` : `${item.projectName}: has a question for you`,
    })
    if (result !== 'shown' && !this.toldDenied) {
      this.toldDenied = true
      this.toast('Notifications are off, so chats that need you show a badge in the sidebar instead.')
    }
  }
}

/** One store, and its first chat, per platform: React may run the effect that starts them twice. */
const started = new WeakMap<Platform, { store: ChatsStore; ready: Promise<void> }>()

function startChats(platform: Platform): { store: ChatsStore; ready: Promise<void> } {
  let entry = started.get(platform)
  if (!entry) {
    const store = new ChatsStore({
      host: platform.chats ?? null,
      notify: platform.notify,
      setAttention: platform.setAttention,
      onToast: () => {},
      isFocused: () => document.hasFocus(),
    })
    const ready = (async () => {
      if (platform.chats) {
        if ((await store.reattach()) === 0) await store.open({})
        else store.setActive(store.getSnapshot().items[0].id, false)
      } else {
        const controller = await platform.connect()
        const project = platform.project() ?? { name: 'project', path: '' }
        store.add({ id: 'single', project, cwd: project.path, branch: null, worktree: null }, controller)
      }
    })()
    entry = { store, ready }
    started.set(platform, entry)
  }
  return entry
}

/** The window's chats: through the host when the platform has one, else the platform's single engine as one chat. */
export function useChats(platform: Platform, ready: boolean, onToast: (message: string) => void) {
  const [store, setStore] = useState<ChatsStore | null>(null)
  const [launchError, setLaunchError] = useState<string | null>(null)

  useEffect(() => {
    if (!ready) return
    let cancelled = false
    const entry = startChats(platform)
    entry.store.toast = onToast
    entry.ready.then(
      () => !cancelled && setStore(entry.store),
      (error: Error) => !cancelled && setLaunchError(error.message),
    )
    return () => {
      cancelled = true
    }
    // The settings were read once; later changes apply to the next engine.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [platform, ready])
  useEffect(() => {
    if (store) store.toast = onToast
  }, [store, onToast])

  const snapshot = useSyncExternalStore(
    store?.subscribe ?? noopSubscribe,
    store?.getSnapshot ?? (() => EMPTY),
    store?.getSnapshot ?? (() => EMPTY),
  )
  const focus = useCallback(() => store?.onWindowFocus(), [store])
  useEffect(() => {
    window.addEventListener('focus', focus)
    return () => window.removeEventListener('focus', focus)
  }, [focus])

  const active = snapshot.items.find((item) => item.id === snapshot.activeId) ?? null
  return { store, snapshot, active, launchError }
}

const EMPTY: ChatsSnapshot = { items: [], activeId: null, needsYouCount: 0 }
const noopSubscribe = () => () => {}
