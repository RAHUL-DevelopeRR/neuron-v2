/**
 * L1, the live transcript: running shell output, exit codes and timers, tool
 * input progress, quiet reads, the status-bar meter, report panels, the todo
 * strip and refusal notices. States are built by running protocol events
 * through the reducer, then the components are rendered.
 */
import { afterEach, describe, expect, test } from 'bun:test'
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import axe from 'axe-core'

import {
  initialSession,
  reduceSession,
  type AccountDetails,
  type SessionAction,
  type SessionState,
  type TodoItem,
  type UiEvent,
} from '@neuron/client'

import { LiveOutput, stripAnsi } from '../src/components/LiveOutput'
import { StatusBar } from '../src/components/Bars'
import { ReportPanel } from '../src/components/ReportPanel'
import { TodoStrip } from '../src/components/TodoStrip'
import { Transcript } from '../src/components/Transcript'

afterEach(cleanup)

const status = (over: Record<string, unknown> = {}): UiEvent =>
  ({
    type: 'status',
    model: 'auto',
    permission_mode: 'default',
    session_id: 's1',
    cwd: '/work/app',
    workspace_trusted: true,
    plan_mode: false,
    context_percent: 12,
    git: null,
    ...over,
  }) as UiEvent

const run = (actions: Array<SessionAction | [SessionAction, number]>, from?: SessionState) =>
  actions.reduce<SessionState>(
    (state, entry, index) => {
      const [action, now] = Array.isArray(entry) ? entry : [entry, 1000 + index]
      return reduceSession(state, action, now)
    },
    from ?? reduceSession(reduceSession(initialSession(), { type: 'ready', protocol: 2, version: '1' } as UiEvent), status()),
  )

const call = (id: string, name: string, input: object): SessionAction => ({
  type: 'tool_call_started',
  id,
  name,
  input: JSON.stringify(input),
  cut_off: false,
})
const result = (id: string, name: string, output: string, is_error = false): SessionAction => ({
  type: 'tool_result',
  id,
  name,
  output,
  is_error,
})
const delta = (id: string, chunk: string, stream: 'stdout' | 'stderr' = 'stdout'): SessionAction => ({
  type: 'tool_output_delta',
  tool_use_id: id,
  stream,
  chunk,
})

const show = (state: SessionState, extra: Partial<React.ComponentProps<typeof Transcript>> = {}) =>
  render(<Transcript items={state.items} busy={state.busy} folded={false} showReasoning onLink={() => {}} {...extra} />)

async function expectClean(container: HTMLElement) {
  const found = await axe.run(container, { rules: { 'color-contrast': { enabled: false }, region: { enabled: false } } })
  expect(found.violations.map((v) => `${v.id}: ${v.nodes[0]?.html}`)).toEqual([])
}

