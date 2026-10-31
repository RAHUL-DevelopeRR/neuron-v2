import { afterEach, beforeEach, describe, expect, test } from 'bun:test'

import { resetLiveTurnStore, useLiveTurnStore } from '../../state/live-turn-store'
import {
  STALL_AFTER_MS,
  describeLiveActivity,
  formatBytes,
  inputSizeDetail,
  liveFooterTokens,
  liveOutputTokens,
  setLiveTurnClockForTesting,
  startLiveTurn,
  targetOfToolInput,
} from '../live-turn'
import { turnHarness } from './turn-harness'

import type { TokenCounts, UiEvent } from '../protocol'

/** A clock the test moves by hand. */
let now = 1_000_000
const advance = (ms: number) => {
  now += ms
}

beforeEach(() => {
  now = 1_000_000
  setLiveTurnClockForTesting(() => now)
  resetLiveTurnStore()
})

afterEach(() => {
  setLiveTurnClockForTesting(null)
  resetLiveTurnStore()
})

const live = () => useLiveTurnStore.getState()
const activity = (awaitingApproval = false) =>
  describeLiveActivity(live(), { now, awaitingApproval })
const counts = (input: number, output: number): TokenCounts => ({
  input,
  output,
  cache_read: 0,
  cache_write: 0,
})

const CSS_PATH = 'grocery-shop/style.css'
const CSS = Array.from({ length: 412 }, (_, line) => `.rule-${line} { color: red; }`).join('\n')

/** What a live_progress engine sends for one long write_file call. */
function writeFileStream(id = 'toolu_w'): UiEvent[] {
  const input = JSON.stringify({ path: CSS_PATH, content: CSS })
  const events: UiEvent[] = [
    { type: 'output_progress', output_tokens: 3 },
    { type: 'tool_input_started', id, name: 'write_file' },
  ]
  for (let step = 1; step <= 40; step++) {
    const lines = Math.round((412 * step) / 40)
    events.push({
      type: 'tool_input_progress',
      id,
      name: 'write_file',
      bytes: Math.round((input.length * step) / 40),
      lines,
      target: CSS_PATH,
      preview: `.rule-${lines - 2} { color: red; }\n.rule-${lines - 1} { color: red; }`,
    })
    events.push({ type: 'output_progress', output_tokens: 30 * step })
  }
  events.push(
    { type: 'tool_call_started', id, name: 'write_file', input, cut_off: false },
    { type: 'request_usage', usage: counts(9000, 1250) },
    {
      type: 'tool_result',
      id,
      name: 'write_file',
      output: JSON.stringify({
        type: 'create',
        filePath: CSS_PATH,
        bytes: CSS.length,
        lines: 412,
        summary: `Created ${CSS_PATH} (412 lines, ${CSS.length} bytes)`,
      }),
      is_error: false,
    },
  )
  return events
}

