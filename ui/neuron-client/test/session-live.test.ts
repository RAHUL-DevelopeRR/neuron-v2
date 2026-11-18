import { describe, expect, test } from 'bun:test'

import type { ChangedFile, CheckpointInfo, RewoundEvent, UiEvent } from '../src/protocol'
import {
  LIVE_OUTPUT_LIMIT,
  chatActivity,
  initialSession,
  liveText,
  reduceSession,
  type SessionAction,
  type SessionState,
  type TranscriptItem,
} from '../src/session'

type Tool = Extract<TranscriptItem, { kind: 'tool' }>

const cwd = '/work/app'
const status: UiEvent = {
  type: 'status',
  model: 'auto',
  permission_mode: 'default',
  session_id: 's1',
  cwd,
  workspace_trusted: true,
  plan_mode: false,
  context_percent: 0,
  git: null,
}

/** Applies actions in order; each one is received at the time given (default: its index + 1000). */
const run = (actions: Array<SessionAction | [SessionAction, number]>, from: SessionState = initialSession()) =>
  actions.reduce<SessionState>((state, entry, index) => {
    const [action, now] = Array.isArray(entry) ? entry : [entry, 1000 + index]
    return reduceSession(state, action, now)
  }, from)

const ready = (): SessionState => run([{ type: 'ready', protocol: 1, version: '1' }, status])
const tools = (state: SessionState) => state.items.filter((item): item is Tool => item.kind === 'tool')
const writing = (id = 't1', name = 'write_file'): SessionAction => ({ type: 'tool_input_started', id, name })
const callStarted = (id = 't1', input = '{}', cut_off = false, name = 'write_file'): SessionAction => ({
  type: 'tool_call_started',
  id,
  name,
  input,
  cut_off,
})
const delta = (id: string, stream: 'stdout' | 'stderr', chunk: string): SessionAction => ({
  type: 'tool_output_delta',
  tool_use_id: id,
  stream,
  chunk,
})
const usage = (output: number) => ({ input: 1, output, cache_read: 0, cache_write: 0 })

