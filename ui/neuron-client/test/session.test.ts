import { describe, expect, test } from 'bun:test'

import type { UiEvent } from '../src/protocol'
import {
  changedPaths,
  hasFeature,
  openQuestionItem,
  runningSubagents,
  initialSession,
  needsSignIn,
  openQuestion,
  reduceSession,
  type SessionAction,
  type SessionState,
  type TranscriptItem,
} from '../src/session'

const cwd = 'C:\\work\\ws'
const status = (overrides: Record<string, unknown> = {}): UiEvent => ({
  type: 'status',
  model: 'claude-sonnet-4-6',
  permission_mode: 'workspace-write',
  session_id: 'session-1',
  cwd,
  workspace_trusted: false,
  plan_mode: false,
  context_percent: 0,
  git: null,
  signed_in: false,
  sign_in_required: false,
  account: null,
  ...overrides,
})

const run = (actions: SessionAction[], from: SessionState = initialSession()) => actions.reduce(reduceSession, from)
const started = () => run([{ type: 'ready', protocol: 1, version: '6.3.0' }, status()])
const kinds = (items: TranscriptItem[]) => items.map((item) => item.kind)
const tool = (state: SessionState, index = 0) => state.items.filter((item) => item.kind === 'tool')[index] as Extract<TranscriptItem, { kind: 'tool' }>

describe('a turn', () => {
  test('streamed text, a write, its result and the end of the turn', () => {
    const state = run(
      [
        { type: 'local/submitted', text: 'PARITY_SCENARIO:write_file_allowed' },
        {
          type: 'tool_call_started',
          id: 'toolu_write',
          name: 'write_file',
          input: '{"path":"generated/output.txt","content":"created by mock service\\n"}',
          cut_off: false,
        },
        {
          type: 'tool_result',
          id: 'toolu_write',
          name: 'write_file',
          output: JSON.stringify({ type: 'create', filePath: `${cwd}\\generated\\output.txt`, bytes: 24, lines: 1, summary: 'Created output.txt' }),
          is_error: false,
        },
        { type: 'text_delta', text: 'write_file ' },
        { type: 'text_delta', text: 'succeeded' },
        { type: 'text_block_end' },
        { type: 'usage', turn: { input: 26, output: 11, cache_read: 0, cache_write: 0 }, cumulative: { input: 26, output: 11, cache_read: 0, cache_write: 0 }, estimated: false },
        { type: 'turn_finished' },
      ],
      started(),
    )
    expect(state.connection).toBe('ready')
    expect(state.version).toBe('6.3.0')
    expect(kinds(state.items)).toEqual(['user', 'tool', 'assistant'])
    expect(state.busy).toBe(false)
    const write = tool(state)
    expect(write.state).toBe('done')
    expect(write.view.kind).toBe('write')
    expect(write.view.target).toBe('generated/output.txt')
    expect(write.view.diff?.lines.map((line) => line.text)).toEqual(['created by mock service'])
    expect(write.view.output).toBe('Created output.txt')
    expect(state.items[2]).toMatchObject({ kind: 'assistant', text: 'write_file succeeded', streaming: false })
    expect(state.usage?.cumulative.output).toBe(11)
    expect(changedPaths(state)).toEqual(['generated/output.txt'])
  })

  test('busy from the submit to turn_finished', () => {
    let state = run([{ type: 'local/submitted', text: 'hi' }], started())
    expect(state.busy).toBe(true)
    state = run([{ type: 'turn_finished' }], state)
    expect(state.busy).toBe(false)
  })

  test('reasoning, then text, are separate blocks', () => {
    const state = run(
      [
        { type: 'reasoning_started', chars: null, redacted: false },
        { type: 'reasoning_delta', text: 'think' },
        { type: 'reasoning_delta', text: 'ing' },
        { type: 'text_delta', text: 'answer' },
        { type: 'turn_finished' },
      ],
      started(),
    )
    expect(state.items.map((item) => [item.kind, 'text' in item ? item.text : ''])).toEqual([
      ['reasoning', 'thinking'],
      ['assistant', 'answer'],
    ])
    expect(state.items.every((item) => !('streaming' in item) || !item.streaming)).toBe(true)
  })

  test('parallel calls get their results by id', () => {
    const state = run(
      [
        { type: 'tool_call_started', id: 'r', name: 'read_file', input: '{"path":"fixture.txt"}', cut_off: false },
        { type: 'tool_call_started', id: 'g', name: 'grep_search', input: '{"pattern":"parity","path":"fixture.txt"}', cut_off: false },
        { type: 'tool_result', id: 'g', name: 'grep_search', output: '{"mode":"count","numFiles":1,"filenames":["fixture.txt"],"content":null,"numMatches":2}', is_error: false },
        { type: 'tool_result', id: 'r', name: 'read_file', output: '{"type":"text","file":{"filePath":"fixture.txt","content":"alpha\\nbeta"}}', is_error: false },
      ],
      started(),
    )
    expect(tool(state, 0).view.output).toBe('alpha\nbeta')
    expect(tool(state, 1).view.output).toBe('2 matches in 1 file\nfixture.txt')
  })

  test('an edit shows the diff it will make, then the one it made', () => {
    let state = run(
      [{ type: 'tool_call_started', id: 'e', name: 'edit_file', input: '{"path":"C:\\\\work\\\\ws\\\\src\\\\main.rs","old_string":"let a = 1;","new_string":"let a = 2;"}', cut_off: false }],
      started(),
    )
    expect(tool(state).view.target).toBe('src\\main.rs')
    expect(tool(state).view.diff?.lines.map((line) => line.kind)).toEqual(['remove', 'add'])
    state = run(
      [
        {
          type: 'tool_result',
          id: 'e',
          name: 'edit_file',
          output: JSON.stringify({ filePath: 'src/main.rs', diff: '@@ -7,1 +7,1 @@\n-let a = 1;\n+let a = 2;\n', structuredPatch: [] }),
          is_error: false,
        },
      ],
      state,
    )
    expect(tool(state).view.diff?.lines.map((line) => [line.kind, line.oldNumber ?? line.newNumber ?? null])).toEqual([
      ['hunk', null],
      ['remove', 7],
      ['add', 7],
    ])
  })
})

