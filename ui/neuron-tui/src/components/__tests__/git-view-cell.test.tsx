import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import { afterEach, beforeAll, describe, expect, test } from 'bun:test'
import React from 'react'

import { serializeConversation } from '../../commands/copy-conversation'
import { initializeThemeStore } from '../../hooks/use-theme'
import { useMessageBlockStore } from '../../state/message-block-store'
import { gitViewMarkdown, splitHunks } from '../../utils/git-view-text'
import { chatThemes, createMarkdownPalette } from '../../utils/theme-system'
import { GitViewCell } from '../git-view-cell'
import { MessageWithAgents } from '../message-with-agents'

import type { DiffFile, RewoundEvent } from '../../engine/protocol'
import type { GitView } from '../../types/git-view'

let cleanup: (() => void) | undefined
beforeAll(() => initializeThemeStore())
afterEach(() => {
  cleanup?.()
  cleanup = undefined
})

async function mount(node: React.ReactNode, height = 24) {
  const setup = await createTestRenderer({ width: 90, height, kittyKeyboard: true })
  const root = createRoot(setup.renderer)
  cleanup = () => {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
  flushSync(() => root.render(<>{node}</>))
  await setup.renderOnce()
  await new Promise((resolve) => setTimeout(resolve, 20))
  await setup.renderOnce()
  return { ...setup, frame: () => setup.captureCharFrame() }
}

const PATCH = [
  '@@ -1,3 +1,4 @@ fn parse',
  ' fn parse(input: &str) -> Tree {',
  '+    if input.is_empty() { return Tree::empty() }',
  '-    let tree = old(input);',
  '+    let tree = new(input);',
  '@@ -20 +21 @@',
  '-// end',
  '+// the end',
].join('\n')

const FILES: DiffFile[] = [
  { path: 'src/parser.rs', status: 'modified', added: 3, removed: 2, patch: PATCH, binary: false },
  { path: 'assets/logo.png', status: 'created', added: 0, removed: 0, patch: '', binary: true },
  { path: 'src/big.rs', status: 'modified', added: 400, removed: 0, patch: '', binary: false },
]

describe('the diff cell', () => {
  test('splits a patch into hunks at its @@ lines', () => {
    const hunks = splitHunks(PATCH)
    expect(hunks.map((hunk) => hunk.header)).toEqual(['@@ -1,3 +1,4 @@ fn parse', '@@ -20 +21 @@'])
    expect(hunks[1]!.text).toBe('@@ -20 +21 @@\n-// end\n+// the end')
  })

  test('shows each file with its status, counts and hunks, and the notes', async () => {
    const view: GitView = {
      kind: 'diff',
      title: 'Changes against HEAD on main',
      files: FILES,
      truncated: true,
    }
    const ui = await mount(<GitViewCell view={view} />, 30)
    const frame = ui.frame()
    expect(frame).toContain('Changes against HEAD on main · 3 files +403 -2')
    expect(frame).toContain('M src/parser.rs  +3 -2')
    expect(frame).toContain('@@ -1,3 +1,4 @@ fn parse')
    // The first context line keeps its indent.
    expect(frame).toContain('   fn parse(input: &str) -> Tree {')
    expect(frame).toContain('+    if input.is_empty() { return Tree::empty() }')
    expect(frame).toContain('-    let tree = old(input);')
    expect(frame).toContain('@@ -20 +21 @@')
    expect(frame).toContain('A assets/logo.png  +0 -0  (binary file)')
    expect(frame).toContain('src/big.rs  +400 -0  (diff left out to keep this short)')
    expect(frame).toContain('Some files or hunks were left out')
  })

  test("a rewind's result lists what it did", async () => {
    const result: Omit<RewoundEvent, 'type'> = {
      checkpoint: 2,
      mode: 'code_and_chat',
      dry_run: false,
      restored: ['src/parser.rs'],
      removed: ['src/new.rs'],
      not_restored: [{ path: 'src/lexer.rs', reason: 'changed outside the agent' }],
      shell_commands: ['cargo test'],
      removed_turns: 2,
      prompt: 'fix it',
    }
    const ui = await mount(<GitViewCell view={{ kind: 'rewound', ...result }} />)
    const frame = ui.frame()
    expect(frame).toContain('Rewound: checkpoint 2, code and chat')
    expect(frame).toContain('Restored 1 file: src/parser.rs')
    expect(frame).toContain('Removed 1 file that did not exist then: src/new.rs')
    expect(frame).toContain('Not restored src/lexer.rs: changed outside the agent')
    expect(frame).toContain('1 shell command ran since then')
    expect(frame).toContain('Took 2 prompts out of the conversation; the prompt is back in the input.')
  })

  test('/copy and /export get the diff as Markdown', () => {
    const markdown = gitViewMarkdown({
      kind: 'diff',
      title: 'Changes against HEAD on main',
      files: FILES,
      truncated: true,
    })
    expect(markdown).toContain('**Changes against HEAD on main** (3 files +403 -2)')
    expect(markdown).toContain('M src/parser.rs +3 -2\n\n```diff\n@@ -1,3 +1,4 @@ fn parse')
    expect(markdown).toContain('A assets/logo.png +0 -0 (binary file)')
    expect(markdown).toContain('_Some files or hunks were left out._')
    const copied = serializeConversation([
      {
        id: 'sys-git-1',
        variant: 'ai',
        content: '',
        timestamp: '',
        blocks: [{ type: 'git-view', view: { kind: 'diff', title: 'Diff', files: FILES, truncated: false } }],
      },
    ]).text
    expect(copied).toContain('+    if input.is_empty() { return Tree::empty() }')
  })

  test('a diff message shows in the transcript', async () => {
    useMessageBlockStore.setState({
      context: {
        readOnly: false,
        theme: chatThemes.dark,
        markdownPalette: createMarkdownPalette(chatThemes.dark),
        messageTree: new Map(),
        isWaitingForResponse: false,
        timerStartTime: null,
        availableWidth: 88,
      },
    })
    const message = {
      id: 'sys-git-1',
      variant: 'ai' as const,
      content: '',
      timestamp: '',
      blocks: [
        {
          type: 'git-view' as const,
          view: { kind: 'diff' as const, title: 'Changes in src/parser.rs', files: FILES.slice(0, 1), truncated: false },
        },
      ],
    }
    const ui = await mount(
      <MessageWithAgents message={message} depth={0} isLastMessage={true} availableWidth={88} />,
    )
    const frame = ui.frame()
    expect(frame).toContain('Changes in src/parser.rs')
    expect(frame).toContain('+    let tree = new(input);')
    useMessageBlockStore.getState().reset()
  })
})