describe('tools written live', () => {
  test('tool_input_started shows the call as writing, with the time it began', () => {
    const state = run([[writing(), 5000]], ready())
    expect(tools(state)).toHaveLength(1)
    expect(tools(state)[0]).toMatchObject({
      id: 't1',
      name: 'write_file',
      state: 'writing',
      input: '',
      output: null,
      permissionId: null,
      progress: null,
      live: null,
      startedAt: 5000,
      endedAt: null,
    })
    expect(tools(state)[0].view).toMatchObject({ kind: 'write', verb: 'Write', target: '' })
  })

  test('tool_input_progress records how far the model got and refreshes the target', () => {
    const state = run(
      [
        writing(),
        {
          type: 'tool_input_progress',
          id: 't1',
          name: 'write_file',
          bytes: 300,
          lines: 7,
          target: `${cwd}/src/main.rs`,
          preview: 'fn main() {}',
        },
      ],
      ready(),
    )
    expect(tools(state)[0].progress).toEqual({ bytes: 300, lines: 7, target: `${cwd}/src/main.rs`, preview: 'fn main() {}' })
    expect(tools(state)[0].view.target).toBe('src/main.rs')
    expect(tools(state)[0].state).toBe('writing')
  })

  test('progress without a target keeps the view, and progress for an unknown call is ignored', () => {
    const base = run([writing()], ready())
    const next = reduceSession(base, { type: 'tool_input_progress', id: 't1', name: 'write_file', bytes: 5, lines: 0, target: null, preview: null })
    expect(tools(next)[0].view.target).toBe('')
    expect(tools(next)[0].progress).toEqual({ bytes: 5, lines: 0, target: null, preview: null })
    const unknown = reduceSession(base, { type: 'tool_input_progress', id: 'zz', name: 'x', bytes: 5, lines: 0, target: null, preview: null })
    expect(unknown).toBe(base)
  })

  test('tool_call_started for a writing call updates it in place and keeps startedAt', () => {
    const input = JSON.stringify({ path: `${cwd}/a.txt`, content: 'one\ntwo' })
    const state = run([[writing(), 100], [callStarted('t1', input), 400]], ready())
    expect(tools(state)).toHaveLength(1)
    const item = tools(state)[0]
    expect(item.state).toBe('running')
    expect(item.input).toBe(input)
    expect(item.view.target).toBe('a.txt')
    expect(item.view.diff).toBeDefined()
    expect(item.startedAt).toBe(100)
  })

  test('a cut-off call that was being written ends cut_off', () => {
    const state = run([writing(), callStarted('t1', '{}', true)], ready())
    expect(tools(state)[0].state).toBe('cut_off')
  })

  test('a call that started without a writing phase begins at tool_call_started', () => {
    const state = run([[callStarted('t9'), 777]], ready())
    expect(tools(state)[0]).toMatchObject({ state: 'running', startedAt: 777, endedAt: null })
  })

  test('without a time, startedAt and endedAt are null', () => {
    const state = [writing(), { type: 'tool_result', id: 't1', name: 'write_file', output: '{}', is_error: false } as SessionAction].reduce(
      (acc, action) => reduceSession(acc, action),
      ready(),
    )
    expect(tools(state)[0].startedAt).toBeNull()
    expect(tools(state)[0].endedAt).toBeNull()
  })

  test('tool_result sets endedAt and finishes a call that was still writing', () => {
    const state = run(
      [[writing(), 100], [callStarted(), 200], [{ type: 'tool_result', id: 't1', name: 'write_file', output: 'ok', is_error: false }, 900]],
      ready(),
    )
    expect(tools(state)[0]).toMatchObject({ state: 'done', startedAt: 100, endedAt: 900 })
  })

  test('a result for a call never seen gets both times from the result', () => {
    const state = run([[{ type: 'tool_result', id: 'late', name: 'bash', output: 'x', is_error: false }, 50]], ready())
    expect(tools(state)[0]).toMatchObject({ id: 'late', state: 'done', startedAt: 50, endedAt: 50 })
  })

  test('turn_finished and a lost engine close writing calls as errors', () => {
    const finished = run([writing(), { type: 'turn_finished' }], ready())
    expect(tools(finished)[0].state).toBe('error')
    const lost = run([writing(), { type: 'local/disconnected', error: null }], ready())
    expect(tools(lost)[0].state).toBe('error')
  })
})