describe('live shell output', () => {
  test('strips ANSI escape sequences', () => {
    expect(stripAnsi('\u001b[31mred\u001b[0m \u001b[1;32mgreen\u001b[0m\u001b]0;title\u0007 ok')).toBe('red green ok')
  })

  test('a running command shows its output as it arrives, stderr tinted', () => {
    const state = run([
      call('t1', 'bash', { command: 'make' }),
      delta('t1', 'compiling\n'),
      delta('t1', '\u001b[31mwarning: x\u001b[0m\n', 'stderr'),
    ])
    const { container } = show(state)
    const live = screen.getByLabelText('Live output')
    expect(live.textContent).toBe('compiling\nwarning: x\n')
    expect(container.querySelectorAll('.live-chunk.stderr')).toHaveLength(1)
    expect(container.querySelector('.live-chunk.stderr')?.textContent).toBe('warning: x\n')
    expect(container.querySelector('.tool-output')).toBeNull()
  })

  test('is not a live region: screen readers hear the card state, not every chunk', () => {
    render(<LiveOutput live={{ chunks: [{ stream: 'stdout', text: 'a' }], dropped: 0 }} />)
    expect(screen.getByRole('region', { name: 'Live output' }).getAttribute('aria-live')).toBe('off')
  })

  test('says how much was dropped from the front', () => {
    render(<LiveOutput live={{ chunks: [{ stream: 'stdout', text: 'tail' }], dropped: 1234 }} />)
    expect(screen.getByText('… 1234 earlier characters not shown')).toBeTruthy()
  })

  test('shows no dropped line when nothing was dropped', () => {
    render(<LiveOutput live={{ chunks: [{ stream: 'stdout', text: 'a' }], dropped: 0 }} />)
    expect(screen.queryByText(/earlier characters/)).toBeNull()
  })

  test('follows new output until the user scrolls up', () => {
    const live = (text: string) => ({ chunks: [{ stream: 'stdout' as const, text }], dropped: 0 })
    const { rerender } = render(<LiveOutput live={live('a')} />)
    const box = screen.getByLabelText('Live output')
    Object.defineProperty(box, 'scrollHeight', { configurable: true, value: 500 })
    Object.defineProperty(box, 'clientHeight', { configurable: true, value: 100 })
    rerender(<LiveOutput live={live('ab')} />)
    expect(box.scrollTop).toBe(500)
    // The user scrolls up.
    box.scrollTop = 10
    fireEvent.scroll(box)
    Object.defineProperty(box, 'scrollHeight', { configurable: true, value: 900 })
    rerender(<LiveOutput live={live('abc')} />)
    expect(box.scrollTop).toBe(10)
  })

  test('the final output replaces the live block when the result arrives', () => {
    const state = run([
      call('t1', 'bash', { command: 'echo hi' }),
      delta('t1', 'hi\n'),
      result('t1', 'bash', JSON.stringify({ stdout: 'hi\n', stderr: '' })),
    ])
    const { container } = show(state)
    expect(screen.queryByLabelText('Live output')).toBeNull()
    expect(container.querySelector('.tool-output')?.textContent).toBe('hi\n')
  })

  test('folded cards still open a running command', () => {
    const state = run([call('t1', 'bash', { command: 'make' }), delta('t1', 'x')])
    show(state, { folded: true })
    expect(screen.getByLabelText('Live output')).toBeTruthy()
  })

  test('PowerShell calls get live output too', () => {
    const state = run([call('t1', 'PowerShell', { command: 'dir' }), delta('t1', 'files')])
    show(state)
    expect(screen.getByLabelText('Live output').textContent).toBe('files')
  })

  test('axe is clean on a running card', async () => {
    const state = run([call('t1', 'bash', { command: 'make' }), delta('t1', 'a\n'), delta('t1', 'b\n', 'stderr')])
    const { container } = show(state)
    await expectClean(container)
  })
})

describe('exit code and duration', () => {
  test('a finished call shows its duration and a tinted non-zero exit code', () => {
    const state = run([
      [call('t1', 'bash', { command: 'false' }), 1000],
      [result('t1', 'bash', JSON.stringify({ stdout: '', stderr: 'no', returnCodeInterpretation: 'exit_code:3' })), 66_000],
    ])
    show(state)
    expect(screen.getByText('1m 05s')).toBeTruthy()
    expect(screen.getByText('exit 3').className).toContain('exit-fail')
  })

  test('a zero exit code is shown untinted', () => {
    const state = run([
      [call('t1', 'bash', { command: 'true' }), 1000],
      [result('t1', 'bash', JSON.stringify({ stdout: 'ok', stderr: '', exitCode: 0 })), 3500],
    ])
    show(state)
    expect(screen.getByText('exit 0').className).not.toContain('exit-fail')
    expect(screen.getByText('2s')).toBeTruthy()
  })

  test('no exit code is shown when the output carries none', () => {
    const state = run([
      [call('t1', 'bash', { command: 'ls' }), 1000],
      [result('t1', 'bash', JSON.stringify({ stdout: 'a', stderr: '' })), 1500],
    ])
    show(state)
    expect(screen.queryByText(/^exit /)).toBeNull()
  })

  test('non-shell cards show neither', () => {
    const state = run([
      [call('t1', 'read_file', { path: 'a' }), 1000],
      [result('t1', 'read_file', '{"file":{"content":"x"}}'), 9000],
    ])
    show(state)
    expect(screen.queryByText('8s')).toBeNull()
  })

  test('a running call ticks every second, and stops when it finishes', async () => {
    const realNow = Date.now
    let clock = 10_000
    Date.now = () => clock
    try {
      const state = run([[call('t1', 'bash', { command: 'sleep 9' }), 10_000]])
      const { rerender } = show(state)
      expect(screen.getByLabelText('running for').textContent).toBe('0s')
      clock = 13_000
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 1100))
      })
      expect(screen.getByLabelText('running for').textContent).toBe('3s')
      const done = run([[result('t1', 'bash', '{"stdout":"","stderr":""}'), 13_000]], state)
      rerender(<Transcript items={done.items} busy={false} folded={false} showReasoning />)
      expect(screen.queryByLabelText('running for')).toBeNull()
      expect(screen.getByText('3s')).toBeTruthy()
    } finally {
      Date.now = realNow
    }
  })
})

