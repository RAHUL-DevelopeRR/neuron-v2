import { describe, expect, test } from 'bun:test'

import { formatToolOutput } from '../../utils/tool-display'
import { commandExitOf, displayPath, mapToolCall, mapToolResult } from '../tool-mapping'

// Inputs and outputs below are the engine's real lines (from
// `neuron engine --stdio` against the mock Anthropic service).
describe('mapToolCall', () => {
  test('bash becomes run_terminal_command with the timeout in seconds', () => {
    expect(
      mapToolCall('bash', '{"command":"printf ok | tee a.txt","timeout":1000}'),
    ).toEqual({
      toolName: 'run_terminal_command',
      input: { command: 'printf ok | tee a.txt', timeout_seconds: 1 },
    })
  })

  test('file tools take the renderers’ shapes', () => {
    expect(mapToolCall('read_file', '{"path":"fixture.txt"}')).toEqual({
      toolName: 'read_files',
      input: { paths: ['fixture.txt'] },
    })
    expect(
      mapToolCall('edit_file', '{"path":"a.rs","old_string":"x","new_string":"y"}'),
    ).toEqual({
      toolName: 'str_replace',
      input: {
        path: 'a.rs',
        replacements: [{ old: 'x', new: 'y', allowMultiple: false }],
      },
    })
    expect(
      mapToolCall('grep_search', '{"pattern":"parity","path":"src","glob":"*.rs"}'),
    ).toEqual({
      toolName: 'code_search',
      input: { pattern: 'parity', cwd: 'src', flags: '-g *.rs' },
    })
  })

  test('TodoWrite becomes write_todos with done flags, status and present tense', () => {
    expect(
      mapToolCall(
        'TodoWrite',
        JSON.stringify({
          todos: [
            { content: 'Read', activeForm: 'Reading', status: 'completed' },
            { content: 'Fix', activeForm: 'Fixing', status: 'in_progress' },
          ],
        }),
      ),
    ).toEqual({
      toolName: 'write_todos',
      input: {
        todos: [
          { task: 'Read', completed: true, status: 'completed', activeForm: 'Reading' },
          { task: 'Fix', completed: false, status: 'in_progress', activeForm: 'Fixing' },
        ],
      },
    })
  })

  test('a tool without a renderer keeps its name and input', () => {
    expect(mapToolCall('Agent', '{"prompt":"look"}')).toEqual({
      toolName: 'Agent',
      input: { prompt: 'look' },
    })
    expect(mapToolCall('MCP', 'not json')).toEqual({
      toolName: 'MCP',
      input: { input: 'not json' },
    })
  })
})

