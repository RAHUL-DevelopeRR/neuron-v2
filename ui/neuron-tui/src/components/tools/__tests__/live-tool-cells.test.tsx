import { afterEach, beforeAll, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import { turnHarness } from '../../../engine/__tests__/turn-harness'
import { mapToolResult } from '../../../engine/tool-mapping'
import { resetLiveTurnStore } from '../../../state/live-turn-store'
import { initializeThemeStore, useThemeStore } from '../../../hooks/use-theme'
import { useChatStore } from '../../../state/chat-store'
import { updateToolBlockWithOutput } from '../../../utils/message-block-helpers'
import { createMarkdownPalette } from '../../../utils/theme-system'
import { ToolBranch } from '../../blocks/tool-branch'

import type { ToolContentBlock } from '../../../types/chat'

beforeAll(() => {
  initializeThemeStore()
})

afterEach(() => {
  useChatStore.getState().setStreamingAgents(() => new Set())
  resetLiveTurnStore()
})

/** The tool row as the chat draws it, as text; `running` while it has no result. */
async function frame(block: ToolContentBlock, running = false, height = 10): Promise<string> {
  useChatStore
    .getState()
    .setStreamingAgents(() => (running ? new Set([block.toolCallId]) : new Set()))
  const setup = await createTestRenderer({ width: 90, height })
  const root = createRoot(setup.renderer)
  flushSync(() => {
    root.render(
      <ToolBranch
        toolBlock={block}
        keyPrefix="test"
        availableWidth={90}
        onToggleCollapsed={() => {}}
        markdownPalette={createMarkdownPalette(useThemeStore.getState().theme)}
      />,
    )
  })
  try {
    await setup.renderOnce()
    return setup.captureCharFrame()
  } finally {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
}

/** The block once the engine's result for it came in. */
const withResult = (block: ToolContentBlock, name: string, output: string, isError = false) =>
  updateToolBlockWithOutput([block], {
    toolCallId: block.toolCallId,
    toolOutput: mapToolResult(name, output, isError),
    isError,
  })[0] as ToolContentBlock

describe('a file the model is writing', () => {
  const streaming: ToolContentBlock = {
    type: 'tool',
    toolCallId: 'w1',
    toolName: 'write_file',
    input: { path: '', content: '' },
    liveInput: {
      bytes: 12_595,
      lines: 412,
      target: 'grocery-shop/style.css',
      preview: '.cart {\n  display: grid;\n}\n.total { font-weight: 700; }',
    },
  }

  test('shows Write, the path, the size and the last lines while it streams', async () => {
    const shown = await frame(streaming, true)
    expect(shown).toContain('• Write grocery-shop/style.css  412 lines · 12.3 KB')
    expect(shown).toContain('.total { font-weight: 700; }')
    expect(shown).toContain('display: grid;')
  })

  test('before its first progress it already reads Write', async () => {
    const shown = await frame({ ...streaming, liveInput: { bytes: 0, lines: 0, target: null, preview: null } }, true)
    expect(shown).toContain('• Write')
    expect(shown).not.toContain('Edit')
  })

  test('complete, running, then created: one row that says so', async () => {
    const content = 'a\nb\nc\n'
    const complete: ToolContentBlock = { ...streaming, liveInput: undefined, input: { path: 'grocery-shop/style.css', content } }
    const running = await frame(complete, true)
    expect(running).toContain('• Write grocery-shop/style.css  3 lines · 6 B')
    expect(running).not.toContain('font-weight')

    const created = withResult(
      complete,
      'write_file',
      JSON.stringify({ type: 'create', filePath: 'grocery-shop/style.css', bytes: 6, lines: 3, summary: '' }),
    )
    const shown = await frame(created)
    expect(shown).toContain('• Create grocery-shop/style.css  3 lines · 6 B')
    // The whole file is not repeated as added lines.
    expect(shown).not.toContain('+ a')
  })

  test('an overwrite shows the diff the engine sends, and only that', async () => {
    const complete: ToolContentBlock = { ...streaming, liveInput: undefined, input: { path: 'a.css', content: 'x\n' } }
    const overwritten = withResult(
      complete,
      'write_file',
      JSON.stringify({ type: 'update', filePath: 'a.css', bytes: 2, lines: 1, diff: '@@ -1 +1 @@\n-old\n+x' }),
    )
    const shown = await frame(overwritten, false, 14)
    expect(shown).toContain('• Write a.css  1 line · 2 B')
    expect(shown).toContain('old')
  })

  test('an edit being written reads Edit with its size', async () => {
    const shown = await frame(
      {
        type: 'tool',
        toolCallId: 'e1',
        toolName: 'str_replace',
        input: { path: '', replacements: [{ old: '', new: '', allowMultiple: false }] },
        liveInput: { bytes: 2048, lines: 3, target: 'src/app.ts', preview: 'const a = 1' },
      },
      true,
    )
    expect(shown).toContain('• Edit src/app.ts  3 lines · 2 KB')
    expect(shown).toContain('const a = 1')
  })
})

describe('a command', () => {
  const base: ToolContentBlock = {
    type: 'tool',
    toolCallId: 'b1',
    toolName: 'run_terminal_command',
    input: { command: '' },
  }

  test('shows as soon as the model wrote it, then Running… until its result', async () => {
    const writing = await frame(
      { ...base, liveInput: { bytes: 20, lines: 1, target: 'npm test', preview: 'npm test' } },
      true,
    )
    expect(writing).toContain('$ npm test')
    expect(writing).not.toContain('Running…')

    const started: ToolContentBlock = { ...base, input: { command: 'npm test' } }
    expect(await frame(started, true)).toContain('Running…')

    const done = withResult(started, 'bash', JSON.stringify({ stdout: '12 passed\n', stderr: '' }))
    const shown = await frame(done)
    expect(shown).toContain('12 passed')
    expect(shown).not.toContain('Running…')

    const failed = withResult(
      started,
      'bash',
      JSON.stringify({ stdout: '', stderr: '1 failed\n', returnCodeInterpretation: 'exit_code:1' }),
      true,
    )
    expect(await frame(failed)).toContain('$ npm test  ✗ failed (exit code 1)')
  })
})

describe('a sub-agent', () => {
  const agent: ToolContentBlock = {
    type: 'tool',
    toolCallId: 'a1',
    toolName: 'Agent' as ToolContentBlock['toolName'],
    input: { description: 'Map the auth flow', prompt: 'Find every login path', subagent_type: 'Explore' },
  }

  test('reads running while it works', async () => {
    const shown = await frame(
      {
        ...agent,
        subagent: {
          description: 'Map the auth flow',
          subagentType: 'Explore',
          status: 'running',
          finished: false,
          isError: false,
          result: null,
          note: null,
        },
      },
      true,
    )
    expect(shown).toContain('• Agent Map the auth flow (Explore)  running…')
  })

  test('shows the start of its answer when it is done', async () => {
    const done = withResult(
      {
        ...agent,
        subagent: {
          description: 'Map the auth flow',
          subagentType: 'Explore',
          status: 'completed',
          finished: true,
          isError: false,
          result: 'Three login paths:\n1. password\n2. device code\n3. token',
          note: null,
        },
      },
      'Agent',
      JSON.stringify({ status: 'completed', result: 'Three login paths' }),
    )
    const shown = await frame(done)
    expect(shown).toContain('• Agent Map the auth flow (Explore)')
    expect(shown).not.toContain('running…')
    expect(shown).toContain('Three login paths:')
    expect(shown).toContain('2. device code')
    expect(shown).not.toContain('3. token')
  })

  test('a failed one says so, with why', async () => {
    const failed = withResult(
      {
        ...agent,
        subagent: {
          description: 'Map the auth flow',
          subagentType: null,
          status: 'failed',
          finished: true,
          isError: true,
          result: null,
          note: 'the model refused',
        },
      },
      'Agent',
      JSON.stringify({ status: 'failed', error: 'the model refused' }),
      true,
    )
    const shown = await frame(failed)
    expect(shown).toContain('✗ Agent Map the auth flow (Explore)  failed')
    expect(shown).toContain('the model refused')
  })

  test('without sub-agent events it still reads from its input and result', async () => {
    expect(await frame(agent, true)).toContain('• Agent Map the auth flow (Explore)  running…')
  })
})

describe('a streamed call, end to end', () => {
  test('the sink keeps one row, drawn from the first progress to the result', async () => {
    const { sink, tools, recorded } = turnHarness()
    const content = 'body { margin: 0; }\n.cart { display: grid; }\n'
    sink.handle({ type: 'tool_input_started', id: 'w', name: 'write_file' })
    sink.handle({
      type: 'tool_input_progress',
      id: 'w',
      name: 'write_file',
      bytes: 61,
      lines: 2,
      target: 'grocery-shop/style.css',
      preview: '.cart { display: grid; }',
    })
    const midStream = await frame(tools()[0]!, recorded.streamingRows.has('w'))
    expect(midStream).toContain('• Write grocery-shop/style.css  2 lines · 61 B')
    expect(midStream).toContain('.cart { display: grid; }')

    sink.handle({
      type: 'tool_call_started',
      id: 'w',
      name: 'write_file',
      input: JSON.stringify({ path: 'grocery-shop/style.css', content }),
      cut_off: false,
    })
    sink.handle({
      type: 'tool_result',
      id: 'w',
      name: 'write_file',
      output: JSON.stringify({ type: 'create', filePath: 'grocery-shop/style.css', bytes: 45, lines: 2, summary: '' }),
      is_error: false,
    })
    sink.handle({ type: 'turn_finished' })
    sink.finish()
    expect(tools()).toHaveLength(1)
    const done = await frame(tools()[0]!, recorded.streamingRows.has('w'))
    expect(done).toContain('• Create grocery-shop/style.css  2 lines · 45 B')
    expect(done).not.toContain('✗')
  })

  test('a call whose input never completed reads as not run, not as refused', async () => {
    const { sink, tools } = turnHarness()
    sink.handle({ type: 'tool_input_started', id: 'w', name: 'write_file' })
    sink.handle({
      type: 'tool_input_progress',
      id: 'w',
      name: 'write_file',
      bytes: 900,
      lines: 30,
      target: 'big.css',
      preview: null,
    })
    sink.handle({ type: 'error', message: 'The model stopped responding. (upstream_timeout)' })
    sink.handle({ type: 'turn_finished' })
    sink.finish()
    const shown = await frame(tools()[0]!)
    expect(shown).toContain('✗ Write big.css failed')
    expect(shown).toContain('The response ended before this call was complete')
    expect(shown).not.toContain('refused')
  })
})
