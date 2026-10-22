import { beforeAll, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import { mapToolResult } from '../../../engine/tool-mapping'
import { initializeThemeStore, useThemeStore } from '../../../hooks/use-theme'
import { updateToolBlockWithOutput } from '../../../utils/message-block-helpers'
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
      detail: null,
    })
    expect(
      toolFailureOf({
        isError: true,
        output: 'Error: No result: the call was denied, blocked by policy, or did not run.',
      })?.label,
    ).toBe('refused')
    expect(
      toolFailureOf({ isError: true, output: 'Error: The system cannot find the file specified.' }),
    ).toEqual({
      label: 'failed',
      reason: 'The system cannot find the file specified.',
      detail: null,
    })
    expect(toolFailureOf({ isError: true, output: '' })).toEqual({
      label: 'failed',
      reason: null,
      detail: null,
    })
    expect(toolFailureOf({ isError: false, output: 'Error: looks bad' })).toBeNull()
    expect(toolFailureOf({ output: 'fine' })).toBeNull()
  })

  test('a command that says how it ended failed, whatever it printed', () => {
    expect(
      toolFailureOf({ isError: true, output: 'permission denied by policy', exitCode: 1 }),
    ).toEqual({ label: 'failed', reason: 'permission denied by policy', detail: 'exit code 1' })
    expect(toolFailureOf({ isError: true, output: '', stopReason: 'timed out' })?.detail).toBe(
      'timed out',
    )
  })
})

describe('a failed command says how it ended', () => {
  /** A bash result as the engine sends it, through the chat's tool-result path. */
  const ran = (command: string, result: Record<string, unknown>): ToolContentBlock => {
    const [block] = updateToolBlockWithOutput(
      [{ type: 'tool', toolCallId: 'toolu_9', toolName: 'run_terminal_command', input: { command } }],
      {
        toolCallId: 'toolu_9',
        toolOutput: mapToolResult('bash', JSON.stringify(result), true),
        isError: true,
      },
    )
    return block as ToolContentBlock
  }

  test('a non-zero exit shows its code next to the command, above its output', async () => {
    const shown = await frame(
      ran('node fail.js', {
        stdout: 'out-line\n',
        stderr: 'err-line\n',
        interrupted: false,
        returnCodeInterpretation: 'exit_code:2',
      }),
    )
    // Was "$ node fail.js  ✗ failed", with no exit code.
    expect(shown).toContain('$ node fail.js  ✗ failed (exit code 2)')
    expect(shown).toContain('out-line')
    expect(shown).toContain('err-line')
    expect(shown).not.toContain('(no output)')
  })

  test('a failed command that printed nothing says so', async () => {
    const shown = await frame(
      ran('findstr nothing x.txt', {
        stdout: '',
        stderr: '',
        interrupted: false,
        returnCodeInterpretation: 'exit_code:1',
      }),
    )
    // Was the bare header "$ findstr nothing x.txt  ✗ failed".
    expect(shown).toContain('$ findstr nothing x.txt  ✗ failed (exit code 1)')
    expect(shown).toContain('(no output)')
  })

  test('a command stopped before it finished says why', async () => {
    const killed = await frame(
      ran('server', { stdout: '', stderr: '', interrupted: false, returnCodeInterpretation: 'killed' }),
    )
    expect(killed).toContain('$ server  ✗ failed (killed)')
    expect(killed).toContain('(no output)')

    const timedOut = await frame(
      ran('sleep 100', {
        stdout: '',
        stderr: 'Command exceeded timeout of 1000 ms; the process tree was terminated',
        interrupted: true,
        returnCodeInterpretation: 'timeout',
      }),
    )
    expect(timedOut).toContain('$ sleep 100  ✗ failed (timed out)')
    expect(timedOut).toContain('Command exceeded timeout of 1000 ms')

    const interrupted = await frame(
      ran('cargo build', {
        stdout: '',
        stderr: '',
        interrupted: true,
        returnCodeInterpretation: 'cancelled',
      }),
    )
    expect(interrupted).toContain('$ cargo build  ✗ failed (interrupted)')
  })

  test('a command that succeeded has no failure marks', async () => {
    const [block] = updateToolBlockWithOutput(
      [{ type: 'tool', toolCallId: 'toolu_10', toolName: 'run_terminal_command', input: { command: 'true' } }],
      {
        toolCallId: 'toolu_10',
        toolOutput: mapToolResult(
          'bash',
          JSON.stringify({ stdout: '', stderr: '', interrupted: false }),
          false,
        ),
        isError: false,
      },
    )
    const shown = await frame(block as ToolContentBlock)
    expect(shown).toContain('$ true')
    expect(shown).not.toContain('✗')
    expect(shown).not.toContain('(no output)')
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

  test('a command whose output is JSON shows it as printed', async () => {
    // The row's output is the command's stdout and stderr; one that parsed as
    // JSON (a number, `cat package.json`) used to be read as an empty result.
    const printed = async (stdout: string) => {
      const [block] = updateToolBlockWithOutput(
        [{ type: 'tool', toolCallId: 'toolu_5', toolName: 'run_terminal_command', input: { command: 'cmd' } }],
        {
          toolCallId: 'toolu_5',
          toolOutput: mapToolResult('bash', JSON.stringify({ stdout, stderr: '' }), false),
          isError: false,
        },
      )
      return frame(block as ToolContentBlock)
    }
    expect(await printed('42\n')).toContain('42')
    expect(await printed('0\n')).toContain('0')
    expect(await printed('{"name": "neuron-tui"}\n')).toContain('{"name": "neuron-tui"}')
  })
})
