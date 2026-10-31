import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import { afterEach, beforeAll, beforeEach, describe, expect, test } from 'bun:test'
import React from 'react'
import { useShallow } from 'zustand/react/shallow'

import { flushExchanges, installFakeGitEngine } from '../../__tests__/helpers/git-views-engine'
import { setNeuronEngineClientForTesting } from '../../engine/neuron-engine-client'
import { initializeThemeStore } from '../../hooks/use-theme'
import { useChatStore } from '../../state/chat-store'
import { useGitViewsStore } from '../../state/git-views-store'
import { ChangesPanel, ChangesSummary } from '../changes-panel'
import { COMMIT_KEY_GUARD_MS, CommitProposalCard, resolveCommitProposalKey } from '../commit-proposal'
import { RewindPicker, resolveRewindKey } from '../rewind-picker'

import type { CheckpointInfo, RewoundEvent } from '../../engine/protocol'

let cleanup: (() => void) | undefined
beforeAll(() => initializeThemeStore())
beforeEach(() => {
  useGitViewsStore.getState().reset()
  useChatStore.getState().reset()
})
afterEach(() => {
  cleanup?.()
  cleanup = undefined
  setNeuronEngineClientForTesting(null)
})

async function mount(node: React.ReactNode, height = 24) {
  const setup = await createTestRenderer({ width: 90, height, kittyKeyboard: true })
  const root = createRoot(setup.renderer)
  cleanup = () => {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
  flushSync(() => root.render(<>{node}</>))
  const settle = async (ms = 20) => {
    await setup.renderOnce()
    await new Promise((resolve) => setTimeout(resolve, ms))
    await setup.renderOnce()
  }
  await settle()
  return { ...setup, settle, frame: () => setup.captureCharFrame() }
}

/** The git panels as chat.tsx draws them, in the composer's place. */
function GitPanels() {
  const { proposal, rewind, changesPanelOpen, changes } = useGitViewsStore(
    useShallow((state) => ({
      proposal: state.proposal,
      rewind: state.rewind,
      changesPanelOpen: state.changesPanelOpen,
      changes: state.changes,
    })),
  )
  if (proposal) return <CommitProposalCard proposal={proposal} width={90} />
  if (rewind) return <RewindPicker flow={rewind} width={90} />
  if (changesPanelOpen) return <ChangesPanel files={changes} width={90} />
  return (
    <box style={{ flexDirection: 'column' }}>
      <ChangesSummary files={changes} />
      <text>Composer restored</text>
    </box>
  )
}

describe('the commit card', () => {
  const proposal = {
    type: 'commit_proposal' as const,
    message: 'feat(parser): handle empty input\n\nReturns an empty tree.',
    branch: 'main',
    files: [
      { path: 'src/parser.rs', status: 'modified', added: 3, removed: 2 },
      { path: 'src/empty.rs', status: 'created', added: 10, removed: 0 },
    ],
    stage_all: true,
  }

  test('keys: Enter commits, e edits, Esc and Ctrl-C cancel; none before the guard', () => {
    const key = (name: string, ctrl = false) => ({ name, ctrl, meta: false })
    expect(resolveCommitProposalKey(key('return'), { armed: true })).toBe('commit')
    expect(resolveCommitProposalKey(key('e'), { armed: true })).toBe('edit')
    expect(resolveCommitProposalKey(key('escape'), { armed: true })).toBe('cancel')
    expect(resolveCommitProposalKey(key('c', true), { armed: false })).toBe('cancel')
    expect(resolveCommitProposalKey(key('return'), { armed: false })).toBe('none')
    expect(resolveCommitProposalKey(key('e'), { armed: false })).toBe('none')
    expect(resolveCommitProposalKey(key('y'), { armed: true })).toBe('none')
  })

  test('shows the branch, files and message; Enter commits once the guard is over', async () => {
    const { sent, emit } = installFakeGitEngine()
    const ui = await mount(<GitPanels />)
    emit(proposal)
    await ui.settle()
    const frame = ui.frame()
    expect(frame).toContain('On main · 2 files +13 -2')
    expect(frame).toContain('takes every change')
    expect(frame).toContain('M src/parser.rs  +3 -2')
    expect(frame).toContain('A src/empty.rs  +10 -0')
    expect(frame).toContain('feat(parser): handle empty input')
    expect(frame).toContain('Returns an empty tree.')
    expect(frame).toContain('enter commit')

    // The Enter that sent /commit, still held: nothing happens.
    ui.mockInput.pressEnter()
    await ui.settle()
    await flushExchanges()
    expect(sent).toEqual([])

    await ui.settle(COMMIT_KEY_GUARD_MS + 50)
    ui.mockInput.pressEnter()
    await ui.settle()
    await flushExchanges()
    expect(sent).toEqual([{ type: 'commit', message: proposal.message }])
    expect(ui.frame()).toContain('Composer restored')
  })

  test('e puts the message in the input; Esc cancels; neither commits', async () => {
    const { sent, emit } = installFakeGitEngine()
    const ui = await mount(<GitPanels />)
    emit(proposal)
    await ui.settle(COMMIT_KEY_GUARD_MS + 50)
    ui.mockInput.pressKey('e')
    await ui.settle()
    expect(useChatStore.getState().inputValue).toBe(`/commit ${proposal.message}`)
    expect(ui.frame()).toContain('Composer restored')

    emit(proposal)
    await ui.settle()
    ui.mockInput.pressEscape()
    await ui.settle()
    await flushExchanges()
    expect(ui.frame()).toContain('Composer restored')
    expect(sent).toEqual([])
  })
})

describe('the rewind picker', () => {
  const now = Date.now()
  const checkpoints: CheckpointInfo[] = [
    {
      id: 3,
      created_at_ms: now,
      prompt_preview: 'add tests for the parser',
      files: ['src/parser.test.rs'],
      shell_commands: ['cargo test'],
    },
    {
      id: 2,
      created_at_ms: now - 60_000,
      prompt_preview: 'fix the parser',
      files: ['src/parser.rs', 'src/lexer.rs'],
      shell_commands: [],
    },
  ]
  const dryRun: RewoundEvent = {
    type: 'rewound',
    checkpoint: 2,
    mode: 'code_and_chat',
    dry_run: true,
    restored: ['src/parser.rs'],
    removed: ['src/parser.test.rs'],
    not_restored: [{ path: 'src/lexer.rs', reason: 'changed outside the agent' }],
    shell_commands: ['cargo test'],
    removed_turns: 2,
    prompt: 'fix the parser',
  }

  test('keys: arrows move and Enter picks; the preview needs the guard; Esc goes back', () => {
    const key = (name: string) => ({ name, ctrl: false, meta: false })
    expect(resolveRewindKey('pick', key('down'))).toEqual({ type: 'move', delta: 1 })
    expect(resolveRewindKey('mode', key('return'))).toEqual({ type: 'select' })
    expect(resolveRewindKey('preview', key('return'), { armed: false })).toEqual({ type: 'none' })
    expect(resolveRewindKey('preview', key('return'), { armed: true })).toEqual({ type: 'select' })
    expect(resolveRewindKey('preview', key('down'))).toEqual({ type: 'none' })
    expect(resolveRewindKey('running', key('return'))).toEqual({ type: 'none' })
    expect(resolveRewindKey('running', key('escape'))).toEqual({ type: 'back' })
  })

  test('list, mode, dry-run preview, then the confirmed rewind', async () => {
    const { sent, emit } = installFakeGitEngine()
    const ui = await mount(<GitPanels />)
    emit({ type: 'checkpoints', items: checkpoints }, { type: 'turn_finished' })
    await ui.settle()
    let frame = ui.frame()
    expect(frame).toContain('add tests for the parser')
    expect(frame).toContain('1 file  1 shell command (not undone)')
    expect(frame).toContain('fix the parser')
    expect(frame).toContain('2 files')

    ui.mockInput.pressArrow('down')
    await ui.settle()
    ui.mockInput.pressEnter()
    await ui.settle()
    frame = ui.frame()
    expect(frame).toContain('Back to before')
    expect(frame).toContain('Code and chat')
    expect(frame).toContain('Chat only')
    expect(frame).toContain('Code only')

    ui.mockInput.pressEnter()
    await ui.settle()
    await flushExchanges()
    expect(sent).toEqual([{ type: 'rewind', checkpoint: 2, mode: 'code_and_chat', dry_run: true }])
    expect(ui.frame()).toContain('Checking what this rewind would do')

    emit(dryRun, { type: 'turn_finished' })
    await ui.settle()
    frame = ui.frame()
    expect(frame).toContain('Would restore 1 file: src/parser.rs')
    expect(frame).toContain('Cannot restore src/lexer.rs: changed outside the agent')
    expect(frame).toContain('1 shell command ran since then')
    expect(frame).toContain('Would take 2 prompts out of the conversation.')
    expect(frame).toContain('enter rewind')

    // Too soon after the preview: ignored.
    ui.mockInput.pressEnter()
    await ui.settle()
    await flushExchanges()
    expect(sent).toHaveLength(1)

    await ui.settle(350)
    ui.mockInput.pressEnter()
    await ui.settle()
    await flushExchanges()
    expect(sent[1]).toEqual({ type: 'rewind', checkpoint: 2, mode: 'code_and_chat', dry_run: false })
    expect(ui.frame()).toContain('Rewinding')

    emit({ ...dryRun, dry_run: false }, { type: 'changes', files: [] }, { type: 'turn_finished' })
    await ui.settle()
    expect(ui.frame()).toContain('Composer restored')
    expect(useChatStore.getState().inputValue).toBe('fix the parser')
  })

  test('Esc goes back a step, and closes from the list', async () => {
    const { sent, emit } = installFakeGitEngine()
    const ui = await mount(<GitPanels />)
    emit({ type: 'checkpoints', items: checkpoints }, { type: 'turn_finished' })
    await ui.settle()
    ui.mockInput.pressEnter()
    await ui.settle()
    expect(ui.frame()).toContain('Chat only')
    ui.mockInput.pressEscape()
    await ui.settle()
    expect(ui.frame()).toContain('enter pick')
    ui.mockInput.pressEscape()
    await ui.settle()
    expect(ui.frame()).toContain('Composer restored')
    await flushExchanges()
    expect(sent).toEqual([])
  })
})

describe('the changes tracker', () => {
  const changes = [
    { path: 'src/parser.rs', status: 'modified', added: 100, removed: 4 },
    { path: 'src/empty.rs', status: 'created', added: 20, removed: 0 },
    { path: 'old.rs', status: 'deleted', added: 0, removed: 0 },
  ]

  test('a summary line shows above the input while files changed', async () => {
    const { emit } = installFakeGitEngine()
    const ui = await mount(<GitPanels />)
    expect(ui.frame()).not.toContain('changed')
    emit({ type: 'changes', files: changes })
    await ui.settle()
    expect(ui.frame()).toContain('3 files changed +120 -4 · /changes')
  })

  test('the panel lists the files; Enter shows the focused one, a everything', async () => {
    const { sent, emit } = installFakeGitEngine()
    emit({ type: 'changes', files: changes })
    useGitViewsStore.getState().openChangesPanel()
    const ui = await mount(<GitPanels />)
    const frame = ui.frame()
    expect(frame).toContain('Changes · 3 files changed +120 -4')
    expect(frame).toContain('› M src/parser.rs  +100 -4')
    expect(frame).toContain('A src/empty.rs  +20 -0')
    expect(frame).toContain('D old.rs  +0 -0')

    ui.mockInput.pressArrow('down')
    await ui.settle()
    ui.mockInput.pressEnter()
    await ui.settle()
    await flushExchanges()
    expect(sent).toEqual([{ type: 'slash', command: 'diff', args: 'src/empty.rs' }])
    expect(useChatStore.getState().messages.map((m) => m.content)).toEqual(['/diff src/empty.rs'])
    expect(ui.frame()).toContain('Composer restored')

    emit({ type: 'turn_finished' })
    useGitViewsStore.getState().openChangesPanel()
    await ui.settle()
    ui.mockInput.pressKey('a')
    await ui.settle()
    await flushExchanges()
    expect(sent[1]).toEqual({ type: 'slash', command: 'diff', args: '' })
  })

  test('an empty panel says so', async () => {
    useGitViewsStore.getState().openChangesPanel()
    const ui = await mount(<GitPanels />)
    expect(ui.frame()).toContain('No files changed in this session yet.')
    ui.mockInput.pressEscape()
    await ui.settle()
    expect(ui.frame()).toContain('Composer restored')
  })
})