describe('live output', () => {
  const withTool = () => run([callStarted('t1', '{"command":"ls"}', false, 'bash')], ready())

  test('chunks append, merging with the last one of the same stream', () => {
    const state = run([delta('t1', 'stdout', 'a'), delta('t1', 'stdout', 'b'), delta('t1', 'stderr', 'E'), delta('t1', 'stdout', 'c')], withTool())
    expect(tools(state)[0].live).toEqual({
      chunks: [
        { stream: 'stdout', text: 'ab' },
        { stream: 'stderr', text: 'E' },
        { stream: 'stdout', text: 'c' },
      ],
      dropped: 0,
    })
    expect(liveText(tools(state)[0].live)).toBe('abEc')
  })

  test('a delta for an unknown call, or an empty one, changes nothing', () => {
    const base = withTool()
    expect(reduceSession(base, delta('nope', 'stdout', 'x'))).toBe(base)
    expect(reduceSession(base, delta('t1', 'stdout', ''))).toBe(base)
  })

  test('only the last LIVE_OUTPUT_LIMIT characters are kept, and dropped counts the rest', () => {
    expect(LIVE_OUTPUT_LIMIT).toBe(64 * 1024)
    let state = withTool()
    state = reduceSession(state, delta('t1', 'stdout', 'a'.repeat(LIVE_OUTPUT_LIMIT)))
    expect(tools(state)[0].live?.dropped).toBe(0)
    state = reduceSession(state, delta('t1', 'stderr', 'bbbb'))
    const live = tools(state)[0].live!
    expect(live.dropped).toBe(4)
    expect(liveText(live).length).toBe(LIVE_OUTPUT_LIMIT)
    expect(liveText(live).endsWith('abbbb'.slice(1))).toBe(true)
    expect(live.chunks.map((chunk) => chunk.stream)).toEqual(['stdout', 'stderr'])
  })

  test('whole chunks drop from the front first', () => {
    let state = withTool()
    state = reduceSession(state, delta('t1', 'stderr', 'x'.repeat(10)))
    state = reduceSession(state, delta('t1', 'stdout', 'y'.repeat(LIVE_OUTPUT_LIMIT)))
    const live = tools(state)[0].live!
    expect(live.chunks).toEqual([{ stream: 'stdout', text: 'y'.repeat(LIVE_OUTPUT_LIMIT) }])
    expect(live.dropped).toBe(10)
  })

  test('one delta larger than the limit keeps its tail', () => {
    const state = reduceSession(withTool(), delta('t1', 'stdout', 'z'.repeat(LIVE_OUTPUT_LIMIT + 5)))
    const live = tools(state)[0].live!
    expect(liveText(live).length).toBe(LIVE_OUTPUT_LIMIT)
    expect(live.dropped).toBe(5)
  })

  test('liveText of nothing is empty', () => {
    expect(liveText(null)).toBe('')
    expect(liveText(undefined)).toBe('')
  })
})

describe('reports, refusals and prompts', () => {
  test('a report becomes a transcript item and closes the open stream', () => {
    const state = run(
      [
        { type: 'text_delta', text: 'hi' },
        {
          type: 'report',
          command: 'usage',
          title: 'Usage',
          sections: [{ heading: null, markdown: 'x' }],
          links: [{ label: 'Dashboard', url: 'https://example.test/d' }],
        },
      ],
      ready(),
    )
    expect(state.items.map((item) => item.kind)).toEqual(['assistant', 'report'])
    expect(state.items[0]).toMatchObject({ streaming: false })
    expect(state.items[1]).toMatchObject({
      kind: 'report',
      command: 'usage',
      title: 'Usage',
      sections: [{ heading: null, markdown: 'x' }],
      links: [{ label: 'Dashboard', url: 'https://example.test/d' }],
    })
  })

  test('a refusal notice keeps its kind; a plain one has none', () => {
    const state = run([{ type: 'notice', message: 'declined', kind: 'refusal' }, { type: 'notice', message: 'plain' }], ready())
    expect(state.items[0]).toMatchObject({ kind: 'notice', text: 'declined', noticeKind: 'refusal' })
    expect((state.items[1] as { noticeKind?: unknown }).noticeKind).toBeUndefined()
  })

  test('submitted prompts are prompts; slash commands and answers are not', () => {
    const state = run(
      [
        { type: 'local/submitted', text: 'fix it' },
        { type: 'turn_finished' },
        { type: 'local/slash', command: 'diff', args: '' },
        { type: 'turn_finished' },
        { type: 'question', id: 4, question: 'Which?', options: [] },
        { type: 'local/answered', id: 4, text: 'this one' },
        { type: 'question', id: 5, question: 'Again?', options: [] },
        { type: 'local/submitted', text: 'typed answer' },
      ],
      ready(),
    )
    const users = state.items.filter((item) => item.kind === 'user') as Array<Extract<TranscriptItem, { kind: 'user' }>>
    expect(users.map((user) => [user.text, user.prompt])).toEqual([
      ['fix it', true],
      ['/diff', false],
      ['this one', false],
      ['typed answer', false],
    ])
  })

  test('restored transcripts mark user entries as prompts and tools with empty live fields', () => {
    const state = run(
      [
        {
          type: 'transcript',
          session_id: 's2',
          truncated: false,
          entries: [
            { kind: 'user', text: 'hello' },
            { kind: 'tool_call', id: 'c1', name: 'bash', input: '{"command":"ls"}' },
            { kind: 'tool_result', id: 'c1', name: 'bash', output: 'x', is_error: false },
          ],
        },
      ],
      ready(),
    )
    expect(state.items[0]).toMatchObject({ kind: 'user', prompt: true })
    expect(tools(state)[0]).toMatchObject({ progress: null, live: null, startedAt: null, endedAt: null, state: 'done' })
  })
})