describe('a long write_file call, as a live_progress engine streams it', () => {
  test('one cell from the first event on, filled in, then completed: never a second', () => {
    const { sink, tools, recorded } = turnHarness()
    const stream = writeFileStream()
    const started = stream.findIndex((event) => event.type === 'tool_input_started')
    const complete = stream.findIndex((event) => event.type === 'tool_call_started')

    // The model names the tool: its row is there at once, and the turn is
    // no longer "waiting".
    for (const event of stream.slice(0, started + 1)) sink.handle(event)
    expect(tools()).toHaveLength(1)
    expect(tools()[0]).toMatchObject({
      toolCallId: 'toolu_w',
      toolName: 'write_file',
      liveInput: { bytes: 0, lines: 0, target: null, preview: null },
    })
    expect(recorded.streamStatus).toEqual(['streaming'])
    expect(recorded.streamingRows.has('toolu_w')).toBe(true)
    expect(activity()).toMatchObject({ verb: 'Writing', target: null })

    // The input grows: path, line count and the last lines show before the
    // complete input arrives.
    for (const event of stream.slice(started + 1, complete)) sink.handle(event)
    expect(tools()).toHaveLength(1)
    const streaming = tools()[0]!
    expect(streaming.liveInput).toMatchObject({ lines: 412, target: CSS_PATH })
    expect(streaming.liveInput!.preview).toContain('.rule-411 { color: red; }')
    expect(activity()).toEqual({
      verb: 'Writing',
      target: CSS_PATH,
      detail: `412 lines · ${formatBytes(streaming.liveInput!.bytes)}`,
      stalled: false,
    })
    expect(liveOutputTokens(live())).toBe(1200)

    // The complete input replaces what streamed, in the same row.
    sink.handle(stream[complete]!)
    expect(tools()).toHaveLength(1)
    expect(tools()[0]!.liveInput).toBeUndefined()
    expect(tools()[0]!.input).toEqual({ path: CSS_PATH, content: CSS })
    expect(activity()).toMatchObject({ verb: 'Writing', target: CSS_PATH, detail: null })

    // The request's usage: the live count is the provider's now.
    sink.handle(stream[complete + 1]!)
    expect(liveOutputTokens(live())).toBe(1250)

    // The result closes it; the next request is awaited.
    sink.handle(stream[complete + 2]!)
    sink.handle({ type: 'output_progress', output_tokens: 40 })
    sink.handle({ type: 'text_delta', text: 'Done.' })
    sink.handle({ type: 'request_usage', usage: counts(9500, 20) })
    sink.handle({
      type: 'usage',
      turn: counts(18500, 1270),
      cumulative: counts(18500, 1270),
      estimated: false,
    })
    sink.handle({ type: 'turn_finished' })
    sink.finish()

    expect(tools()).toHaveLength(1)
    expect(tools()[0]).toMatchObject({ isError: false })
    expect(tools()[0]!.output).toContain(`Created new file ${CSS_PATH}`)
    expect(tools()[0]!.output).toContain('lines: 412')
    expect(recorded.streamingRows.size).toBe(0)
    expect(live()).toMatchObject({ active: false, calls: [], unsettledInput: 0, unsettledOutput: 0 })
  })

  test('the status line follows the turn: waiting, writing, running, waiting, responding', () => {
    const { sink } = turnHarness()
    expect(activity().verb).toBe('Waiting for the model')
    sink.handle({ type: 'reasoning_delta', text: 'plan' })
    expect(activity().verb).toBe('Thinking')
    sink.handle({ type: 'tool_input_started', id: 't1', name: 'bash' })
    sink.handle({
      type: 'tool_input_progress',
      id: 't1',
      name: 'bash',
      bytes: 20,
      lines: 1,
      target: 'npm test',
      preview: 'npm test',
    })
    expect(activity()).toMatchObject({ verb: 'Preparing', target: 'npm test' })
    sink.handle({
      type: 'tool_call_started',
      id: 't1',
      name: 'bash',
      input: '{"command":"npm test"}',
      cut_off: false,
    })
    expect(activity()).toMatchObject({ verb: 'Running', target: 'npm test' })
    expect(activity(true)).toMatchObject({ verb: 'Waiting for approval', target: 'npm test' })
    sink.handle({ type: 'tool_result', id: 't1', name: 'bash', output: 'ok', is_error: false })
    expect(activity()).toMatchObject({ verb: 'Waiting for the model', detail: null })
    sink.handle({ type: 'text_delta', text: 'All green.' })
    expect(activity().verb).toBe('Responding')
  })

  test('edit_file reads "Editing"; the footer adds what the turn used so far', () => {
    const { sink } = turnHarness()
    sink.handle({ type: 'tool_input_started', id: 'e1', name: 'edit_file' })
    sink.handle({
      type: 'tool_input_progress',
      id: 'e1',
      name: 'edit_file',
      bytes: 2048,
      lines: 3,
      target: '\\\\?\\C:\\work\\src\\app.ts',
      preview: null,
    })
    expect(activity()).toMatchObject({
      verb: 'Editing',
      target: 'C:\\work\\src\\app.ts',
      detail: '3 lines · 2 KB',
    })
    sink.handle({ type: 'output_progress', output_tokens: 90 })
    expect(liveFooterTokens(live())).toEqual({ input: 0, output: 90 })
    sink.handle({ type: 'request_usage', usage: { input: 100, output: 95, cache_read: 900, cache_write: 5 } })
    expect(liveFooterTokens(live())).toEqual({ input: 1005, output: 95 })
    sink.handle({ type: 'usage', turn: counts(1, 1), cumulative: counts(1, 1), estimated: false })
    expect(liveFooterTokens(live())).toBeNull()
  })
})

