import { describe, expect, test } from 'bun:test'

import { formatToolOutput } from '../../utils/tool-display'
import { displayPath, mapToolCall, mapToolResult } from '../tool-mapping'

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

  test('TodoWrite becomes write_todos with done flags', () => {
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
          { task: 'Read', completed: true },
          { task: 'Fix', completed: false },
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

  test('a command that ran and failed still shows its output', () => {
    // A non-zero exit is an error result carrying the command's output.
    const output = JSON.stringify({
      stdout: 'partial',
      stderr: 'error: boom',
      interrupted: false,
      returnCodeInterpretation: 'exit_code:2',
    })
    expect(mapToolResult('bash', output, true)).toEqual([
      { type: 'json', value: { stdout: 'partial', stderr: 'error: boom' } },
    ])
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
})

test('displayPath strips the Windows verbatim prefix only', () => {
  expect(displayPath('\\\\?\\C:\\a\\b')).toBe('C:\\a\\b')
  expect(displayPath('\\\\?\\UNC\\server\\share')).toBe('\\\\server\\share')
  expect(displayPath('/home/a')).toBe('/home/a')
})