describe('turn progress', () => {
  test('output_progress sets the streaming tokens; request_usage records a request and restarts them', () => {
    let state = run([{ type: 'local/submitted', text: 'go' }], ready())
    expect(state.turnProgress).toBeNull()
    state = reduceSession(state, { type: 'output_progress', output_tokens: 40 })
    expect(state.turnProgress).toEqual({ outputTokens: 40, requests: [] })
    state = reduceSession(state, { type: 'output_progress', output_tokens: 90 })
    expect(state.turnProgress?.outputTokens).toBe(90)
    state = reduceSession(state, { type: 'request_usage', usage: usage(95) })
    expect(state.turnProgress).toEqual({ outputTokens: 0, requests: [usage(95)] })
    state = reduceSession(state, { type: 'output_progress', output_tokens: 10 })
    state = reduceSession(state, { type: 'request_usage', usage: usage(12) })
    expect(state.turnProgress).toEqual({ outputTokens: 0, requests: [usage(95), usage(12)] })
  })

  test('it survives turn_finished and resets on the next prompt, a slash command, ready and reset', () => {
    const progressed = run(
      [{ type: 'local/submitted', text: 'go' }, { type: 'output_progress', output_tokens: 7 }, { type: 'turn_finished' }],
      ready(),
    )
    expect(progressed.turnProgress).toEqual({ outputTokens: 7, requests: [] })
    expect(reduceSession(progressed, { type: 'local/submitted', text: 'again' }).turnProgress).toBeNull()
    expect(reduceSession(progressed, { type: 'local/slash', command: 'x', args: '' }).turnProgress).toBeNull()
    expect(reduceSession(progressed, { type: 'ready', protocol: 1, version: '1' }).turnProgress).toBeNull()
    expect(reduceSession(progressed, { type: 'local/reset' }).turnProgress).toBeNull()
  })

  test('an answer to a question does not reset it', () => {
    const state = run(
      [
        { type: 'local/submitted', text: 'go' },
        { type: 'question', id: 1, question: 'q', options: [] },
        { type: 'output_progress', output_tokens: 3 },
        { type: 'local/answered', id: 1, text: 'a' },
      ],
      ready(),
    )
    expect(state.turnProgress).toEqual({ outputTokens: 3, requests: [] })
  })
})