describe('permissions', () => {
  const bash = '{"command":"printf ok | tee approved.txt","timeout":1000}'
  const asked = () =>
    run(
      [
        { type: 'local/submitted', text: 'go' },
        { type: 'tool_call_started', id: 'b', name: 'bash', input: bash, cut_off: false },
        {
          type: 'permission_requested',
          id: 1,
          request: {
            tool: 'bash',
            current_mode: 'workspace-write',
            required_mode: 'danger-full-access',
            reason: '`tee` is not a known read-only command',
            input: bash,
            session_prefixes: ['tee'],
          },
        },
      ],
      started(),
    )

  test('a request attaches to the call it is for', () => {
    const state = asked()
    expect(state.permissions).toHaveLength(1)
    expect(state.permissions[0].toolKey).toBe(tool(state).key)
    expect(tool(state).state).toBe('awaiting_permission')
    expect(tool(state).permissionId).toBe(1)
  })

  test('allowing runs the call; its result closes it', () => {
    let state = run([{ type: 'local/permission_answered', id: 1, decision: { kind: 'allow_once' } }], asked())
    expect(state.permissions).toEqual([])
    expect(tool(state).state).toBe('running')
    state = run([{ type: 'tool_result', id: 'b', name: 'bash', output: '{"stdout":"ok","stderr":""}', is_error: false }], state)
    expect(tool(state).state).toBe('done')
    expect(tool(state).view.output).toBe('ok')
  })

  test('denying marks the call denied, also after its error result', () => {
    let state = run([{ type: 'local/permission_answered', id: 1, decision: { kind: 'deny', note: 'not now' } }], asked())
    expect(tool(state).state).toBe('denied')
    state = run([{ type: 'tool_result', id: 'b', name: 'bash', output: "tool 'bash' denied by the user: not now", is_error: true }], state)
    expect(tool(state).state).toBe('denied')
  })

  test('another client answering closes the request here too', () => {
    const state = run([{ type: 'local/permission_resolved', id: 1 }], asked())
    expect(state.permissions).toEqual([])
  })

  test('the end of the turn drops requests and fails calls left open', () => {
    const state = run([{ type: 'turn_finished' }], asked())
    expect(state.permissions).toEqual([])
    expect(tool(state).state).toBe('error')
  })

  test('a repeated request id is ignored', () => {
    const state = asked()
    expect(run([{ type: 'permission_requested', id: 1, request: state.permissions[0].request }], state).permissions).toHaveLength(1)
  })
})