describe('a stalled stream', () => {
  test('a streaming request silent for more than 10 s says so, counting up', () => {
    const { sink } = turnHarness()
    sink.handle({ type: 'text_delta', text: 'Let me' })
    advance(STALL_AFTER_MS)
    expect(activity()).toMatchObject({ verb: 'Responding', stalled: false })
    advance(22_000)
    expect(activity()).toEqual({
      verb: 'Waiting for the model',
      target: null,
      detail: 'no tokens for 32s',
      stalled: true,
    })
    advance(60_000)
    expect(activity().detail).toBe('no tokens for 1m 32s')
    // The next token ends it.
    sink.handle({ type: 'output_progress', output_tokens: 12 })
    expect(activity()).toMatchObject({ verb: 'Responding', stalled: false })
  })

  test('a tool input that stops growing is a stall too', () => {
    const { sink } = turnHarness()
    sink.handle({ type: 'tool_input_started', id: 'w', name: 'write_file' })
    advance(15_000)
    expect(activity()).toMatchObject({ detail: 'no tokens for 15s', stalled: true })
    sink.handle({
      type: 'tool_input_progress',
      id: 'w',
      name: 'write_file',
      bytes: 10,
      lines: 1,
      target: 'a.txt',
      preview: 'x',
    })
    expect(activity()).toMatchObject({ verb: 'Writing', target: 'a.txt', stalled: false })
  })

  test('before the first token it waits, with the wait time after 10 s', () => {
    turnHarness()
    advance(5_000)
    expect(activity()).toMatchObject({ verb: 'Waiting for the model', detail: null })
    advance(27_000)
    expect(activity()).toMatchObject({
      verb: 'Waiting for the model',
      detail: '32s',
      stalled: false,
    })
  })

  test('a running tool or the permission dialog is an expected silence', () => {
    const { sink } = turnHarness()
    sink.handle({ type: 'text_delta', text: 'Running the tests.' })
    sink.handle({
      type: 'tool_call_started',
      id: 't1',
      name: 'bash',
      input: '{"command":"cargo test"}',
      cut_off: false,
    })
    advance(120_000)
    expect(activity()).toMatchObject({ verb: 'Running', target: 'cargo test', stalled: false })
    expect(activity(true)).toMatchObject({ verb: 'Waiting for approval', stalled: false })
    expect(activity().detail).toBeNull()
  })

  test('status, notice and usage events do not reset the clock', () => {
    const { sink } = turnHarness()
    sink.handle({ type: 'text_delta', text: 'Hi' })
    advance(20_000)
    sink.handle({ type: 'notice', message: 'Context is 80% full.' })
    sink.handle({
      type: 'usage',
      turn: counts(1, 1),
      cumulative: counts(1, 1),
      estimated: true,
    })
    expect(activity()).toMatchObject({ detail: 'no tokens for 20s', stalled: true })
  })

  test('a retry shows "retrying", and the clock starts over with the next token', () => {
    const { sink, recorded, tools } = turnHarness()
    sink.handle({ type: 'text_delta', text: 'Partial answer' })
    sink.handle({ type: 'tool_input_started', id: 'lost', name: 'write_file' })
    advance(95_000)
    expect(activity().stalled).toBe(true)
    sink.handle({
      type: 'notice',
      message: 'Zero-X upstream_timeout; retrying in 1.3s (retry 1 of 2)',
    })
    expect(recorded.retrying.at(-1)).toBe(true)
    // The failed attempt's tool input never runs: its row goes with its text.
    expect(tools()).toHaveLength(0)
    expect(live().calls).toEqual([])
    expect(activity()).toMatchObject({ verb: 'Waiting for the model', stalled: false })
    advance(3_000)
    sink.handle({ type: 'output_progress', output_tokens: 4 })
    expect(recorded.retrying.at(-1)).toBe(false)
    expect(activity()).toMatchObject({ verb: 'Responding', stalled: false })
  })

  test('an older engine: only text and reasoning reset the clock', () => {
    const { sink } = turnHarness()
    sink.handle({ type: 'reasoning_delta', text: 'hmm' })
    advance(12_000)
    expect(activity().stalled).toBe(true)
    sink.handle({ type: 'reasoning_delta', text: ' ok' })
    expect(activity()).toMatchObject({ verb: 'Thinking', stalled: false })
    advance(11_000)
    sink.handle({ type: 'text_delta', text: 'Answer' })
    expect(activity()).toMatchObject({ verb: 'Responding', stalled: false })
    expect(liveOutputTokens(live())).toBe(0)
    expect(liveFooterTokens(live())).toBeNull()
  })
})

