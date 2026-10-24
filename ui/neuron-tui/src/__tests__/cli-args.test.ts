// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the tests
// now exercise parseArgs itself; the Freebuff variant, `login`, `--agent`,
// `--trust-agents` and the agent-mode flags they covered were removed.
// See ui/neuron-tui/PORTING.md.
import { describe, test, expect } from 'bun:test'

import { parseArgs } from '../cli-args'

const parse = (...args: string[]) =>
  parseArgs({ argv: ['node', 'neuron', ...args], version: '1.0.0' })

describe('CLI argument parsing', () => {
  test('joins the remaining words into the initial prompt', () => {
    expect(parse('find', 'all', 'TypeScript', 'files').initialPrompt).toBe(
      'find all TypeScript files',
    )
  })

  test('has no initial prompt without arguments', () => {
    const result = parse()
    expect(result.initialPrompt).toBeNull()
    expect(result.clearLogs).toBe(false)
    expect(result.continue).toBe(false)
    expect(result.initialMode).toBeUndefined()
  })

  test('parses --clear-logs beside a prompt', () => {
    const result = parse('--clear-logs', 'hello')
    expect(result.clearLogs).toBe(true)
    expect(result.initialPrompt).toBe('hello')
  })

  test('parses --cwd before or after the prompt', () => {
    expect(parse('--cwd', '/tmp', 'hi').cwd).toBe('/tmp')
    expect(parse('hi', '--cwd', '/tmp').cwd).toBe('/tmp')
  })

  test('parses --continue with and without a conversation id', () => {
    expect(parse('--continue')).toMatchObject({
      continue: true,
      continueId: null,
    })
    expect(parse('--continue', 'chat-42')).toMatchObject({
      continue: true,
      continueId: 'chat-42',
    })
  })

  test('engine flags pass through, and are absent when not given', () => {
    expect(
      parse(
        '--model', 'opus',
        '--provider', 'anthropic',
        '--permission-mode', 'read-only',
        '--allowedTools', 'bash, read_file',
        '--reasoning-effort', 'high',
        '--allow-broad-cwd',
        '--dangerously-skip-permissions',
        'fix it',
      ),
    ).toMatchObject({
      initialPrompt: 'fix it',
      engine: {
        model: 'opus',
        provider: 'anthropic',
        permissionMode: 'read-only',
        allowedTools: ['bash', 'read_file'],
        reasoningEffort: 'high',
        allowBroadCwd: true,
        dangerouslySkipPermissions: true,
      },
    })
    expect(parse().engine).toEqual({
      model: undefined,
      provider: undefined,
      permissionMode: undefined,
      allowedTools: undefined,
      reasoningEffort: undefined,
      allowBroadCwd: undefined,
      dangerouslySkipPermissions: undefined,
    })
  })
})
