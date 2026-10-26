/**
 * Presence vectors for `neuron engine --share` (Rust, `rust/crates/neuron-share`): sessions as a
 * local host streams them (a `snapshot`, then `action`s) and the presence the apps compute after
 * each step with the real reducer and `presenceOf`. The Rust presence tracker must give the same.
 *
 *   bun run test/fixtures/presence-vectors.ts > test/fixtures/presence-vectors.json
 *
 * test/presence-vectors.test.ts checks the committed JSON is still what this prints.
 */

import { initialSession, reduceSession, type SessionAction, type SessionState } from '@neuron/client'

import { presenceOf } from '../../src/relay/presence'

type Step = { snapshot: { seq: number; state: SessionState } } | { action: { seq: number; action: SessionAction } }

const CWD = '/work/neuron-v2'
const status = (overrides: Record<string, unknown> = {}) =>
  ({
    type: 'status',
    model: 'glm-5.2',
    permission_mode: 'workspace-write',
    session_id: 's1',
    cwd: CWD,
    workspace_trusted: true,
    plan_mode: false,
    context_percent: 3,
    git: { branch: 'main', dirty: false, ahead: 0, behind: 0 },
    ...overrides,
  }) as unknown as SessionAction
const request = (input: string) => ({ tool: 'Bash', current_mode: 'read-only', required_mode: 'workspace-write', reason: null, input, session_prefixes: [] })

const a = (action: Record<string, unknown>) => action as unknown as SessionAction

/** A whole turn and its aftermath, from an empty session. */
const turn: SessionAction[] = [
  a({ type: 'local/connecting' }),
  a({ type: 'ready', version: '7.0.0', protocol: 1, features: ['questions'] }),
  status(),
  a({ type: 'local/submitted', text: '  Fix   the build\nplease  ' }),
  a({ type: 'text_delta', text: 'Looking.' }),
  a({ type: 'reasoning_started', chars: null, redacted: false }),
  a({ type: 'reasoning_delta', text: 'hmm' }),
  a({ type: 'tool_call_started', id: 't1', name: 'Bash', input: '{"command":"cargo build"}', cut_off: false }),
  a({ type: 'permission_requested', id: 1, request: request('{ "command": "cargo build" }') }),
  a({ type: 'local/permission_answered', id: 1, decision: { kind: 'allow_once' } }),
  a({ type: 'tool_result', id: 't1', name: 'Bash', output: 'ok', is_error: false }),
  a({ type: 'text_delta', text: 'Built.' }),
  a({ type: 'text_block_end' }),
  a({ type: 'turn_finished' }),
  a({ type: 'question', id: 7, question: 'Which branch?', options: ['main', 'dev'] }),
  a({ type: 'local/answered', id: 7, text: 'main' }),
  a({ type: 'local/submitted', text: 'Now run the tests' }),
  a({ type: 'tool_call_started', id: 't2', name: 'Bash', input: '{"command":"cargo test"}', cut_off: false }),
  a({ type: 'permission_requested', id: 2, request: request('{"command":"cargo test"}') }),
  a({ type: 'local/permission_answered', id: 2, decision: { kind: 'deny', note: 'not now' } }),
  a({ type: 'tool_result', id: 't2', name: 'Bash', output: 'Permission denied by user', is_error: true }),
  a({ type: 'error', message: 'The model stopped.' }),
  a({ type: 'turn_finished' }),
  a({ type: 'sessions', sessions: [{ id: 's1', title: 'Build fixes', model: 'glm-5.2', message_count: 4, updated_at_ms: 1, current: true, branch: 'main' }] }),
  a({ type: 'notice', message: 'Question: proceed with the release?' }),
  a({ type: 'local/submitted', text: 'yes' }),
  a({ type: 'turn_finished' }),
  a({ type: 'local/slash', command: 'compact', args: '' }),
  a({ type: 'tool_result', id: 't9', name: 'Read', output: 'late', is_error: false }),
  a({ type: 'turn_finished' }),
  a({ type: 'tool_call_started', id: 't3', name: 'Edit', input: '{"path":"a.rs"}', cut_off: false }),
  a({ type: 'permission_requested', id: 3, request: { ...request('{"path":"a.rs"}'), tool: 'Edit' } }),
  a({ type: 'local/disconnected', error: 'The engine stopped.' }),
  a({ type: 'local/connecting' }),
  a({ type: 'ready', version: '7.0.0', protocol: 1, features: [] }),
  status({ session_id: 's2', git: null, model: 'kimi-k2' }),
  a({
    type: 'transcript',
    session_id: 's2',
    truncated: true,
    entries: [
      { kind: 'user', text: 'Earlier prompt' },
      { kind: 'tool_call', id: 'x1', name: 'Bash', input: '{}' },
      { kind: 'tool_result', id: 'x1', output: 'fine', is_error: false },
      { kind: 'assistant', text: 'Done before.' },
    ],
  }),
  a({ type: 'session_starting' }),
  a({ type: 'turn_finished' }),
  a({ type: 'sign_in_code', url: 'https://zero-x.live/device', url_complete: null, user_code: 'ABCD-EFGH', expires_in_secs: 600, browser_opened: false }),
  a({ type: 'turn_finished' }),
  a({ type: 'local/reset' }),
]

/** A phone that attaches mid-turn: the snapshot holds a pending permission linked to its call. */
const midTurn: SessionAction[] = [
  a({ type: 'ready', version: '7.0.0', protocol: 1, features: [] }),
  status({ cwd: 'C:\\Users\\rahul\\proj\\', git: { branch: 'feat/x', dirty: true, ahead: 1, behind: 0 } }),
  a({ type: 'local/submitted', text: 'Edit the config' }),
  a({ type: 'tool_call_started', id: 'e1', name: 'Write', input: '{"path":"c.json"}', cut_off: false }),
  a({ type: 'permission_requested', id: 5, request: { ...request('{"path":"c.json"}'), tool: 'Write' } }),
  a({ type: 'permission_requested', id: 6, request: { ...request('{"path":"other"}'), tool: 'Write' } }),
]
const afterAttach: SessionAction[] = [
  a({ type: 'tool_result', id: 'e1', name: 'Write', output: 'written', is_error: false }),
  a({ type: 'local/permission_resolved', id: 6 }),
  a({ type: 'tool_call_started', id: 'e2', name: 'Write', input: '{"x":1}', cut_off: true }),
  a({ type: 'tool_result', name: 'Write', output: 'cut', is_error: true }),
  a({ type: 'text_delta', text: 'Done.' }),
  a({ type: 'turn_finished' }),
]

function run(name: string, fallbackCwd: string, before: SessionAction[], after: SessionAction[]) {
  let state = initialSession()
  let seq = 0
  for (const action of before) {
    seq += 1
    state = reduceSession(state, action)
  }
  const steps: Array<Step & { expect: unknown }> = []
  const expect = () => {
    const { updated_at_ms: _at, ...presence } = presenceOf(state, seq, fallbackCwd, 0)
    return presence
  }
  steps.push({ snapshot: { seq, state }, expect: expect() })
  for (const action of after) {
    seq += 1
    state = reduceSession(state, action)
    steps.push({ action: { seq, action }, expect: expect() })
  }
  return { name, fallback_cwd: fallbackCwd, steps }
}

const vectors = {
  generated_by: 'ui/neuron-mobile/test/fixtures/presence-vectors.ts',
  scenarios: [run('a whole session from the start', '/fallback/project', [], turn), run('attached mid-turn', 'C:\\fallback', midTurn, afterAttach)],
}

if (import.meta.main) console.log(JSON.stringify(vectors, null, 1))

export default vectors