describe('the rest of the protocol', () => {
  test('a question from the agent, answered by the next submit without a new turn', () => {
    let state = run([{ type: 'local/submitted', text: 'plan it' }, { type: 'notice', message: 'Question: Which crate?\n  1. api\n  2. tools' }], started())
    expect(openQuestion(state)).toBe('Which crate?\n  1. api\n  2. tools')
    state = run([{ type: 'local/submitted', text: '2' }], state)
    expect(openQuestion(state)).toBeNull()
    expect(state.busy).toBe(true)
  })

  test('sign-in: the signed-out stage, the code, then the session starting', () => {
    let state = run([{ type: 'ready', protocol: 1, version: '6.3.0' }, status({ sign_in_required: true, session_id: '' })])
    expect(needsSignIn(state)).toBe(true)
    state = run(
      [
        { type: 'local/slash', command: 'login', args: '' },
        { type: 'sign_in_code', url: 'https://zero-x.live/device', url_complete: null, user_code: 'ABCD-1234', expires_in_secs: 600, browser_opened: false },
      ],
      state,
    )
    expect(state.signInCode?.user_code).toBe('ABCD-1234')
    state = run([{ type: 'session_starting' }], state)
    expect(state.signInCode).toBeNull()
    expect(state.sessionStarting).toBe(true)
    state = run([{ type: 'turn_finished' }, status({ signed_in: true, account: { email: null, plan: 'free', source: 'keychain', expires_at: null } })], state)
    expect(needsSignIn(state)).toBe(false)
    expect(state.sessionStarting).toBe(false)
  })

  test('a switch to another session is marked in the transcript', () => {
    let state = run([{ type: 'local/submitted', text: 'hi' }, { type: 'turn_finished' }, status()], started())
    expect(state.items.map((item) => item.kind)).toEqual(['user'])
    state = run([{ type: 'local/slash', command: 'clear', args: '--confirm' }, { type: 'turn_finished' }, status({ session_id: 'session-2' })], state)
    expect(state.items.at(-1)).toMatchObject({ kind: 'notice', text: 'New session session-2' })
    // Signed out has no session yet; the first one is not a switch.
    const signedOut = run([{ type: 'ready', protocol: 1, version: '6.3.0' }, status({ session_id: '' }), status({ session_id: 'session-9' })])
    expect(signedOut.items).toEqual([])
  })

  test('a launch error before ready is the connection error', () => {
    const state = run([{ type: 'error', message: 'neuron is running from a very broad directory' }])
    expect(state.connectionError).toContain('broad directory')
  })

  test('the engine going away fails what was running', () => {
    const state = run(
      [{ type: 'tool_call_started', id: 'x', name: 'bash', input: '{"command":"sleep 9"}', cut_off: false }, { type: 'local/disconnected', error: 'exit code 1' }],
      started(),
    )
    expect(state.connection).toBe('exited')
    expect(state.connectionError).toBe('exit code 1')
    expect(tool(state).state).toBe('error')
  })

  test('a result for a call this client never saw still shows', () => {
    const state = run([{ type: 'tool_result', id: 'late', name: 'read_file', output: 'text', is_error: false }], started())
    expect(tool(state)).toMatchObject({ id: 'late', state: 'done', output: 'text' })
  })

  test('an MCP call names its server and tool', () => {
    const state = run([{ type: 'tool_call_started', id: 'm', name: 'mcp__playwright__browser_navigate', input: '{"url":"https://example.com"}', cut_off: false }], started())
    expect(tool(state).view).toMatchObject({ kind: 'mcp', mcp: { server: 'playwright', tool: 'browser_navigate' } })
  })
})