describe('mapToolResult', () => {
  test('bash output keeps stdout and stderr for the terminal renderer', () => {
    const output = JSON.stringify({
      stdout: 'approved via prompt',
      stderr: '',
      rawOutputPath: null,
      interrupted: false,
    })
    expect(mapToolResult('bash', output, false)).toEqual([
      { type: 'json', value: { stdout: 'approved via prompt', stderr: '' } },
    ])
    expect(mapToolResult('bash', 'timed out', true)).toEqual([
      { type: 'json', value: { errorMessage: 'timed out' } },
    ])
  })

  test('a command that ran and failed still shows its output, and its exit code', () => {
    // A non-zero exit is an error result carrying the command's output.
    const output = JSON.stringify({
      stdout: 'partial',
      stderr: 'error: boom',
      interrupted: false,
      returnCodeInterpretation: 'exit_code:2',
    })
    expect(mapToolResult('bash', output, true)).toEqual([
      { type: 'json', value: { stdout: 'partial', stderr: 'error: boom', exitCode: 2 } },
    ])
    // A failure with nothing printed keeps how it ended too.
    expect(
      mapToolResult(
        'PowerShell',
        JSON.stringify({ stdout: '', stderr: '', interrupted: false, returnCodeInterpretation: 'exit_code:1' }),
        true,
      ),
    ).toEqual([{ type: 'json', value: { stdout: '', stderr: '', exitCode: 1 } }])
  })

  test('how a failed command ended, from returnCodeInterpretation and interrupted', () => {
    // BashCommandOutput (runtime/src/bash.rs) sets the interpretation only
    // for a command that did not succeed.
    const ended = (returnCodeInterpretation: string | null, interrupted = false) =>
      commandExitOf({ stdout: '', stderr: '', interrupted, returnCodeInterpretation })
    expect(ended(null)).toEqual({})
    expect(ended('exit_code:7')).toEqual({ exitCode: 7 })
    expect(ended('exit_code:-1073741510')).toEqual({ exitCode: -1073741510 })
    expect(ended('timeout', true)).toEqual({ stopReason: 'timed out' })
    expect(ended('cancelled', true)).toEqual({ stopReason: 'interrupted' })
    expect(ended('killed')).toEqual({ stopReason: 'killed' })
    expect(ended('signal:9')).toEqual({ stopReason: 'killed (signal 9)' })
    expect(ended('preflight_blocked:branch_divergence')).toEqual({
      stopReason: 'blocked: branch divergence',
    })
    expect(ended('preflight_blocked')).toEqual({ stopReason: 'blocked' })
    expect(ended(null, true)).toEqual({ stopReason: 'interrupted' })
    // Exit-code fields of other shapes.
    expect(commandExitOf({ stdout: '', exitCode: 3 })).toEqual({ exitCode: 3 })
    expect(commandExitOf({ stdout: '', exit_code: 4 })).toEqual({ exitCode: 4 })
    expect(commandExitOf({ stdout: '', exitCode: 0 })).toEqual({})
  })

  test('write_file reads as a created file, without the verbatim prefix', () => {
    const output = JSON.stringify({
      type: 'create',
      filePath: '\\\\?\\C:\\work\\generated\\output.txt',
      bytes: 24,
      lines: 1,
      summary: 'created',
    })
    const shown = formatToolOutput(mapToolResult('write_file', output, false))
    expect(shown).toContain('Created new file C:\\work\\generated\\output.txt')
  })

  test('errors and plain text are shown as they came', () => {
    expect(
      formatToolOutput(
        mapToolResult('read_file', 'The system cannot find the file specified.', true),
      ),
    ).toBe('Error: The system cannot find the file specified.')
    expect(formatToolOutput(mapToolResult('WebFetch', 'page text: a, b', false))).toBe(
      'page text: a, b',
    )
  })

  test('grep counts lead with the match total the summary reads', () => {
    const output = JSON.stringify({ numFiles: 1, filenames: ['a.rs'], numMatches: 3 })
    expect(formatToolOutput(mapToolResult('grep_search', output, false))).toBe(
      'Found 3 matches\na.rs',
    )
  })

  test('write_file keeps what it wrote, and a diff when the engine sends one', () => {
    const output = JSON.stringify({ type: 'update', filePath: 'a.css', bytes: 2048, lines: 80, summary: '' })
    expect(mapToolResult('write_file', output, false)).toEqual([
      { type: 'json', value: { message: 'Updated file a.css', file: 'a.css', lines: 80, bytes: 2048 } },
    ])
    const withDiff = JSON.stringify({ type: 'update', filePath: 'a.css', diff: '@@ -1 +1 @@\n-a\n+b' })
    expect((mapToolResult('write_file', withDiff, false)[0] as { value: unknown }).value).toMatchObject({
      unifiedDiff: '@@ -1 +1 @@\n-a\n+b',
    })
  })

  test("an Agent result reads as the sub-agent's text, not its manifest", () => {
    const manifest = { agentId: 'a1', status: 'completed', outputFile: '/tmp/a1.md' }
    expect(
      formatToolOutput(mapToolResult('Agent', JSON.stringify({ ...manifest, result: 'Found it.' }), false)),
    ).toBe('Found it.')
    expect(formatToolOutput(mapToolResult('Agent', JSON.stringify(manifest), false))).toBe(
      'Sub-agent completed.',
    )
    expect(
      formatToolOutput(
        mapToolResult('Agent', JSON.stringify({ ...manifest, status: 'failed', error: 'boom' }), true),
      ),
    ).toBe('Error: boom')
    expect(formatToolOutput(mapToolResult('Agent', 'agent type not allowed', true))).toBe(
      'Error: agent type not allowed',
    )
  })
})

test('displayPath strips the Windows verbatim prefix only', () => {
  expect(displayPath('\\\\?\\C:\\a\\b')).toBe('C:\\a\\b')
  expect(displayPath('\\\\?\\UNC\\server\\share')).toBe('\\\\server\\share')
  expect(displayPath('/home/a')).toBe('/home/a')
})
