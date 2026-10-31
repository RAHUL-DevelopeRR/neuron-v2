/**
 * The engine's git views (protocol features `git_views` and `checkpoints`):
 * the files the agent changed this session, a commit proposal waiting for the
 * user, and the `/rewind` picker's state. The events are dispatched here by
 * engine/git-views.ts; the panels read it.
 */

import { create } from 'zustand'
import { immer } from 'zustand/middleware/immer'

import { FEATURE_CHECKPOINTS, FEATURE_GIT_VIEWS } from '../engine/protocol'

import type {
  ChangedFile,
  CheckpointInfo,
  CommitProposalEvent,
  RewindMode,
  RewoundEvent,
} from '../engine/protocol'
import type { ChatMessage } from '../types/chat'

export type CommitProposal = Omit<CommitProposalEvent, 'type'>
export type RewindResult = Omit<RewoundEvent, 'type'>

/**
 * `/rewind` in steps: pick a checkpoint, then what to put back, then a dry
 * run's preview to confirm. The real rewind's answer closes it.
 */
export interface RewindFlow {
  /** Newest first, as the engine sent them. */
  items: CheckpointInfo[]
  checkpoint: CheckpointInfo | null
  mode: RewindMode | null
  /** The dry run's answer for `checkpoint` and `mode`; `null` while it runs. */
  preview: RewindResult | null
  /** The real rewind was sent. */
  confirmed: boolean
  /** Why the dry run gave no preview (the engine's error). */
  error: string | null
}

export type RewindStep = 'pick' | 'mode' | 'preview' | 'running'

export function rewindStep(flow: RewindFlow): RewindStep {
  if (flow.confirmed) return 'running'
  if (!flow.checkpoint) return 'pick'
  if (!flow.mode) return 'mode'
  return 'preview'
}

export interface GitViewsState {
  /** `ready.features` of the running engine. */
  features: string[]
  /** The session's changed files, from the last `changes` event. */
  changes: ChangedFile[]
  changesPanelOpen: boolean
  /** A `/commit` proposal waiting for Enter, Tab or Esc. */
  proposal: CommitProposal | null
  rewind: RewindFlow | null
  /**
   * The proposal or rewind picker waits behind the composer instead of
   * taking its place: it came while the user had a draft there, or they
   * typed into it. Enter on an empty input brings it up.
   */
  deferred: boolean

  /** A new engine: its features, and nothing from the last one. */
  setFeatures: (features: string[]) => void
  setChanges: (files: ChangedFile[]) => void
  openChangesPanel: () => void
  closeChangesPanel: () => void
  setProposal: (proposal: CommitProposal | null, options?: { deferred?: boolean }) => void
  openRewind: (items: CheckpointInfo[], options?: { deferred?: boolean }) => void
  /** The proposal or picker steps aside for the composer, or comes back. */
  setDeferred: (deferred: boolean) => void
  pickCheckpoint: (checkpoint: CheckpointInfo | null) => void
  pickMode: (mode: RewindMode | null) => void
  setRewindPreview: (preview: RewindResult) => void
  setRewindError: (error: string | null) => void
  markRewindConfirmed: () => void
  closeRewind: () => void
  reset: () => void
}

const initial = {
  features: [] as string[],
  changes: [] as ChangedFile[],
  changesPanelOpen: false,
  proposal: null,
  rewind: null,
  deferred: false,
}

export const useGitViewsStore = create<GitViewsState>()(
  immer((set) => ({
    ...initial,
    setFeatures: (features) => set(() => ({ ...initial, features: [...features] })),
    setChanges: (files) =>
      set((state) => {
        state.changes = files
      }),
    openChangesPanel: () =>
      set((state) => {
        state.changesPanelOpen = true
      }),
    closeChangesPanel: () =>
      set((state) => {
        state.changesPanelOpen = false
      }),
    setProposal: (proposal, options = {}) =>
      set((state) => {
        state.proposal = proposal
        state.deferred =
          proposal !== null ? options.deferred === true : state.rewind !== null && state.deferred
      }),
    openRewind: (items, options = {}) =>
      set((state) => {
        state.deferred = options.deferred === true
        state.rewind = {
          items,
          checkpoint: null,
          mode: null,
          preview: null,
          confirmed: false,
          error: null,
        }
      }),
    pickCheckpoint: (checkpoint) =>
      set((state) => {
        if (!state.rewind) return
        state.rewind.checkpoint = checkpoint
        state.rewind.mode = null
        state.rewind.preview = null
        state.rewind.error = null
      }),
    pickMode: (mode) =>
      set((state) => {
        if (!state.rewind) return
        state.rewind.mode = mode
        state.rewind.preview = null
        state.rewind.error = null
      }),
    setRewindPreview: (preview) =>
      set((state) => {
        if (state.rewind) state.rewind.preview = preview
      }),
    setRewindError: (error) =>
      set((state) => {
        if (state.rewind) state.rewind.error = error
      }),
    markRewindConfirmed: () =>
      set((state) => {
        if (state.rewind) state.rewind.confirmed = true
      }),
    closeRewind: () =>
      set((state) => {
        state.rewind = null
        if (!state.proposal) state.deferred = false
      }),
    setDeferred: (deferred) =>
      set((state) => {
        state.deferred = deferred && (state.proposal !== null || state.rewind !== null)
      }),
    reset: () => set(() => ({ ...initial })),
  })),
)

