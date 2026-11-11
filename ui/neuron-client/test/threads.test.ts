/*
 * Adapted from Freebuff common/src/desktop-cloud/threads.test.ts (commit 0dc95cb),
 * Copyright Codebuff, Apache License 2.0 (see NOTICE). Changed for Neuron: the
 * cloud/local kinds, pull requests, resolved threads and the panel layout are
 * gone; the digest is built from a `SessionState`, and approvals wait like questions.
 */
import { describe, expect, test } from 'bun:test'

import type { UiEvent } from '../src/protocol'
import { initialSession, reduceSession, type SessionAction, type SessionState } from '../src/session'
import {
  THREAD_DIGEST_TEXT_LIMIT,
  digestText,
  sessionDigest,
  threadGroup,
  threadStatusLine,
  threadStepCount,
  type ThreadDigest,
} from '../src/threads'

const digest = (patch: Partial<ThreadDigest> = {}): ThreadDigest => ({
  id: 'thread-1',
  title: 'Fix login',
  runState: 'idle',
  outcome: 'completed',
  steps: [],
  statusLine: null,
  lastMessage: null,
  waiting: null,
  ...patch,
})

const question = { kind: 'question' as const, text: 'Ship it?' }

describe('threadGroup', () => {
  test('waiting outranks working, and a failed idle turn waits too', () => {
    expect(threadGroup(digest({ runState: 'running', waiting: question }))).toBe('waiting')
    expect(threadGroup(digest({ outcome: 'failed' }))).toBe('waiting')
    expect(threadGroup(digest({ runState: 'running', outcome: 'failed' }))).toBe('working')
    expect(threadGroup(digest({ runState: 'running' }))).toBe('working')
    expect(threadGroup(digest())).toBe('idle')
    // A stopped engine cannot be helped by an answer: it is idle until it starts again.
    expect(threadGroup(digest({ runState: 'stopped', outcome: 'failed' }))).toBe('idle')
  })
})

test('status line falls back through the shared fields', () => {
  expect(threadStatusLine(digest({ statusLine: 'Running tests', waiting: question }))).toBe('Ship it?')
  expect(threadStatusLine(digest({ statusLine: 'Running tests' }))).toBe('Running tests')
  expect(threadStatusLine(digest({ runState: 'running', steps: [{ text: 'Write tests', state: 'active' }] }))).toBe('Write tests')
  expect(threadStatusLine(digest({ runState: 'running' }))).toBe('Working')
  expect(threadStatusLine(digest({ runState: 'stopped' }))).toBe('Not running')
  expect(threadStatusLine(digest({ outcome: 'failed' }))).toBe('Turn failed')
  expect(threadStatusLine(digest({ outcome: 'stopped' }))).toBe('Stopped')
  expect(threadStatusLine(digest({ lastMessage: 'All done' }))).toBe('All done')
  expect(threadStatusLine(digest())).toBe('Finished')
  expect(threadStatusLine(digest({ outcome: null }))).toBe('Not started')
})

test('step count', () => {
  expect(threadStepCount(digest())).toBeNull()
  expect(
    threadStepCount(
      digest({
        steps: [
          { text: 'a', state: 'done' },
          { text: 'b', state: 'done' },
          { text: 'c', state: 'active' },
          { text: 'd', state: 'pending' },
        ],
      }),
    ),
  ).toEqual({ done: 2, total: 4 })
})

test('digest text is one bounded line', () => {
  expect(digestText('  two\n  lines ')).toBe('two lines')
  expect(digestText('')).toBeNull()
  expect(digestText(null)).toBeNull()
  const long = digestText('x'.repeat(THREAD_DIGEST_TEXT_LIMIT + 10))!
  expect(long).toHaveLength(THREAD_DIGEST_TEXT_LIMIT)
  expect(long.endsWith('…')).toBe(true)
})

// ---------------------------------------------------------------- from a session

const run = (actions: SessionAction[], from: SessionState = initialSession()) => actions.reduce((state, action) => reduceSession(state, action, 0), from)
const ready: UiEvent = { type: 'ready', protocol: 1, version: '6.3.0' }
const of = (state: SessionState) => sessionDigest(state, { id: 'c1', title: 'Fix login' })
const todos = (...statuses: string[]): UiEvent => ({
  type: 'todos',
  items: statuses.map((status, index) => ({ content: `step ${index + 1}`, status, active_form: status === 'in_progress' ? `Doing step ${index + 1}` : null })),
})

