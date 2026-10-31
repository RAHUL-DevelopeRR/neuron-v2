import { afterEach, beforeEach, describe, expect, test } from 'bun:test'

import { flushExchanges, installFakeGitEngine } from '../../__tests__/helpers/git-views-engine'
import { footerParts } from '../../components/engine-footer'
import { useChatStore } from '../../state/chat-store'
import { useEngineStore } from '../../state/engine-store'
import { useGitViewsStore } from '../../state/git-views-store'
import { getUserMessage } from '../../utils/message-history'
import { handleEngineEvent, setEngineTurnActive } from '../engine-session'
import {
  acceptCommitProposal,
  cancelCommitProposal,
  confirmRewind,
  editCommitProposal,
  previewRewind,
} from '../git-views'
import { setNeuronEngineClientForTesting } from '../neuron-engine-client'

import type { CheckpointInfo, StatusEvent } from '../protocol'
import type { ChatMessage, GitViewContentBlock } from '../../types/chat'

const CHECKPOINTS: CheckpointInfo[] = [
  {
    id: 3,
    created_at_ms: Date.now(),
    prompt_preview: 'add tests',
    files: ['src/a.test.ts'],
    shell_commands: ['bun test'],
  },
  {
    id: 2,
    created_at_ms: Date.now() - 60_000,
    prompt_preview: 'fix the parser',
    files: ['src/parser.ts', 'src/lexer.ts'],
    shell_commands: [],
  },
]

const STATUS: StatusEvent = {
  type: 'status',
  model: 'glm-5.2',
  permission_mode: 'workspace-write',
  session_id: 'session-1',
  cwd: '/work',
  workspace_trusted: true,
  plan_mode: false,
  context_percent: 4,
  git: { branch: 'main', changed: 3, staged: 0, unstaged: 2, untracked: 1, conflicted: 0 },
}

const messages = () => useChatStore.getState().messages
const gitViews = () => useGitViewsStore.getState()
const gitBlocks = () =>
  messages().flatMap((m) =>
    (m.blocks ?? []).filter((b): b is GitViewContentBlock => b.type === 'git-view'),
  )
const lines = () => messages().map((m) => `${m.variant}: ${m.content}`)

beforeEach(() => {
  useEngineStore.getState().reset()
  useChatStore.getState().reset()
  useGitViewsStore.getState().reset()
  setEngineTurnActive(false)
})
afterEach(() => setNeuronEngineClientForTesting(null))

describe('git-view events', () => {
  test('ready records the features, and the engine store still gets it', () => {
    handleEngineEvent({
      type: 'ready',
      protocol: 1,
      version: '6.4.0',
      features: ['capabilities', 'git_views', 'checkpoints'],
    })
    expect(gitViews().features).toEqual(['capabilities', 'git_views', 'checkpoints'])
    expect(useEngineStore.getState().connection).toBe('ready')
  })

  test('changes feed the tracker, also mid-turn, and add nothing to the chat', () => {
    setEngineTurnActive(true, 'ai-1')
    handleEngineEvent({
      type: 'changes',
      files: [{ path: 'src/a.ts', status: 'modified', added: 3, removed: 1 }],
    })
    expect(gitViews().changes).toEqual([
      { path: 'src/a.ts', status: 'modified', added: 3, removed: 1 },
    ])
    expect(messages()).toEqual([])
  })

  test('a mid-turn status updates the footer git counts', () => {
    setEngineTurnActive(true, 'ai-1')
    handleEngineEvent(STATUS)
    const footer = footerParts(useEngineStore.getState().status, null).map((p) => p.text)
    expect(footer).toContain('main ~2 ?1')
  })

  test('a diff becomes a transcript cell', () => {
    handleEngineEvent({
      type: 'diff',
      title: 'Changes against HEAD on main',
      files: [
        {
          path: 'src/a.ts',
          status: 'modified',
          added: 1,
          removed: 1,
          patch: '@@ -1 +1 @@\n-old\n+new',
          binary: false,
        },
      ],
      truncated: false,
    })
    expect(gitBlocks()).toHaveLength(1)
    const view = gitBlocks()[0]!.view
    expect(view.kind).toBe('diff')
    expect(view.kind === 'diff' && view.files[0]!.path).toBe('src/a.ts')
    expect(messages()[0]!.id.startsWith('sys-')).toBe(true)
  })

  test('a proposal opens the card; one with no files says there is nothing to commit', () => {
    handleEngineEvent({
      type: 'commit_proposal',
      message: 'fix: parser',
      branch: 'main',
      files: [],
      stage_all: true,
    })
    expect(gitViews().proposal).toBeNull()
    expect(lines()).toEqual(['ai: Nothing to commit.'])
    handleEngineEvent({
      type: 'commit_proposal',
      message: 'fix: parser',
      branch: 'main',
      files: [{ path: 'src/parser.ts', status: 'modified', added: 2, removed: 1 }],
      stage_all: true,
    })
    expect(gitViews().proposal?.message).toBe('fix: parser')
  })

  test('no checkpoints is a chat line, not an empty picker', () => {
    handleEngineEvent({ type: 'checkpoints', items: [] })
    expect(gitViews().rewind).toBeNull()
    expect(lines()[0]).toContain('No checkpoints yet')
  })

  test('without the features, the engine answers in notices, shown as before', () => {
    handleEngineEvent({ type: 'ready', protocol: 1, version: '6.3.0' })
    handleEngineEvent({ type: 'notice', message: 'diff --git a/a.ts b/a.ts\n+new' })
    expect(gitViews().features).toEqual([])
    expect(lines()).toEqual(['ai: diff --git a/a.ts b/a.ts\n+new'])
  })
})