describe('an older engine (no features, no progress events)', () => {
  test('tool calls draw exactly as before: one row each, closed by their results', () => {
    const { sink, tools, recorded } = turnHarness()
    sink.handle({ type: 'text_delta', text: 'Writing it.' })
    sink.handle({
      type: 'tool_call_started',
      id: 'toolu_1',
      name: 'write_file',
      input: JSON.stringify({ path: 'a.txt', content: 'x\ny' }),
      cut_off: false,
    })
    sink.handle({
      type: 'tool_call_started',
      id: 'toolu_2',
      name: 'bash',
      input: '{"command":"ls"}',
      cut_off: false,
    })
    expect(activity()).toMatchObject({ verb: 'Writing', target: 'a.txt' })
    sink.handle({
      type: 'tool_result',
      id: 'toolu_1',
      name: 'write_file',
      output: JSON.stringify({ type: 'update', filePath: 'a.txt', bytes: 3, lines: 2, summary: '' }),
      is_error: false,
    })
    expect(activity()).toMatchObject({ verb: 'Running', target: 'ls' })
    sink.handle({ type: 'tool_result', id: 'toolu_2', name: 'bash', output: 'a.txt', is_error: false })
    sink.handle({ type: 'usage', turn: counts(10, 5), cumulative: counts(10, 5), estimated: false })
    sink.handle({ type: 'turn_finished' })
    sink.finish()

    expect(tools().map(({ toolCallId, toolName, input, isError }) => ({ toolCallId, toolName, input, isError }))).toEqual([
      { toolCallId: 'toolu_1', toolName: 'write_file', input: { path: 'a.txt', content: 'x\ny' }, isError: false },
      { toolCallId: 'toolu_2', toolName: 'run_terminal_command', input: { command: 'ls' }, isError: false },
    ])
    expect(tools().some((tool) => 'liveInput' in tool)).toBe(false)
    expect(recorded.streamingRows.size).toBe(0)
    expect(liveOutputTokens(live())).toBe(0)
  })

  test('TodoWrite still feeds the task list', () => {
    const { sink } = turnHarness()
    sink.handle({
      type: 'tool_call_started',
      id: 'todo',
      name: 'TodoWrite',
      input: JSON.stringify({
        todos: [
          { content: 'Read the code', activeForm: 'Reading the code', status: 'completed' },
          { content: 'Fix the bug', activeForm: 'Fixing the bug', status: 'in_progress' },
          { content: 'Run the tests', activeForm: 'Running the tests', status: 'pending' },
        ],
      }),
      cut_off: false,
    })
    expect(live().todosThisTurn).toBe(true)
    expect(live().todos).toEqual([
      { task: 'Read the code', completed: true, status: 'completed', activeForm: 'Reading the code' },
      { task: 'Fix the bug', completed: false, status: 'in_progress', activeForm: 'Fixing the bug' },
      { task: 'Run the tests', completed: false, status: 'pending', activeForm: 'Running the tests' },
    ])
    expect(activity()).toMatchObject({ verb: 'Updating the todo list', target: null })
  })
})