describe('git views and checkpoints', () => {
  const files: ChangedFile[] = [{ path: 'a.rs', status: 'modified', added: 1, removed: 0 }]
  const initialViews = (state: SessionState) => [state.changes, state.diff, state.commitProposal, state.checkpoints, state.rewound]

  test('everything starts null', () => {
    expect(initialViews(initialSession())).toEqual([null, null, null, null, null])
  })

  test('changes, diff, commit proposal and checkpoints are kept as the engine sent them', () => {
    const diff: UiEvent = { type: 'diff', title: 'Changes', files: [], truncated: false }
    const proposal: UiEvent = { type: 'commit_proposal', message: 'm', branch: 'main', files, stage_all: true }
    const items: CheckpointInfo[] = [{ id: 1, created_at_ms: 1, prompt_preview: 'p', files: [], shell_commands: [] }]
    const state = run([{ type: 'changes', files }, diff, proposal, { type: 'checkpoints', items }], ready())
    expect(state.changes).toEqual(files)
    expect(state.diff).toEqual(diff)
    expect(state.commitProposal).toEqual(proposal)
    expect(state.checkpoints).toEqual(items)
  })

  test('an empty changes event is an empty list, not null', () => {
    expect(run([{ type: 'changes', files: [] }], ready()).changes).toEqual([])
  })

  test('commit_sent and commit_dismissed clear the proposal', () => {
    const proposed = run([{ type: 'commit_proposal', message: 'm', branch: null, files, stage_all: false }], ready())
    expect(reduceSession(proposed, { type: 'local/commit_sent', message: 'm' }).commitProposal).toBeNull()
    expect(reduceSession(proposed, { type: 'local/commit_dismissed' }).commitProposal).toBeNull()
  })

  test('ready, reset and transcript clear all five', () => {
    const full = run(
      [
        { type: 'changes', files },
        { type: 'diff', title: 't', files: [], truncated: false },
        { type: 'commit_proposal', message: 'm', branch: null, files, stage_all: false },
        { type: 'checkpoints', items: [] },
        rewound({ dry_run: true }),
      ],
      ready(),
    )
    expect(initialViews(full).every((view) => view !== null)).toBe(true)
    for (const action of [
      { type: 'ready', protocol: 1, version: '1' },
      { type: 'local/reset' },
      { type: 'transcript', session_id: 's', entries: [], truncated: false },
    ] as SessionAction[]) {
      expect(initialViews(reduceSession(full, action))).toEqual([null, null, null, null, null])
    }
  })
})

function rewound(overrides: Partial<RewoundEvent> = {}): RewoundEvent {
  return {
    type: 'rewound',
    checkpoint: 1,
    mode: 'code_and_chat',
    dry_run: false,
    restored: [],
    removed: [],
    not_restored: [],
    shell_commands: [],
    removed_turns: 0,
    prompt: null,
    ...overrides,
  }
}

describe('rewinding', () => {
  const chat = () =>
    run(
      [
        { type: 'local/submitted', text: 'first' },
        { type: 'text_delta', text: 'reply 1' },
        { type: 'turn_finished' },
        { type: 'local/slash', command: 'diff', args: '' },
        { type: 'turn_finished' },
        { type: 'local/submitted', text: 'second' },
        { type: 'text_delta', text: 'reply 2' },
        { type: 'turn_finished' },
        { type: 'local/submitted', text: 'third' },
        { type: 'text_delta', text: 'reply 3' },
        { type: 'turn_finished' },
        { type: 'checkpoints', items: [] },
      ],
      ready(),
    )
  const texts = (state: SessionState) => state.items.map((item) => ('text' in item ? item.text : item.kind))

  test('a dry run only records the answer', () => {
    const before = chat()
    const after = reduceSession(before, rewound({ dry_run: true, removed_turns: 2, prompt: 'second' }))
    expect(after.rewound).toMatchObject({ dry_run: true, removed_turns: 2 })
    expect(after.items).toEqual(before.items)
    expect(after.unsent).toBeNull()
    expect(after.checkpoints).toEqual([])
  })

  test('a real rewind drops the last removed_turns prompts with everything after them', () => {
    const after = reduceSession(chat(), rewound({ removed_turns: 2, prompt: 'second' }))
    expect(texts(after)).toEqual(['first', 'reply 1', '/diff'])
    expect(after.unsent).toEqual({ text: 'second', attachments: [] })
    expect(after.checkpoints).toBeNull()
    expect(after.rewound).toMatchObject({ dry_run: false, removed_turns: 2 })
  })

  test('removing one prompt drops only the last', () => {
    const after = reduceSession(chat(), rewound({ removed_turns: 1, prompt: 'third' }))
    expect(texts(after)).toEqual(['first', 'reply 1', '/diff', 'second', 'reply 2'])
  })

  test('more removed turns than prompts drops everything from the first prompt on', () => {
    const after = reduceSession(chat(), rewound({ removed_turns: 9, prompt: 'first' }))
    expect(after.items).toEqual([])
  })

  test('a rewind that kept the chat leaves the transcript and the message box alone', () => {
    const before = chat()
    const after = reduceSession(before, rewound({ mode: 'code_only', removed_turns: 0, prompt: null, restored: ['a.rs'] }))
    expect(after.items).toEqual(before.items)
    expect(after.unsent).toBeNull()
    expect(after.checkpoints).toBeNull()
  })

  test('the prompt flag decides what a prompt is; without it a leading slash does not', () => {
    const state: SessionState = {
      ...ready(),
      items: [
        { kind: 'user', key: 'a', text: '/looks like a command', attachments: [], prompt: true },
        { kind: 'assistant', key: 'b', text: 'r', streaming: false },
        { kind: 'user', key: 'c', text: 'old style', attachments: [] },
        { kind: 'user', key: 'd', text: '/slash', attachments: [] },
        { kind: 'user', key: 'e', text: 'flagged off', attachments: [], prompt: false },
      ],
    }
    const after = reduceSession(state, rewound({ removed_turns: 1 }))
    expect(after.items.map((item) => item.key)).toEqual(['a', 'b'])
  })

  test('no prompts at all: nothing is dropped', () => {
    const state = run([{ type: 'notice', message: 'n' }], ready())
    expect(reduceSession(state, rewound({ removed_turns: 1, prompt: 'p' })).items).toEqual(state.items)
  })
})