describe('the commit card', () => {
  const proposal = {
    message: 'feat(parser): handle empty input\n\nReturns an empty tree.',
    branch: 'main',
    files: [{ path: 'src/parser.ts', status: 'modified', added: 4, removed: 1 }],
    stage_all: false,
  }

  test('Enter commits with the drafted message', async () => {
    const { sent, emit } = installFakeGitEngine()
    emit({ type: 'commit_proposal', ...proposal })
    acceptCommitProposal()
    await flushExchanges()
    expect(sent).toEqual([{ type: 'commit', message: proposal.message }])
    expect(gitViews().proposal).toBeNull()
    emit(
      { type: 'notice', message: 'Committed abc1234 feat(parser): handle empty input' },
      STATUS,
      { type: 'turn_finished' },
    )
    expect(lines()).toContain('ai: Committed abc1234 feat(parser): handle empty input')
  })

  test('e puts /commit <message> in the input and sends nothing', async () => {
    const { sent, emit } = installFakeGitEngine()
    emit({ type: 'commit_proposal', ...proposal })
    editCommitProposal()
    await flushExchanges()
    expect(sent).toEqual([])
    expect(gitViews().proposal).toBeNull()
    expect(useChatStore.getState().inputValue).toBe(`/commit ${proposal.message}`)
  })

  test('Esc cancels without a commit', async () => {
    const { sent, emit } = installFakeGitEngine()
    emit({ type: 'commit_proposal', ...proposal })
    cancelCommitProposal()
    await flushExchanges()
    expect(sent).toEqual([])
    expect(gitViews().proposal).toBeNull()
  })
})