describe('todos and sub-agents', () => {
  test('a todos event replaces the list and marks the turn as having one', () => {
    const { sink } = turnHarness()
    sink.handle({
      type: 'todos',
      items: [
        { content: 'Scaffold', status: 'completed', active_form: 'Scaffolding' },
        { content: 'Style', status: 'in_progress', active_form: 'Styling the page' },
      ],
    })
    expect(live().todos).toEqual([
      { task: 'Scaffold', completed: true, status: 'completed', activeForm: 'Scaffolding' },
      { task: 'Style', completed: false, status: 'in_progress', activeForm: 'Styling the page' },
    ])
    sink.handle({ type: 'turn_finished' })
    // The list outlives the turn (for /todo); the strip is the turn's.
    expect(live()).toMatchObject({ active: false, todosThisTurn: false })
    expect(live().todos).toHaveLength(2)
  })

  test('an Agent call becomes one cell that follows its sub-agent', () => {
    const { sink, tools } = turnHarness()
    sink.handle({
      type: 'tool_call_started',
      id: 'agent_1',
      name: 'Agent',
      input: JSON.stringify({
        description: 'Map the auth flow',
        prompt: 'Find every login path',
        subagent_type: 'Explore',
      }),
      cut_off: false,
    })
    sink.handle({
      type: 'subagent_started',
      id: 'agent_1',
      description: 'Map the auth flow',
      subagent_type: 'Explore',
    })
    expect(tools()[0]).toMatchObject({
      toolName: 'Agent',
      subagent: { description: 'Map the auth flow', subagentType: 'Explore', status: 'running', finished: false },
    })
    expect(activity()).toMatchObject({ verb: 'Running agent', target: 'Map the auth flow' })
    sink.handle({
      type: 'subagent_finished',
      id: 'agent_1',
      status: 'completed',
      is_error: false,
      result: 'Three login paths: password, device code, token.',
      note: null,
    })
    sink.handle({
      type: 'tool_result',
      id: 'agent_1',
      name: 'Agent',
      output: JSON.stringify({ status: 'completed', result: 'Three login paths: password, device code, token.' }),
      is_error: false,
    })
    expect(tools()).toHaveLength(1)
    expect(tools()[0]).toMatchObject({
      isError: false,
      subagent: { status: 'completed', finished: true, isError: false, result: 'Three login paths: password, device code, token.' },
    })
  })

  test('without sub-agent events, the Agent result tells how it ended', () => {
    const { sink, tools } = turnHarness()
    sink.handle({
      type: 'tool_call_started',
      id: 'agent_2',
      name: 'Agent',
      input: JSON.stringify({ description: 'Audit deps', prompt: 'x' }),
      cut_off: false,
    })
    sink.handle({
      type: 'tool_result',
      id: 'agent_2',
      name: 'Agent',
      output: JSON.stringify({
        agentId: 'a2',
        description: 'Audit deps',
        subagentType: 'general-purpose',
        status: 'failed',
        error: 'the model refused',
      }),
      is_error: true,
    })
    expect(tools()[0]).toMatchObject({
      isError: true,
      subagent: { description: 'Audit deps', status: 'failed', finished: true, isError: true, note: 'the model refused' },
    })
  })
})

describe('turns that overlap', () => {
  test("an interrupted turn's last events do not end the next turn's view", () => {
    const old = turnHarness({ isActive: () => false })
    old.sink.handle({ type: 'text_delta', text: 'ignored' })
    // A prompt sent right after the interrupt starts its own view.
    const next = startLiveTurn()
    old.sink.handle({ type: 'turn_finished' })
    old.sink.finish()
    expect(live().active).toBe(true)
    next.end()
    expect(live().active).toBe(false)
  })
})

describe('targets and sizes', () => {
  test('each tool names what it works on', () => {
    expect(targetOfToolInput('bash', '{"command":"npm test\\nnpm run lint"}')).toBe('npm test')
    expect(targetOfToolInput('read_file', '{"path":"\\\\\\\\?\\\\C:\\\\a.txt"}')).toBe('C:\\a.txt')
    expect(targetOfToolInput('grep_search', '{"pattern":"TODO"}')).toBe('TODO')
    expect(targetOfToolInput('TodoWrite', '{"todos":[]}')).toBeNull()
    expect(targetOfToolInput('bash', 'not json')).toBeNull()
  })

  test('sizes read short', () => {
    expect(formatBytes(900)).toBe('900 B')
    expect(formatBytes(1024)).toBe('1 KB')
    expect(formatBytes(12_595)).toBe('12.3 KB')
    expect(formatBytes(3 * 1024 * 1024)).toBe('3 MB')
    expect(inputSizeDetail(0, 0)).toBeNull()
    expect(inputSizeDetail(1, 10)).toBe('1 line · 10 B')
  })
})