describe('E1: sessions, transcripts, questions, tasks, sub-agents, trust', () => {
  test('ready carries the engine features', () => {
    const state = run([{ type: 'ready', protocol: 1, version: '6.4.0', features: ['capabilities', 'sessions'] }])
    expect(hasFeature(state, 'sessions')).toBe(true)
    expect(hasFeature(state, 'trust_workspace')).toBe(false)
    expect(run([{ type: 'ready', protocol: 1, version: '6.3.0' }]).features).toEqual([])
  })

  test('a transcript replaces what is shown, with calls and their results', () => {
    const before = run([{ type: 'local/submitted', text: 'old chat' }, { type: 'turn_finished' }, status()], started())
    const state = run(
      [
        {
          type: 'transcript',
          session_id: 'session-2',
          truncated: true,
          entries: [
            { kind: 'user', text: 'write it' },
            { kind: 'assistant', text: 'Writing.' },
            { kind: 'tool_call', id: 'w', name: 'write_file', input: '{"path":"a.txt","content":"hi\\n"}' },
            { kind: 'tool_result', id: 'w', name: 'write_file', output: '{"type":"create","summary":"Created a.txt"}', is_error: false },
            { kind: 'tool_call', id: 'b', name: 'bash', input: '{"command":"rm -rf /"}' },
            { kind: 'tool_result', id: 'b', name: 'bash', output: "tool 'bash' denied by the user: no", is_error: true },
            { kind: 'tool_call', id: 'x', name: 'read_file', input: '{"path":"a.txt"}' },
          ],
        },
        status({ session_id: 'session-2' }),
      ],
      before,
    )
    expect(kinds(state.items)).toEqual(['notice', 'user', 'assistant', 'tool', 'tool', 'tool'])
    expect(state.items[0]).toMatchObject({ kind: 'notice', text: 'Earlier messages of this chat are not shown.' })
    expect(tool(state, 0)).toMatchObject({ state: 'done', view: { kind: 'write', target: 'a.txt', output: 'Created a.txt' } })
    expect(tool(state, 1).state).toBe('denied')
    expect(tool(state, 2).state).toBe('error')
    // No "New session" line: the transcript already switched the session.
    expect(state.items.some((item) => item.kind === 'notice' && item.text.startsWith('New session'))).toBe(false)
  })

  test('the session list', () => {
    const sessions = [{ id: 's1', title: 'Fix the build', model: 'sonnet', message_count: 4, updated_at_ms: 1, current: true, branch: null }]
    expect(run([{ type: 'sessions', current: 's1', sessions }], started()).sessions).toEqual(sessions)
  })

  test('a structured question with options, answered by id', () => {
    let state = run([{ type: 'local/submitted', text: 'plan' }, { type: 'question', id: 3, question: 'Which crate?', options: ['api', 'tools'] }], started())
    expect(openQuestionItem(state)).toMatchObject({ id: 3, text: 'Which crate?', options: ['api', 'tools'], answered: false })
    state = run([{ type: 'local/answered', id: 9, text: 'stale' }], state)
    expect(openQuestionItem(state)?.id).toBe(3)
    state = run([{ type: 'local/answered', id: 3, text: 'tools' }], state)
    expect(openQuestionItem(state)).toBeNull()
    expect(state.items.at(-1)).toMatchObject({ kind: 'user', text: 'tools' })
  })

  test('the task list', () => {
    const items = [
      { content: 'Run the tests', status: 'in_progress', active_form: 'Running the tests' },
      { content: 'Fix the build', status: 'pending', active_form: null },
    ]
    expect(run([{ type: 'todos', items }], started()).todos).toEqual(items)
  })

  test('a sub-agent on its Agent call, running then finished', () => {
    let state = run(
      [
        { type: 'tool_call_started', id: 'a1', name: 'Agent', input: '{"description":"Audit MCP","prompt":"..."}', cut_off: false },
        { type: 'subagent_started', id: 'a1', description: 'Audit MCP', subagent_type: 'Explore' },
      ],
      started(),
    )
    expect(runningSubagents(state).map((item) => item.id)).toEqual(['a1'])
    expect(tool(state).subagent).toMatchObject({ description: 'Audit MCP', type: 'Explore', running: true })
    state = run([{ type: 'subagent_finished', id: 'a1', status: 'completed', is_error: false, result: 'Only stdio.', note: null }], state)
    expect(runningSubagents(state)).toEqual([])
    expect(tool(state).subagent).toMatchObject({ running: false, status: 'completed', result: 'Only stdio.', type: 'Explore' })
  })

  test('trusting the workspace asks for a restart, which the next ready clears', () => {
    let state = run([{ type: 'workspace_trusted', cwd: cwd, restart_required: true }], started())
    expect(state.restartRequired).toBe(true)
    expect(state.status?.workspace_trusted).toBe(true)
    state = run([{ type: 'ready', protocol: 1, version: '6.3.0', features: [] }], state)
    expect(state.restartRequired).toBe(false)
  })
})