describe('chatActivity', () => {
  test('idle, running, needs_you and stopped', () => {
    const base = ready()
    expect(chatActivity(base)).toBe('idle')
    expect(chatActivity({ ...base, busy: true })).toBe('running')
    expect(
      chatActivity({ ...base, busy: true, permissions: [{ id: 1, toolKey: null, request: { tool: 'bash', current_mode: 'a', required_mode: 'b', reason: null, input: '{}', session_prefixes: [] } }] }),
    ).toBe('needs_you')
    expect(chatActivity(run([{ type: 'question', id: 1, question: 'q', options: [] }], base))).toBe('needs_you')
    expect(chatActivity(run([{ type: 'question', id: 1, question: 'q', options: [] }, { type: 'local/answered', id: 1, text: 'a' }], base))).toBe('idle')
    expect(chatActivity(reduceSession(base, { type: 'local/disconnected', error: null }))).toBe('stopped')
  })

  test('a stopped engine is stopped even with an old question open', () => {
    const asked = run([{ type: 'question', id: 1, question: 'q', options: [] }, { type: 'local/disconnected', error: 'x' }], ready())
    expect(chatActivity(asked)).toBe('stopped')
  })
})

describe('reused tool-call ids', () => {
  const result = (id: string, output: string, name = 'bash'): SessionAction => ({ type: 'tool_result', id, name, output, is_error: false })
  const twoTurns = (extra: SessionAction[] = []) =>
    run(
      [
        { type: 'local/submitted', text: 'one' },
        writing('call_0', 'bash'),
        callStarted('call_0', '{"command":"a"}', false, 'bash'),
        delta('call_0', 'stdout', 'first live'),
        result('call_0', 'first out'),
        { type: 'turn_finished' },
        { type: 'local/submitted', text: 'two' },
        writing('call_0', 'bash'),
        callStarted('call_0', '{"command":"b"}', false, 'bash'),
        delta('call_0', 'stdout', 'second live'),
        ...extra,
        result('call_0', 'second out'),
      ],
      ready(),
    )

  test('two turns that both use call_0 keep both cards, each with its own output', () => {
    const state = twoTurns()
    const calls = tools(state)
    expect(calls).toHaveLength(2)
    expect(calls.map((c) => c.output)).toEqual(['first out', 'second out'])
    expect(calls.map((c) => c.state)).toEqual(['done', 'done'])
    expect(calls.map((c) => c.input)).toEqual(['{"command":"a"}', '{"command":"b"}'])
  })

  test('a duplicate tool_input_started or tool_call_started for an open call is ignored', () => {
    const state = run([writing('call_0', 'bash'), writing('call_0', 'bash'), callStarted('call_0', '{}', false, 'bash'), callStarted('call_0', '{"x":1}', false, 'bash')], ready())
    expect(tools(state)).toHaveLength(1)
    expect(tools(state)[0].state).toBe('running')
    expect(tools(state)[0].input).toBe('{}')
  })

  test('a call that finished does not take the next writing event', () => {
    const state = run([callStarted('call_0', '{}', false, 'bash'), result('call_0', 'o'), writing('call_0', 'bash')], ready())
    expect(tools(state).map((c) => c.state)).toEqual(['done', 'writing'])
  })

  test('progress and sub-agent events go to the open call with the id', () => {
    const state = run(
      [
        callStarted('call_0', '{}', false, 'agent'),
        result('call_0', 'o', 'agent'),
        callStarted('call_0', '{}', false, 'agent'),
        { type: 'subagent_started', id: 'call_0', description: 'd', subagent_type: null },
        { type: 'tool_input_progress', id: 'call_0', bytes: 3, lines: 1, target: null, preview: null },
      ] as SessionAction[],
      ready(),
    )
    const [first, second] = tools(state)
    expect(first.subagent).toBeUndefined()
    expect(second.subagent?.running).toBe(true)
    expect(second.progress?.bytes).toBe(3)
  })
})