describe('tool input progress', () => {
  const writingState = () =>
    run([
      { type: 'tool_input_started', id: 'w1', name: 'write_file' } as SessionAction,
      {
        type: 'tool_input_progress',
        id: 'w1',
        bytes: 900,
        lines: 42,
        target: 'src/app.ts',
        preview: 'one\ntwo\nthree\nfour\nfive',
      } as SessionAction,
    ])

  test('shows the target, line count, the last lines and a busy indicator', () => {
    const { container } = show(writingState())
    expect(screen.getByText('Writing src/app.ts…')).toBeTruthy()
    expect(screen.getByText('42 lines')).toBeTruthy()
    const preview = container.querySelector('.writing-preview')
    expect(preview?.textContent).toContain('five')
    expect(preview?.textContent).toContain('three')
    expect(preview?.textContent).not.toContain('one')
    expect(container.querySelector('.writing-busy')).toBeTruthy()
  })

  test('becomes the normal card in place when the call starts', () => {
    const state = run([call('w1', 'write_file', { path: 'src/app.ts', content: 'a\nb' })], writingState())
    const { container } = show(state)
    expect(screen.queryByText(/^Writing /)).toBeNull()
    expect(container.querySelectorAll('[data-testid="tool-card"]')).toHaveLength(1)
    expect(container.querySelector('.tool-state')?.textContent).toBe('running')
  })

  test('axe is clean while writing', async () => {
    const { container } = show(writingState())
    await expectClean(container)
  })
})

describe('quiet reads', () => {
  const done = (id: string, name: string, input: object, output = '{}') => [call(id, name, input), result(id, name, output)]

  test('finished reads and searches fold into one expandable line', () => {
    const state = run([
      ...done('r1', 'read_file', { path: 'a.ts' }, '{"file":{"content":"x"}}'),
      ...done('r2', 'read_file', { path: 'b.ts' }, '{"file":{"content":"x"}}'),
      ...done('r3', 'grep_search', { pattern: 'foo' }),
    ])
    const { container } = show(state)
    const head = screen.getByRole('button', { name: /Read 2 files, searched once/ })
    expect(head.getAttribute('aria-expanded')).toBe('false')
    expect(container.querySelectorAll('[data-testid="tool-card"]')).toHaveLength(0)
    fireEvent.click(head)
    expect(head.getAttribute('aria-expanded')).toBe('true')
    expect(container.querySelectorAll('[data-testid="tool-card"]')).toHaveLength(3)
  })

  test('never folds a failed, running, writing or waiting call', () => {
    const state = run([
      ...done('r1', 'read_file', { path: 'a.ts' }),
      ...done('r2', 'read_file', { path: 'b.ts' }),
      call('r3', 'read_file', { path: 'c.ts' }),
      call('r4', 'read_file', { path: 'd.ts' }),
      result('r4', 'read_file', 'not found', true),
      { type: 'tool_input_started', id: 'r5', name: 'read_file' } as SessionAction,
    ])
    const { container } = show(state)
    expect(screen.getByRole('button', { name: /Read 2 files/ })).toBeTruthy()
    expect(container.querySelectorAll('[data-testid="tool-card"]')).toHaveLength(3)
  })

  test('a single finished read stays a normal card', () => {
    const state = run(done('r1', 'read_file', { path: 'a.ts' }))
    const { container } = show(state)
    expect(container.querySelectorAll('[data-testid="tool-card"]')).toHaveLength(1)
  })

  test('other messages break a run', () => {
    const state = run([
      ...done('r1', 'read_file', { path: 'a.ts' }),
      ...done('r2', 'read_file', { path: 'b.ts' }),
      { type: 'text_delta', text: 'hello' } as SessionAction,
      ...done('r3', 'read_file', { path: 'c.ts' }),
    ])
    const { container } = show(state)
    expect(screen.getByRole('button', { name: /Read 2 files/ })).toBeTruthy()
    expect(container.querySelectorAll('[data-testid="tool-card"]')).toHaveLength(1)
  })

  test('axe is clean on a folded run', async () => {
    const state = run([...done('r1', 'read_file', { path: 'a.ts' }), ...done('r2', 'glob_search', { pattern: '*.ts' })])
    const { container } = show(state)
    await expectClean(container)
  })
})

