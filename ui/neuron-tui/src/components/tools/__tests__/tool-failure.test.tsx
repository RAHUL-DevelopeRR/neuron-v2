import { beforeAll, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import { initializeThemeStore, useThemeStore } from '../../../hooks/use-theme'
import { createMarkdownPalette } from '../../../utils/theme-system'
import { ToolBranch } from '../../blocks/tool-branch'
import { toolFailureOf } from '../tool-call-item'

import type { ToolContentBlock } from '../../../types/chat'

beforeAll(() => {
  initializeThemeStore()
})

/** The tool row as the chat draws it, as text. */
async function frame(block: ToolContentBlock, height = 12): Promise<string> {
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

const REFUSED_WRITE =
  "Error: tool 'write_file' requires workspace-write permission; current mode is read-only"

describe('toolFailureOf', () => {
  test('refusals by the engine read "refused", other errors "failed"', () => {
    expect(toolFailureOf({ isError: true, output: REFUSED_WRITE })).toEqual({
      label: 'refused',
      reason: "tool 'write_file' requires workspace-write permission; current mode is read-only",
    })
    expect(
      toolFailureOf({
        isError: true,
        output: 'Error: No result: the call was denied, blocked by policy, or did not run.',
      })?.label,
    ).toBe('refused')
    expect(
      toolFailureOf({ isError: true, output: 'Error: The system cannot find the file specified.' }),
    ).toEqual({ label: 'failed', reason: 'The system cannot find the file specified.' })
    expect(toolFailureOf({ isError: true, output: '' })).toEqual({ label: 'failed', reason: null })
    expect(toolFailureOf({ isError: false, output: 'Error: looks bad' })).toBeNull()
    expect(toolFailureOf({ output: 'fine' })).toBeNull()
  })
})

describe('a failed tool call does not look like a success', () => {
  const edit = (isError: boolean, output: string): ToolContentBlock => ({
    type: 'tool',
    toolCallId: 'toolu_1',
    toolName: 'write_file',
    input: { path: 'ro.txt', content: 'x' },
    output,
    isError,
  })

  test('a refused write shows ✗, "refused" and why', async () => {
    const shown = await frame(edit(true, REFUSED_WRITE))
    // Was "• Edit ro.txt", exactly like a write that worked.
    expect(shown).toContain('✗ Edit ro.txt refused')
    expect(shown).toContain('current mode is read-only')
    expect(shown).not.toContain('•')
  })

  test('a successful write keeps its bullet', async () => {
    const shown = await frame(edit(false, 'message: Created new file ro.txt'))
    expect(shown).toContain('• Create ro.txt')
    expect(shown).not.toContain('✗')
    expect(shown).not.toContain('refused')
  })

  test('a failed command says so next to the command', async () => {
    const shown = await frame({
      type: 'tool',
      toolCallId: 'toolu_2',
      toolName: 'run_terminal_command',
      input: { command: 'cargo test' },
      output: 'error: could not compile',
      isError: true,
    })
    expect(shown).toContain('$ cargo test  ✗ failed')
    expect(shown).toContain('error: could not compile')
  })

  test('a failed read names the file, then the reason', async () => {
    const shown = await frame({
      type: 'tool',
      toolCallId: 'toolu_3',
      toolName: 'read_files',
      input: { paths: ['missing.txt'] },
      output: 'Error: The system cannot find the file specified.',
      isError: true,
    })
    expect(shown).toContain('✗ Read missing.txt failed')
    expect(shown).toContain('The system cannot find the file specified.')
  })
})

describe('tool output is data, not Markdown', () => {
  test('file contents keep their indentation and JSDoc lines', async () => {
    // The bare ``` line closed the ```yaml fence the result used to be
    // wrapped in; everything after it was parsed as Markdown.
    const source = [
      'const doc = `',
      '```',
      '`',
      '/**',
      ' * Adds two numbers.',
      ' * ```ts',
      ' * add(1, 2)',
      ' * ```',
      ' */',
      'function add(a, b) {',
      '    return a + b',
      '}',
    ].join('\n')
    const shown = await frame(
      {
        type: 'tool',
        toolCallId: 'toolu_4',
        toolName: 'NotebookRead' as ToolContentBlock['toolName'],
        input: { path: 'add.js' },
        output: source,
        isCollapsed: false,
      },
      30,
    )
    for (const line of [' * Adds two numbers.', ' * ```ts', ' */', '    return a + b']) {
      expect(shown).toContain(line)
    }
    // Not turned into list bullets.
    expect(shown).not.toContain('- Adds two numbers.')
  })
})