describe('E2: extensions', () => {
  test('the inventory, which a restarted engine clears', () => {
    const mcp_servers = [
      { name: 'docs', scope: 'user', transport: 'stdio', status: 'ready', tools: ['lookup'], error: null },
      { name: 'ghost', scope: 'project', transport: 'stdio', status: 'failed', tools: [], error: 'spawn failed' },
    ]
    const plugins = [{ id: 'fmt@builtin', name: 'fmt', version: '1.0.0', description: 'Formats', kind: 'builtin', enabled: true }]
    const skills = [{ name: 'plan', description: 'Plans work', source: 'Project', active: true }]
    let state = run([{ type: 'extensions', mcp_servers, plugins, skills, agents: [] }], started())
    expect(state.extensions).toEqual({ mcpServers: mcp_servers, plugins, skills, agents: [] })
    state = run([{ type: 'ready', protocol: 1, version: '6.3.0', features: ['extensions'] }], state)
    expect(state.extensions).toBeNull()
  })
})

describe('WP3: gateway problems', () => {
  test('a failing prompt keeps its text and the problem, until the next prompt', () => {
    let state = run(
      [
        { type: 'local/submitted', text: 'refactor the parser', attachments: [{ kind: 'file', path: 'src/parse.rs' }] },
        { type: 'error', message: 'gateway error insufficient_quota: Daily quota reached' },
        { type: 'turn_finished' },
      ],
      started(),
    )
    expect(state.problem?.kind).toBe('quota')
    expect(state.unsent).toEqual({ text: 'refactor the parser', attachments: [{ kind: 'file', path: 'src/parse.rs' }] })
    const error = state.items.at(-1)
    expect(error).toMatchObject({ kind: 'error', problem: { kind: 'quota' } })
    state = run([{ type: 'local/submitted', text: 'try again', attachments: [] }], state)
    expect(state.problem).toBeNull()
    expect(state.unsent).toBeNull()
  })

  test('a reply already started: nothing to put back', () => {
    const state = run(
      [
        { type: 'local/submitted', text: 'explain', attachments: [] },
        { type: 'text_delta', text: 'The parser…' },
        { type: 'error', message: 'Could not reach Zero-X: connection reset' },
      ],
      started(),
    )
    expect(state.problem?.kind).toBe('unreachable')
    expect(state.unsent).toBeNull()
  })

  test('other errors are plain error lines', () => {
    const state = run([{ type: 'error', message: 'unknown slash command /frobnicate' }], started())
    expect(state.problem).toBeNull()
    expect(state.items.at(-1)).toEqual({ kind: 'error', key: expect.any(String), text: 'unknown slash command /frobnicate' })
  })
})