describe('sessionDigest', () => {
  test('a fresh session has not started', () => {
    const d = of(run([ready]))
    expect(d).toMatchObject({ id: 'c1', title: 'Fix login', runState: 'idle', outcome: null, steps: [], waiting: null })
    expect(threadStatusLine(d)).toBe('Not started')
    expect(threadGroup(d)).toBe('idle')
  })

  test('steps come from the task list', () => {
    const d = of(run([ready, { type: 'local/submitted', text: 'go' }, todos('completed', 'completed', 'in_progress', 'pending')]))
    expect(threadStepCount(d)).toEqual({ done: 2, total: 4 })
    expect(d.steps[2]).toEqual({ text: 'Doing step 3', state: 'active' })
    // Running, with nothing more recent than the plan: the active step is the line.
    expect(threadStatusLine(d)).toBe('Doing step 3')
  })

  test('the latest tool call is the running line, in plain words', () => {
    const d = of(
      run([
        ready,
        { type: 'local/submitted', text: 'test it' },
        todos('in_progress'),
        { type: 'tool_call_started', id: 't1', name: 'bash', input: '{"command":"bun test"}', cut_off: false },
      ]),
    )
    expect(d.runState).toBe('running')
    expect(threadStatusLine(d)).toMatch(/bun test/)
    expect(threadGroup(d)).toBe('working')
  })

  test('a pending approval waits on the user', () => {
    const d = of(
      run([
        ready,
        { type: 'local/submitted', text: 'clean' },
        { type: 'tool_call_started', id: 't1', name: 'bash', input: '{"command":"rm -rf target"}', cut_off: false },
        {
          type: 'permission_requested',
          id: 4,
          request: { tool: 'bash', current_mode: 'workspace-write', required_mode: 'danger-full-access', reason: null, input: '{"command":"rm -rf target"}', session_prefixes: [] },
        },
      ]),
    )
    expect(d.waiting?.kind).toBe('approval')
    expect(threadStatusLine(d)).toMatch(/^Waiting for your OK/)
    expect(threadStatusLine(d)).toMatch(/rm -rf target/)
    expect(threadGroup(d)).toBe('waiting')
  })

  test('a question waits on the user', () => {
    const d = of(run([ready, { type: 'local/submitted', text: 'plan' }, { type: 'question', id: 3, question: 'Which crate?', options: ['api', 'tools'] }]))
    expect(d.waiting).toEqual({ kind: 'question', text: 'Which crate?' })
    expect(threadStatusLine(d)).toBe('Which crate?')
  })

  test('a turn that ended in an error needs the user, without the raw message', () => {
    const d = of(
      run([
        ready,
        { type: 'local/submitted', text: 'go' },
        { type: 'error', message: 'upstream SomeProvider said 401 invalid_session', detail: { code: 'invalid_session', status: 401, upgrade_url: null, resets_at: null, retry_after_seconds: null } },
        { type: 'turn_finished' },
      ]),
    )
    expect(d.outcome).toBe('failed')
    expect(threadGroup(d)).toBe('waiting')
    const line = threadStatusLine(d)
    expect(line).not.toMatch(/SomeProvider|401|invalid_session/)
    expect(line).toMatch(/sign in/i)
  })

  test('an error that is not about the service still reads as a failed turn', () => {
    const d = of(run([ready, { type: 'local/submitted', text: 'go' }, { type: 'error', message: 'boom at provider X' }, { type: 'turn_finished' }]))
    expect(d.outcome).toBe('failed')
    expect(threadStatusLine(d)).toBe('Turn failed')
  })

  test('a new prompt after a failure starts clean', () => {
    const failed = run([ready, { type: 'local/submitted', text: 'go' }, { type: 'error', message: 'boom' }, { type: 'turn_finished' }])
    const d = of(run([{ type: 'local/submitted', text: 'again' }, { type: 'text_delta', text: 'Done: fixed it.\nMore detail' }, { type: 'turn_finished' }], failed))
    expect(d.outcome).toBe('completed')
    expect(threadGroup(d)).toBe('idle')
    expect(threadStatusLine(d)).toBe('Done: fixed it.')
  })

  test('a stopped engine is not running', () => {
    const d = of(run([ready, { type: 'local/disconnected', error: 'gone' }]))
    expect(d.runState).toBe('stopped')
    expect(threadStatusLine(d)).toBe('Not running')
  })
})