describe('the rewind flow', () => {
  function transcript(): ChatMessage[] {
    return [
      { ...getUserMessage('fix the parser'), id: 'u1' },
      { id: 'a1', variant: 'ai', content: 'fixed', timestamp: '' },
      { ...getUserMessage('add tests'), id: 'u2' },
      { id: 'a2', variant: 'ai', content: 'added', timestamp: '' },
      { ...getUserMessage('/rewind'), id: 'u3' },
    ]
  }

  test('pick, mode and a dry run, then the confirmed rewind truncates the chat', async () => {
    const { sent, emit } = installFakeGitEngine()
    useChatStore.getState().setMessages(transcript())
    emit({ type: 'checkpoints', items: CHECKPOINTS }, { type: 'turn_finished' })

    // The older checkpoint, then "code and chat" and its dry run.
    gitViews().pickCheckpoint(CHECKPOINTS[1]!)
    gitViews().pickMode('code_and_chat')
    previewRewind()
    await flushExchanges()
    expect(sent).toEqual([{ type: 'rewind', checkpoint: 2, mode: 'code_and_chat', dry_run: true }])

    const answer = {
      type: 'rewound' as const,
      checkpoint: 2,
      mode: 'code_and_chat' as const,
      restored: ['src/parser.ts'],
      removed: ['src/a.test.ts'],
      not_restored: [{ path: 'src/lexer.ts', reason: 'changed outside the agent' }],
      shell_commands: ['bun test'],
      removed_turns: 2,
      prompt: 'fix the parser',
    }
    emit({ ...answer, dry_run: true }, { type: 'turn_finished' })
    expect(gitViews().rewind?.preview?.not_restored[0]?.path).toBe('src/lexer.ts')
    // Nothing changed yet.
    expect(messages()).toHaveLength(5)

    confirmRewind()
    await flushExchanges()
    expect(sent[1]).toEqual({ type: 'rewind', checkpoint: 2, mode: 'code_and_chat', dry_run: false })
    expect(gitViews().rewind?.confirmed).toBe(true)

    emit(
      { ...answer, dry_run: false },
      { type: 'changes', files: [] },
      STATUS,
      { type: 'turn_finished' },
    )
    expect(gitViews().rewind).toBeNull()
    expect(gitViews().changes).toEqual([])
    // Both prompts went, with everything after them; the result cell follows.
    expect(messages()).toHaveLength(1)
    expect(gitBlocks()).toHaveLength(1)
    expect(gitBlocks()[0]!.view.kind).toBe('rewound')
    expect(useChatStore.getState().inputValue).toBe('fix the parser')
  })

  test('a chat-only rewind of the last prompt keeps the earlier ones', async () => {
    const { emit } = installFakeGitEngine()
    useChatStore.getState().setMessages(transcript())
    emit({
      type: 'rewound',
      checkpoint: 3,
      mode: 'chat_only',
      dry_run: false,
      restored: [],
      removed: [],
      not_restored: [],
      shell_commands: [],
      removed_turns: 1,
      prompt: 'add tests',
    })
    expect(messages().map((m) => m.id).slice(0, 2)).toEqual(['u1', 'a1'])
    expect(messages()).toHaveLength(3)
    expect(useChatStore.getState().inputValue).toBe('add tests')
  })

  test('a code-only rewind keeps the chat and the input', async () => {
    const { emit } = installFakeGitEngine()
    useChatStore.getState().setMessages(transcript())
    useChatStore.getState().setInputValue({ text: 'draft', cursorPosition: 5, lastEditDueToNav: false })
    emit({
      type: 'rewound',
      checkpoint: 3,
      mode: 'code_only',
      dry_run: false,
      restored: ['src/a.test.ts'],
      removed: [],
      not_restored: [],
      shell_commands: [],
      removed_turns: 0,
      prompt: null,
    })
    expect(messages()).toHaveLength(6)
    expect(useChatStore.getState().inputValue).toBe('draft')
  })

  test('a dry run that fails says why in the picker', async () => {
    const { emit } = installFakeGitEngine()
    emit({ type: 'checkpoints', items: CHECKPOINTS }, { type: 'turn_finished' })
    gitViews().pickCheckpoint(CHECKPOINTS[0]!)
    gitViews().pickMode('code_only')
    previewRewind()
    await flushExchanges()
    emit({ type: 'error', message: 'checkpoint 3 is gone' }, { type: 'turn_finished' })
    await flushExchanges()
    expect(gitViews().rewind?.error).toBe('checkpoint 3 is gone')
    expect(gitViews().rewind?.preview).toBeNull()
  })

  test('a preview that arrives after Esc went back is dropped', async () => {
    const { emit } = installFakeGitEngine()
    emit({ type: 'checkpoints', items: CHECKPOINTS }, { type: 'turn_finished' })
    gitViews().pickCheckpoint(CHECKPOINTS[0]!)
    gitViews().pickMode('code_and_chat')
    previewRewind()
    // Esc went back to the modes.
    gitViews().pickMode(null)
    emit({
      type: 'rewound',
      checkpoint: 3,
      mode: 'code_and_chat',
      dry_run: true,
      restored: [],
      removed: [],
      not_restored: [],
      shell_commands: [],
      removed_turns: 1,
      prompt: 'add tests',
    })
    expect(gitViews().rewind?.preview).toBeNull()
    expect(gitBlocks()).toEqual([])
  })
})