describe('refusal notices', () => {
  test('a refusal has its own quiet style; a plain notice does not', () => {
    const state = run([
      { type: 'notice', message: 'The model declined this request.', kind: 'refusal' } as SessionAction,
      { type: 'notice', message: 'Compacted.' } as SessionAction,
    ])
    const { container } = show(state)
    const refusal = container.querySelector('.notice.refusal')
    expect(refusal?.textContent).toContain('The model declined this request.')
    expect(refusal?.getAttribute('role')).toBe('note')
    expect(container.querySelectorAll('.notice')).toHaveLength(2)
    expect(container.querySelectorAll('.notice.refusal')).toHaveLength(1)
    expect(container.querySelector('.message.assistant')).toBeNull()
  })
})

const account = (over: Partial<AccountDetails> = {}): AccountDetails => ({
  email: 'a@b.c',
  name: 'A',
  plan: 'pro',
  plan_name: 'Pro',
  source: 'device',
  expires_at: null,
  limits: { requests: 100, tokens: 1_000_000 },
  usage_today: { requests: 10, tokens: 1000 },
  resets_at: '2026-10-24T18:30:00Z',
  credit_balance_cents: null,
  suspended: false,
  dashboard_url: 'https://zero-x.live/dashboard',
  pricing_url: 'https://zero-x.live/pricing',
  usage_updated_at: null,
  usage_error: null,
  ...over,
})
const bar = (state: SessionState) => render(<StatusBar state={state} onPane={() => {}} />)
const withAccount = (over: Partial<AccountDetails> = {}) =>
  run([{ type: 'account', signed_in: true, account: account(over) } as SessionAction])

describe('status-bar meter', () => {
  test('shows context, session tokens, and no cost', () => {
    const state = run([
      status({ context_percent: 42 }),
      {
        type: 'usage',
        turn: { input: 1, output: 1 },
        cumulative: { input: 12_345, output: 2_500 },
        estimated: false,
      } as unknown as SessionAction,
    ])
    const { container } = bar(state)
    expect(screen.getByText('Context 42%').className).not.toContain('warn')
    expect(container.textContent).toContain('↑12k')
    expect(container.textContent).toContain('↓2.5k')
    expect(container.textContent).not.toMatch(/\$|cost/i)
  })

  test('tints context from 80%', () => {
    bar(run([status({ context_percent: 80 })]))
    expect(screen.getByText('Context 80%').className).toContain('warn')
  })

  test('shows the streaming output tokens while a turn runs', () => {
    const state = run([{ type: 'local/slash', command: 'x', args: '' } as SessionAction, { type: 'output_progress', output_tokens: 1800 } as SessionAction])
    const { container } = bar(state)
    expect(container.textContent).toContain('writing 1.8k tokens')
  })

  test('shows the served model only when it differs', () => {
    const { container, unmount } = bar(run([status({ model: 'auto', served_model: 'fast-1' })]))
    expect(container.textContent).toContain('answered by fast-1')
    unmount()
    const other = bar(run([status({ model: 'fast-1', served_model: 'fast-1' })]))
    expect(other.container.textContent).not.toContain('answered by')
  })

  test('no quota line below 80%', () => {
    const { container } = bar(withAccount())
    expect(container.querySelector('.quota')).toBeNull()
  })

  test('a quiet warning from 80% of requests', () => {
    const { container } = bar(withAccount({ usage_today: { requests: 85, tokens: 10 } }))
    const quota = container.querySelector('.quota')
    expect(quota?.textContent).toContain("85% of today's requests used")
    expect(quota?.className).not.toContain('limit')
  })

  test('a quiet warning from 80% of tokens', () => {
    const { container } = bar(withAccount({ usage_today: { requests: 1, tokens: 900_000 } }))
    expect(container.querySelector('.quota')?.textContent).toContain("90% of today's tokens used")
  })

  test('at the limit it says when it resets, in local time', () => {
    const details = account({ usage_today: { requests: 100, tokens: 5 } })
    const { container } = bar(run([{ type: 'account', signed_in: true, account: details } as SessionAction]))
    const quota = container.querySelector('.quota')
    const time = new Date(details.resets_at!).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
    expect(quota?.textContent).toBe(`Limit reached, resets at ${time}`)
    expect(quota?.className).toContain('limit')
  })

  test('is quiet when the limits are unknown', () => {
    expect(bar(withAccount({ limits: null })).container.querySelector('.quota')).toBeNull()
  })

  test('low-priority parts are marked so narrow widths can hide them', () => {
    const { container } = bar(run([status({ model: 'auto', served_model: 'x' })]))
    expect(container.querySelectorAll('.status-low').length).toBeGreaterThan(0)
  })

  test('axe is clean with a limit reached', async () => {
    const { container } = bar(withAccount({ usage_today: { requests: 100, tokens: 5 } }))
    await expectClean(container)
  })
})