describe('live output after a result', () => {
  test('tool_result frees the live output but keeps the full output', () => {
    const state = run(
      [callStarted('t1', '{}', false, 'bash'), delta('t1', 'stdout', 'abc'), { type: 'tool_result', id: 't1', name: 'bash', output: 'abc', is_error: false }],
      ready(),
    )
    expect(tools(state)[0].live).toBeNull()
    expect(tools(state)[0].output).toBe('abc')
  })

  test('trimming never splits a surrogate pair', () => {
    const emoji = '\u{1F600}'
    // One unit over the limit: the cut would land between the pair's halves.
    const text = emoji.repeat(LIVE_OUTPUT_LIMIT / 2) + 'x'
    const state = run([callStarted('t1', '{}', false, 'bash'), delta('t1', 'stdout', text)], ready())
    const out = tools(state)[0].live!.chunks.map((c) => c.text).join('')
    const code = out.charCodeAt(0)
    expect(code >= 0xdc00 && code <= 0xdfff).toBe(false)
    expect(out.length).toBeLessThanOrEqual(LIVE_OUTPUT_LIMIT)
    expect(out.startsWith(emoji)).toBe(true)
    expect(out.endsWith('x')).toBe(true)
  })
})

describe('rewinding with pending permissions', () => {
  test('a real rewind drops permissions whose call it removed', () => {
    const request = { tool: 'bash', current_mode: 'workspace-write', required_mode: 'danger-full-access', reason: 'r', input: '{"command":"a"}', session_prefixes: [] }
    const before = run(
      [
        { type: 'local/submitted', text: 'one' },
        { type: 'turn_finished' },
        { type: 'local/submitted', text: 'two' },
        callStarted('t1', '{"command":"a"}', false, 'bash'),
        { type: 'permission_requested', id: 9, request },
        { type: 'permission_requested', id: 10, request: { ...request, tool: 'other' } },
      ] as SessionAction[],
      ready(),
    )
    expect(before.permissions.map((p) => p.id)).toEqual([9, 10])
    const dry = reduceSession(before, rewound({ dry_run: true, removed_turns: 1 }))
    expect(dry.permissions).toHaveLength(2)
    const after = reduceSession(before, rewound({ removed_turns: 1 }))
    expect(after.permissions.map((p) => p.id)).toEqual([10])
  })
})
