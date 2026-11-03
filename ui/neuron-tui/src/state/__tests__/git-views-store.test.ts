import { beforeEach, describe, expect, test } from 'bun:test'

import {
  dropRewoundTurns,
  formatChangesSummary,
  isGitPanelOpen,
  rewindStep,
  statusMark,
  useGitViewsStore,
} from '../git-views-store'

import type { CheckpointInfo } from '../../engine/protocol'
import type { ChatMessage } from '../../types/chat'

let next = 0
const message = (variant: ChatMessage['variant'], content: string): ChatMessage => ({
  id: `m${++next}`,
  variant,
  content,
  timestamp: '12:00',
})

const CHECKPOINT: CheckpointInfo = {
  id: 2,
  created_at_ms: 0,
  prompt_preview: 'fix the parser',
  files: ['src/parser.ts'],
  shell_commands: [],
}

beforeEach(() => useGitViewsStore.getState().reset())

describe('changes summary', () => {
  test('counts files and lines, and says nothing when nothing changed', () => {
    expect(formatChangesSummary([])).toBe('')
    expect(
      formatChangesSummary([
        { path: 'a.ts', status: 'modified', added: 100, removed: 4 },
        { path: 'b.ts', status: 'created', added: 20, removed: 0 },
        { path: 'c.ts', status: 'deleted', added: 0, removed: 0 },
      ]),
    ).toBe('3 files changed +120 -4')
    expect(
      formatChangesSummary([{ path: 'a.ts', status: 'modified', added: 1, removed: 1 }]),
    ).toBe('1 file changed +1 -1')
  })

  test('marks each status with one letter', () => {
    expect(['created', 'modified', 'deleted', 'renamed'].map(statusMark)).toEqual([
      'A',
      'M',
      'D',
      'R',
    ])
    expect(statusMark('copied')).toBe('C')
  })
})

describe('dropRewoundTurns', () => {
  const transcript = [
    message('user', 'first prompt'),
    message('ai', 'first answer'),
    message('user', '/status'),
    message('ai', 'status report'),
    message('user', 'second prompt'),
    message('ai', 'second answer'),
    message('user', 'third prompt'),
    message('ai', 'third answer'),
    message('user', '/rewind'),
  ]

  test('drops the last prompts with everything after them; slash commands are not prompts', () => {
    const kept = dropRewoundTurns(transcript, 2)
    expect(kept.map((m) => m.content)).toEqual([
      'first prompt',
      'first answer',
      '/status',
      'status report',
    ])
  })

  test('keeps the chat when no prompt was taken out', () => {
    expect(dropRewoundTurns(transcript, 0)).toBe(transcript)
  })

  test('drops from the first prompt when the engine took out more than the transcript has', () => {
    const kept = dropRewoundTurns([message('ai', 'notice'), ...transcript], 9)
    expect(kept.map((m) => m.content)).toEqual(['notice'])
  })

  test('leaves a transcript without prompts alone', () => {
    const notices = [message('ai', 'a'), message('user', '/diff')]
    expect(dropRewoundTurns(notices, 1)).toBe(notices)
  })

  test("an answer to the agent's question is not a prompt", () => {
    const answer = { ...message('user', 'use main'), metadata: { answersQuestion: true } }
    const kept = dropRewoundTurns(
      [
        message('user', 'first prompt'),
        message('ai', 'first answer'),
        message('user', 'second prompt'),
        message('ai', 'Question: which branch?'),
        answer,
        message('ai', 'second answer'),
        message('user', 'third prompt'),
        message('ai', 'third answer'),
      ],
      2,
    )
    expect(kept.map((m) => m.content)).toEqual(['first prompt', 'first answer'])
  })
})

describe('which user messages a rewind counts', () => {
  const sent = (content: string, sentToEngine: boolean): ChatMessage => ({
    ...message('user', content),
    metadata: { sentToEngine },
  })

  test('only prompts the engine took, once messages are marked', () => {
    const kept = dropRewoundTurns(
      [
        sent('first prompt', true),
        message('ai', 'first answer'),
        // Dropped while signed out, failed validation, or a failed image path.
        sent('signed out', false),
        message('ai', 'Sign in to Zero-X first: /login.'),
        sent('second prompt', true),
        message('ai', 'second answer'),
        sent('not an image', false),
        sent('third prompt', true),
        message('ai', 'third answer'),
      ],
      2,
    )
    expect(kept.map((m) => m.content)).toEqual([
      'first prompt',
      'first answer',
      'signed out',
      'Sign in to Zero-X first: /login.',
    ])
  })

  test('an answer is never a prompt, marked or not', () => {
    const answer: ChatMessage = {
      ...message('user', 'use main'),
      metadata: { answersQuestion: true, sentToEngine: false },
    }
    expect(
      dropRewoundTurns([sent('a', true), message('ai', 'Question?'), answer, sent('b', true)], 1).map(
        (m) => m.content,
      ),
    ).toEqual(['a', 'Question?', 'use main'])
  })

  test('a chat saved before the marks: the earlier prompts count as before', () => {
    const kept = dropRewoundTurns(
      [
        message('user', 'old prompt'),
        message('ai', 'old answer'),
        message('user', '/status'),
        sent('new prompt', true),
        message('ai', 'new answer'),
      ],
      2,
    )
    expect(kept).toEqual([])
  })
})

describe('the rewind flow', () => {
  test('goes pick, mode, preview, running', () => {
    const store = useGitViewsStore.getState()
    store.openRewind([CHECKPOINT])
    const flow = () => useGitViewsStore.getState().rewind!
    expect(rewindStep(flow())).toBe('pick')
    store.pickCheckpoint(CHECKPOINT)
    expect(rewindStep(flow())).toBe('mode')
    store.pickMode('code_only')
    expect(rewindStep(flow())).toBe('preview')
    store.setRewindError('no preview')
    expect(flow().error).toBe('no preview')
    // Picking again starts the preview afresh.
    store.pickMode('chat_only')
    expect(flow().error).toBeNull()
    store.markRewindConfirmed()
    expect(rewindStep(flow())).toBe('running')
    expect(isGitPanelOpen(useGitViewsStore.getState())).toBe(true)
    store.closeRewind()
    expect(isGitPanelOpen(useGitViewsStore.getState())).toBe(false)
  })

  test('a new engine forgets the last one', () => {
    const store = useGitViewsStore.getState()
    store.setChanges([{ path: 'a.ts', status: 'modified', added: 1, removed: 0 }])
    store.openChangesPanel()
    store.openRewind([CHECKPOINT])
    store.setFeatures(['git_views'])
    const state = useGitViewsStore.getState()
    expect(state.features).toEqual(['git_views'])
    expect(state.changes).toEqual([])
    expect(isGitPanelOpen(state)).toBe(false)
  })
})
