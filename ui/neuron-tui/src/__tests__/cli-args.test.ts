// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the tests
// now exercise parseArgs itself; the Freebuff variant, `login`, `--agent` and
// `--trust-agents` they covered were removed. See ui/neuron-tui/PORTING.md.
import { describe, test, expect } from 'bun:test'

import { parseArgs } from '../cli-args'

const parse = (...args: string[]) =>
  parseArgs({ argv: ['node', 'codebuff', ...args], version: '1.0.0' })

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

  test('mode flags pick the initial mode, the last one winning', () => {
    expect(parse('--lite').initialMode).toBe('LITE')
    expect(parse('--free').initialMode).toBe('LITE')
    expect(parse('--max').initialMode).toBe('MAX')
    expect(parse('--plan').initialMode).toBe('PLAN')
    expect(parse('--lite', '--plan').initialMode).toBe('PLAN')
  })
})