describe('report panels', () => {
  const report = (over: Record<string, unknown> = {}) =>
    run([
      {
        type: 'report',
        command: 'usage',
        title: 'Usage today',
        sections: [
          { heading: 'Requests', markdown: 'You made **74** requests.' },
          { heading: null, markdown: 'Plain intro' },
        ],
        links: [{ label: 'Open the dashboard', url: 'https://zero-x.live/dashboard' }],
        ...over,
      } as SessionAction,
    ])

  test('renders the title, sections as Markdown, and links as buttons', () => {
    const opened: string[] = []
    const { container } = show(report(), { onLink: (url) => opened.push(url) })
    expect(screen.getByRole('heading', { name: /Usage today/ })).toBeTruthy()
    expect(screen.getByRole('heading', { name: 'Requests' })).toBeTruthy()
    expect(container.querySelector('.report strong')?.textContent).toBe('74')
    fireEvent.click(screen.getByRole('button', { name: 'Open the dashboard' }))
    expect(opened).toEqual(['https://zero-x.live/dashboard'])
  })

  test('collapses and expands', () => {
    show(report())
    const toggle = screen.getByRole('button', { name: /Usage today/ })
    expect(toggle.getAttribute('aria-expanded')).toBe('true')
    fireEvent.click(toggle)
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    expect(screen.queryByText('Plain intro')).toBeNull()
    fireEvent.click(toggle)
    expect(screen.getByText('Plain intro')).toBeTruthy()
  })

  test('copies the whole report as text', async () => {
    let copied = ''
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: async (text: string) => void (copied = text) },
    })
    show(report())
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Copy report' }))
    })
    expect(copied).toBe('Usage today\n\nRequests\nYou made **74** requests.\n\nPlain intro')
  })

  test('works without links or sections', () => {
    render(<ReportPanel item={{ kind: 'report', key: 'k', command: 'doctor', title: 'Doctor', sections: [], links: [] }} />)
    expect(screen.getByRole('heading', { name: /Doctor/ })).toBeTruthy()
  })

  test('axe is clean', async () => {
    const { container } = show(report())
    await expectClean(container)
  })
})

describe('todo strip', () => {
  const todos: TodoItem[] = [
    { content: 'Plan', status: 'completed', active_form: 'Planning' },
    { content: 'Build', status: 'in_progress', active_form: 'Building the thing' },
    { content: 'Test', status: 'pending', active_form: 'Testing' },
  ]

  test('is hidden without todos', () => {
    const { container } = render(<TodoStrip todos={[]} />)
    expect(container.firstChild).toBeNull()
  })

  test('shows progress and the active step, and expands to the checklist', () => {
    render(<TodoStrip todos={todos} />)
    const toggle = screen.getByRole('button', { name: /1\/3 done · Building the thing/ })
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    expect(screen.queryByRole('list')).toBeNull()
    fireEvent.click(toggle)
    expect(toggle.getAttribute('aria-expanded')).toBe('true')
    const items = within(screen.getByRole('list')).getAllByRole('listitem')
    expect(items.map((li) => li.textContent)).toEqual(['✓ Plan', '▸ Building the thing', '○ Test'])
  })

  test('falls back to the content when there is no active form', () => {
    render(<TodoStrip todos={[{ content: 'Build', status: 'in_progress', active_form: null }]} />)
    expect(screen.getByRole('button', { name: /0\/1 done · Build/ })).toBeTruthy()
  })

  test('with nothing in progress it shows only the count', () => {
    render(<TodoStrip todos={todos.map((todo) => ({ ...todo, status: 'completed' }))} />)
    expect(screen.getByRole('button').textContent).toContain('3/3 done')
    expect(screen.getByRole('button').textContent).not.toContain('·')
  })

  test('axe is clean, collapsed and expanded', async () => {
    const { container } = render(<TodoStrip todos={todos} />)
    await expectClean(container)
    fireEvent.click(screen.getByRole('button'))
    await expectClean(container)
  })
})
