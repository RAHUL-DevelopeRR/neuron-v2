import { describe, expect, test } from 'bun:test'

import { turnHarness } from './turn-harness'
import {
  LIVE_OUTPUT_KEEP_CHARS,
  liveOutputTail,
  withToolOutputDelta,
} from '../live-tool-blocks'

import type { ToolContentBlock } from '../../types/chat'

const startBash = (h: ReturnType<typeof turnHarness>, id = 'b1') =>
  h.sink.handle({
    type: 'tool_call_started',
    id,
    name: 'bash',
    input: '{"command":"npm test"}',
    cut_off: false,
  })

describe('a running command’s output', () => {
  test('grows its row as it prints, then the result replaces it', () => {
    const h = turnHarness()
    startBash(h)
    h.sink.handle({ type: 'tool_output_delta', tool_use_id: 'b1', stream: 'stdout', chunk: 'compiling\n' })
    h.sink.handle({ type: 'tool_output_delta', tool_use_id: 'b1', stream: 'stderr', chunk: 'warning: x\n' })
    expect(h.tools()[0]!.liveOutput).toBe('compiling\nwarning: x\n')
    expect(h.tools()[0]!.output).toBeUndefined()

    h.sink.handle({
      type: 'tool_result',
      id: 'b1',
      name: 'bash',
      output: JSON.stringify({ stdout: 'compiling\n12 passed\n', stderr: 'warning: x\n' }),
      is_error: false,
    })
    expect(h.tools()[0]!.output).toContain('12 passed')
    // A late delta never reopens a finished row.
    h.sink.handle({ type: 'tool_output_delta', tool_use_id: 'b1', stream: 'stdout', chunk: 'late\n' })
    expect(h.tools()[0]!.liveOutput ?? '').not.toContain('late')
  })

  test('deltas for unknown calls or after the turn stopped are dropped', () => {
    let active = true
    const h = turnHarness({ isActive: () => active })
    startBash(h)
    h.sink.handle({ type: 'tool_output_delta', tool_use_id: 'other', stream: 'stdout', chunk: 'x' })
    expect(h.tools()[0]!.liveOutput).toBeUndefined()
    active = false
    h.sink.handle({ type: 'tool_output_delta', tool_use_id: 'b1', stream: 'stdout', chunk: 'y' })
    expect(h.tools()[0]!.liveOutput).toBeUndefined()
  })

  test('keeps only the tail, and shows its last lines', () => {
    const block: ToolContentBlock = {
      type: 'tool',
      toolCallId: 'b1',
      toolName: 'run_terminal_command',
      input: { command: 'yes' },
    }
    const big = 'y\n'.repeat(LIVE_OUTPUT_KEEP_CHARS)
    const [grown] = withToolOutputDelta([block], 'b1', big + 'END\n') as ToolContentBlock[]
    expect(grown!.liveOutput!.length).toBe(LIVE_OUTPUT_KEEP_CHARS)
    expect(grown!.liveOutput!.endsWith('END\n')).toBe(true)
    expect(liveOutputTail('a\r\nb\nc\nd\n\n', 2)).toBe('c\nd')
    expect(liveOutputTail('\n\n', 3)).toBeNull()
  })
})