/** The engine runs `/diff`, `/commit` and sends `changes`. */
export function hasGitViews(features: readonly string[]): boolean {
  return features.includes(FEATURE_GIT_VIEWS)
}

/** The engine runs `/rewind`. */
export function hasCheckpoints(features: readonly string[]): boolean {
  return features.includes(FEATURE_CHECKPOINTS)
}

/** A git panel takes the composer's place (and the keyboard). */
export function isGitPanelOpen(
  state: Pick<GitViewsState, 'changesPanelOpen' | 'proposal' | 'rewind'> & {
    deferred?: boolean
  },
): boolean {
  if (state.changesPanelOpen) return true
  return !state.deferred && (state.proposal !== null || state.rewind !== null)
}

// ------------------------------------------------------------ pure helpers

/** `+120 -4` for files, summed. */
export function lineCounts(files: readonly Pick<ChangedFile, 'added' | 'removed'>[]): {
  added: number
  removed: number
} {
  let added = 0
  let removed = 0
  for (const file of files) {
    added += file.added
    removed += file.removed
  }
  return { added, removed }
}

const plural = (count: number, one: string, many = `${one}s`) =>
  `${count} ${count === 1 ? one : many}`

/** `3 files changed +120 -4`; empty when nothing changed. */
export function formatChangesSummary(files: readonly ChangedFile[]): string {
  if (files.length === 0) return ''
  const { added, removed } = lineCounts(files)
  return `${plural(files.length, 'file')} changed +${added} -${removed}`
}

/** One column for a file's status: A created, M modified, D deleted, R renamed. */
export function statusMark(status: string): string {
  switch (status) {
    case 'created':
    case 'added':
    case 'untracked':
      return 'A'
    case 'deleted':
      return 'D'
    case 'renamed':
      return 'R'
    case 'modified':
      return 'M'
    default:
      return status ? status[0]!.toUpperCase() : '?'
  }
}

export const REWIND_MODES: readonly { mode: RewindMode; label: string; detail: string }[] = [
  {
    mode: 'code_and_chat',
    label: 'Code and chat',
    detail: 'put the files back and drop the later prompts',
  },
  { mode: 'chat_only', label: 'Chat only', detail: 'drop the later prompts, keep the files' },
  { mode: 'code_only', label: 'Code only', detail: 'put the files back, keep the conversation' },
]

export function rewindModeLabel(mode: RewindMode | string): string {
  return REWIND_MODES.find((entry) => entry.mode === mode)?.label ?? mode
}

/**
 * A prompt the engine took, so it has a checkpoint: a user message marked
 * `sentToEngine` (an answer to the agent's question never is). A chat saved
 * before the marks has none on its messages: there a user message counts
 * when it is not a slash command or an answer (see use-exit-handler's
 * isPrompt).
 */
export function isPromptMessage(
  message: Pick<ChatMessage, 'variant' | 'content' | 'metadata'>,
): boolean {
  if (message.variant !== 'user' || message.metadata?.answersQuestion) return false
  const sent = message.metadata?.sentToEngine
  if (sent !== undefined) return sent
  return !message.content.trim().startsWith('/')
}

/**
 * The transcript after a rewind took `removedTurns` prompts out of the
 * conversation: the last `removedTurns` prompts go, with everything after
 * them. With fewer prompts in the transcript (an older chat), every prompt
 * goes from the first one on.
 */
export function dropRewoundTurns(messages: ChatMessage[], removedTurns: number): ChatMessage[] {
  if (removedTurns <= 0) return messages
  const prompts: number[] = []
  messages.forEach((message, index) => {
    if (isPromptMessage(message)) prompts.push(index)
  })
  if (prompts.length === 0) return messages
  const cut = prompts[Math.max(0, prompts.length - removedTurns)]!
  return messages.slice(0, cut)
}
