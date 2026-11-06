import { afterEach, describe, expect, test } from 'bun:test'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

import type { ClientMessage } from '@neuron/client'

import { App } from '../src/App'
import { FakeEngine, fakePlatform, type FakePlatformOptions } from './fake-platform'

afterEach(() => {
  cleanup()
  try {
    localStorage.clear()
  } catch {
    // No storage: nothing to clear.
  }
})

async function start(statusOverrides: Record<string, unknown> = {}, features?: string[], options?: FakePlatformOptions) {
  const engine = new FakeEngine(statusOverrides, features)
  const platform = fakePlatform(engine, options)
  render(<App platform={platform} />)
  await screen.findByLabelText('Message Neuron')
  return { engine, platform }
}

const emit = (engine: FakeEngine, ...events: Parameters<FakeEngine['emit']>[0][]) =>
  act(() => {
    for (const event of events) engine.emit(event)
  })

const type = (text: string) => {
  const box = screen.getByLabelText('Message Neuron')
  fireEvent.change(box, { target: { value: text, selectionStart: text.length } })
  fireEvent.keyDown(box, { key: 'Enter' })
}

/** The last command of this type the UI sent. */
const sent = (engine: FakeEngine, type: string) =>
  engine.commands().filter((message): message is Extract<ClientMessage, { kind: 'command' }> => message.kind === 'command' && message.type === type).at(-1)

describe('Neuron Desktop: chatting', () => {
  test('on start it says it takes questions and asks for the chat list', async () => {
    const { engine } = await start()
    expect(sent(engine, 'capabilities')).toEqual({ kind: 'command', type: 'capabilities', features: ['questions'] })
    await waitFor(() => expect(sent(engine, 'list_sessions')).toBeTruthy())
  })

  test('a prompt goes to the engine and the reply streams in', async () => {
    const { engine } = await start()
    expect(document.querySelector('.chat-header .git')?.textContent).toContain('⎇ main')
    type('explain the build')
    expect(sent(engine, 'submit')).toEqual({ kind: 'command', type: 'submit', text: 'explain the build', attachments: [] })
    expect(screen.getByTestId('user-message').textContent).toBe('explain the build')
    expect(screen.getByRole('button', { name: /stop/i })).toBeTruthy()

    await emit(engine, { type: 'text_delta', text: 'It uses **cargo**' }, { type: 'text_delta', text: ' and `bun`.' })
    const reply = screen.getByTestId('assistant-message')
    expect(reply.textContent).toContain('It uses cargo and bun.')
    expect(reply.querySelector('strong')?.textContent).toBe('cargo')

    await emit(
      engine,
      { type: 'text_block_end' },
      { type: 'usage', turn: { input: 1200, output: 300, cache_read: 0, cache_write: 0 }, cumulative: { input: 1200, output: 300, cache_read: 0, cache_write: 0 }, estimated: false },
      { type: 'turn_finished' },
      engine.status(),
    )
    expect(screen.queryByRole('button', { name: /stop/i })).toBeNull()
    expect(screen.getByText('↑1.2k ↓300')).toBeTruthy()
    expect(within(screen.getByTestId('assistant-message')).getByRole('button', { name: 'Copy reply' })).toBeTruthy()
  })

  test('a slash command goes as a slash command, with suggestions while typing', async () => {
    const { engine } = await start()
    const box = screen.getByLabelText('Message Neuron')
    fireEvent.change(box, { target: { value: '/com' } })
    expect(screen.getByRole('option', { name: /\/compact/ })).toBeTruthy()
    fireEvent.change(box, { target: { value: '/compact' } })
    fireEvent.keyDown(box, { key: 'Enter' })
    fireEvent.keyDown(box, { key: 'Enter' })
    expect(sent(engine, 'slash')).toEqual({ kind: 'command', type: 'slash', command: 'compact', args: '' })
  })

  test('@ mentions search the workspace and attach the file', async () => {
    const { engine } = await start()
    const box = screen.getByLabelText('Message Neuron')
    fireEvent.change(box, { target: { value: 'look at @main', selectionStart: 13 } })
    const option = await screen.findByRole('option', { name: '@src/main.rs' })
    fireEvent.mouseDown(option)
    expect((box as HTMLTextAreaElement).value).toBe('look at @src/main.rs ')
    expect(within(screen.getByLabelText('Attached files')).getByText('@src/main.rs')).toBeTruthy()
    fireEvent.keyDown(box, { key: 'Enter' })
    expect(sent(engine, 'submit')).toEqual({
      kind: 'command',
      type: 'submit',
      text: 'look at @src/main.rs',
      attachments: [{ kind: 'file', path: 'src/main.rs' }],
    })
  })

  test('the mode menu switches permissions and plan mode', async () => {
    const { engine } = await start()
    fireEvent.click(screen.getByRole('button', { name: 'Permission mode' }))
    fireEvent.click(screen.getByRole('option', { name: /Read only/ }))
    expect(sent(engine, 'slash')).toEqual({ kind: 'command', type: 'slash', command: 'permissions', args: 'read-only' })
    fireEvent.click(screen.getByRole('button', { name: 'Permission mode' }))
    fireEvent.click(screen.getByRole('option', { name: /^Plan/ }))
    expect(sent(engine, 'slash')).toEqual({ kind: 'command', type: 'slash', command: 'plan', args: 'on' })
    // Menu commands are not the user's messages.
    expect(screen.queryByTestId('user-message')).toBeNull()
  })

  test('the model picker lists the catalog and switches', async () => {
    const { engine } = await start()
    fireEvent.click(screen.getByRole('button', { name: 'claude-sonnet-4-6' }))
    expect(sent(engine, 'list_models')).toBeTruthy()
    await emit(engine, {
      type: 'models',
      current: 'claude-sonnet-4-6',
      models: [
        { id: 'zx-fast', display_name: 'Zero-X Fast', available: true, unavailable_reason: null, context_window: 128000, tiers: ['free'] },
        { id: 'zx-max', display_name: 'Zero-X Max', available: false, unavailable_reason: 'upgrade_required', context_window: 200000, tiers: ['pro'] },
      ],
    })
    expect((screen.getByRole('option', { name: /Zero-X Max/ }) as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(screen.getByRole('option', { name: /Zero-X Fast/ }))
    expect(sent(engine, 'set_model')).toEqual({ kind: 'command', type: 'set_model', model: 'zx-fast' })
  })

  test('the engine stopping is shown, with a restart', async () => {
    const { engine, platform } = await start()
    await act(async () => {
      for (const listener of engine.closeListeners) listener({ code: 1, error: 'The Neuron engine stopped (exit code 1).' })
    })
    expect(screen.getByRole('alert').textContent).toContain('exit code 1')
    expect((screen.getByLabelText('Message Neuron') as HTMLTextAreaElement).disabled).toBe(true)
    fireEvent.click(screen.getByRole('button', { name: 'Restart it' }))
    expect(platform.restarts).toEqual(['session-1'])
  })
})

describe('Neuron Desktop: approvals, questions, tasks, sub-agents', () => {
  test('the permission dialog shows the command and sends the answer', async () => {
    const { engine } = await start()
    type('clean up')
    const input = '{"command":"rm -rf target"}'
    await emit(
      engine,
      { type: 'tool_call_started', id: 't1', name: 'bash', input, cut_off: false },
      {
        type: 'permission_requested',
        id: 4,
        request: { tool: 'bash', current_mode: 'workspace-write', required_mode: 'danger-full-access', reason: '`rm` is not a known read-only command', input, session_prefixes: ['rm'] },
      },
    )
    const dialog = screen.getByRole('dialog')
    expect(within(dialog).getByText('$ rm -rf target')).toBeTruthy()
    fireEvent.click(within(dialog).getByRole('button', { name: /allow once/i }))
    expect(engine.sent.at(-1)).toEqual({ kind: 'permission_reply', id: 4, decision: { kind: 'allow_once' } })
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  test('n denies from the keyboard, but not in the first moment', async () => {
    const { engine } = await start()
    type('x')
    const input = '{"command":"git push"}'
    await emit(engine, { type: 'tool_call_started', id: 't', name: 'bash', input, cut_off: false }, {
      type: 'permission_requested',
      id: 9,
      request: { tool: 'bash', current_mode: 'workspace-write', required_mode: 'danger-full-access', reason: null, input, session_prefixes: [] },
    })
    fireEvent.keyDown(window, { key: 'n' })
    expect(screen.getByRole('dialog')).toBeTruthy()
    await new Promise((resolve) => setTimeout(resolve, 350))
    fireEvent.keyDown(window, { key: 'n' })
    expect(engine.sent.at(-1)).toEqual({ kind: 'permission_reply', id: 9, decision: { kind: 'deny', note: null } })
  })

  test("the agent's question: pick an option, or type an answer", async () => {
    const { engine } = await start()
    type('plan it')
    await emit(engine, { type: 'question', id: 2, question: 'Which crate?', options: ['api', 'tools'] })
    const card = screen.getByRole('note', { name: 'Neuron asks' })
    fireEvent.click(within(card).getByRole('button', { name: /tools/ }))
    expect(sent(engine, 'answer')).toEqual({ kind: 'command', type: 'answer', id: 2, text: 'tools' })
    expect(within(screen.getByRole('note', { name: 'Neuron asks' })).queryByRole('button')).toBeNull()

    await emit(engine, { type: 'question', id: 3, question: 'Anything else?', options: [] })
    expect(screen.getByText("Answering Neuron's question")).toBeTruthy()
    type('/nope')
    // Typed while a question waits: the answer, slash and all.
    expect(sent(engine, 'answer')).toEqual({ kind: 'command', type: 'answer', id: 3, text: '/nope' })
  })

  test('the task list shows in the Tasks tab and the status bar', async () => {
    const { engine } = await start()
    await emit(engine, {
      type: 'todos',
      items: [
        { content: 'Run the tests', status: 'completed', active_form: 'Running the tests' },
        { content: 'Fix the build', status: 'in_progress', active_form: 'Fixing the build' },
      ],
    })
    fireEvent.click(screen.getByRole('button', { name: 'Tasks 1/2' }))
    const tasks = screen.getByRole('list', { name: 'Tasks' })
    expect(within(tasks).getByText('Fixing the build')).toBeTruthy()
    expect(within(tasks).getByText('Run the tests')).toBeTruthy()
  })

  test('a sub-agent shows on its Agent call, running then with its result', async () => {
    const { engine } = await start()
    type('audit')
    await emit(
      engine,
      { type: 'tool_call_started', id: 'a1', name: 'Agent', input: '{"description":"Audit MCP","prompt":"…","subagent_type":"Explore"}', cut_off: false },
      { type: 'subagent_started', id: 'a1', description: 'Audit MCP', subagent_type: 'Explore' },
    )
    const card = screen.getByTestId('tool-card')
    expect(within(card).getByText('Sub-agent (Explore)')).toBeTruthy()
    expect(within(card).getByText('sub-agent working')).toBeTruthy()
    await emit(
      engine,
      { type: 'tool_result', id: 'a1', name: 'Agent', output: '{"status":"completed","result":"Only stdio is implemented."}', is_error: false },
      { type: 'subagent_finished', id: 'a1', status: 'completed', is_error: false, result: 'Only stdio is implemented.', note: null },
    )
    fireEvent.click(within(card).getByRole('button', { expanded: false }))
    expect(within(card).getByText('Only stdio is implemented.')).toBeTruthy()
  })

  test('an edit shows its diff and diffstat', async () => {
    const { engine } = await start()
    type('rename')
    await emit(
      engine,
      { type: 'tool_call_started', id: 'e1', name: 'edit_file', input: '{"path":"/work/app/src/main.rs","old_string":"println!(\\"hi\\");","new_string":"println!(\\"hello\\");"}', cut_off: false },
      { type: 'tool_result', id: 'e1', name: 'edit_file', output: JSON.stringify({ filePath: '/work/app/src/main.rs', diff: '@@ -2,1 +2,1 @@\n-    println!("hi");\n+    println!("hello");\n' }), is_error: false },
    )
    const card = screen.getByTestId('tool-card')
    expect(within(card).getByText('src/main.rs')).toBeTruthy()
    expect([...card.querySelectorAll('tr[data-kind]')].map((row) => row.getAttribute('data-kind'))).toEqual(['remove', 'add'])
    expect(card.querySelector('.tool-diffstat')?.textContent).toBe('+1 −1')
  })
})

describe('Neuron Desktop: sessions, projects, trust, panes, settings', () => {
  const sessions = [
    { id: 'session-1', title: 'Fix the login bug', model: 'sonnet', message_count: 6, updated_at_ms: Date.now() - 60_000, current: true, branch: null },
    { id: 'session-0', title: 'Write the release notes', model: 'sonnet', message_count: 4, updated_at_ms: Date.now() - 3 * 86_400_000, current: false, branch: null },
  ]

  test('the session list: open one, search, start a new session', async () => {
    const { engine } = await start()
    await emit(engine, { type: 'sessions', current: 'session-1', sessions })
    const sidebar = screen.getByRole('navigation', { name: 'Neuron' })
    expect(within(sidebar).getByRole('button', { name: /Fix the login bug/ }).getAttribute('aria-current')).toBe('true')
    expect(screen.getByText('Fix the login bug', { selector: '.chat-header-title' })).toBeTruthy()
    fireEvent.click(within(sidebar).getByRole('button', { name: /Write the release notes/ }))
    expect(sent(engine, 'resume')).toEqual({ kind: 'command', type: 'resume', session: 'session-0' })

    fireEvent.change(within(sidebar).getByLabelText('Search sessions'), { target: { value: 'login' } })
    expect(within(sidebar).queryByRole('button', { name: /release notes/ })).toBeNull()

    fireEvent.click(within(sidebar).getByRole('button', { name: /New session/ }))
    expect(sent(engine, 'slash')).toEqual({ kind: 'command', type: 'slash', command: 'clear', args: '--confirm' })
  })

  test('a resumed session comes back from its transcript', async () => {
    const { engine } = await start()
    await emit(
      engine,
      {
        type: 'transcript',
        session_id: 'session-0',
        truncated: false,
        entries: [
          { kind: 'user', text: 'Write the release notes' },
          { kind: 'assistant', text: 'Done: see **NOTES.md**.' },
        ],
      },
      engine.status({ session_id: 'session-0' }),
    )
    expect(screen.getByTestId('user-message').textContent).toBe('Write the release notes')
    expect(screen.getByTestId('assistant-message').textContent).toContain('Done: see NOTES.md.')
  })

  test('switching the project from the Projects page', async () => {
    const { platform } = await start()
    fireEvent.click(screen.getByRole('button', { name: 'Projects' }))
    const list = await screen.findByRole('list', { name: 'Recent projects' })
    fireEvent.click(await within(list).findByRole('button', { name: /website/ }))
    await waitFor(() => expect(platform.projectOpens).toEqual([['/work/website', null]]))
    // The new engine starts with the default mode.
    expect(platform.launches.at(-1)).toEqual({ model: undefined, permissionMode: 'workspace-write' })
  })

  test('the Changes tab lists git changes and shows their diffs', async () => {
    await start()
    const pane = screen.getByRole('complementary', { name: 'Details' })
    const row = await within(pane).findByRole('button', { name: /src\/main\.rs/ })
    expect(within(pane).getByText('⎇ main · 2 changed')).toBeTruthy()
    fireEvent.click(row)
    await waitFor(() => expect(within(pane).getByLabelText('diff of src/main.rs')).toBeTruthy())
    expect(within(pane).getByText('println!("hello");', { exact: false })).toBeTruthy()
    // A new file shows as all additions.
    fireEvent.click(within(pane).getByRole('button', { name: /notes\.txt/ }))
    await waitFor(() => expect(within(pane).getByLabelText('diff of notes.txt').textContent).toContain('a new note'))
  })

  test('the Files tab opens folders and files', async () => {
    await start()
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    const tree = screen.getByRole('navigation', { name: 'Files' })
    fireEvent.click(await within(tree).findByRole('button', { name: /src/ }))
    fireEvent.click(await within(tree).findByRole('button', { name: /main\.rs/ }))
    await waitFor(() => expect(screen.getByTestId('file-view').textContent).toBe('fn main() {\n    println!("hi");\n}\n'))
  })

  test('signed out: the sign-in panel, then the device code', async () => {
    const { engine, platform } = await start({ sign_in_required: true, signed_in: false, account: null, session_id: '' })
    expect(screen.getByRole('button', { name: 'Sign in to Zero-X' })).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))
    expect(sent(engine, 'slash')).toEqual({ kind: 'command', type: 'slash', command: 'login', args: '' })
    await emit(engine, {
      type: 'sign_in_code',
      url: 'https://zero-x.live/device',
      url_complete: 'https://zero-x.live/device?code=WXYZ-1234',
      user_code: 'WXYZ-1234',
      expires_in_secs: 600,
      browser_opened: false,
    })
    expect(screen.getByLabelText('sign-in code').textContent).toBe('WXYZ-1234')
    fireEvent.click(screen.getByRole('button', { name: 'Open the sign-in page' }))
    expect(platform.opened).toEqual(['https://zero-x.live/device?code=WXYZ-1234'])
  })

  test('settings: the light theme and Ctrl+Enter to send', async () => {
    const { engine } = await start()
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }))
    fireEvent.click(screen.getByRole('button', { name: 'Appearance' }))
    fireEvent.change(screen.getByLabelText('Theme'), { target: { value: 'light' } })
    expect(document.documentElement.dataset.theme).toBe('light')
    fireEvent.click(screen.getByLabelText(/Enter sends/))
    fireEvent.click(screen.getByRole('button', { name: 'Close settings' }))
    const box = screen.getByLabelText('Message Neuron')
    fireEvent.change(box, { target: { value: 'two\nlines' } })
    fireEvent.keyDown(box, { key: 'Enter' })
    expect(sent(engine, 'submit')).toBeUndefined()
    fireEvent.keyDown(box, { key: 'Enter', ctrlKey: true })
    expect(sent(engine, 'submit')).toMatchObject({ text: 'two\nlines' })
  })

  test('the Extensions page: MCP servers with their status, plugins, skills and agents', async () => {
    const { engine } = await start()
    fireEvent.click(screen.getByRole('button', { name: 'Extensions' }))
    expect(sent(engine, 'list_extensions')).toEqual({ kind: 'command', type: 'list_extensions' })
    expect(screen.getByText('Asking the engine…')).toBeTruthy()
    await emit(engine, {
      type: 'extensions',
      mcp_servers: [
        { name: 'docs', scope: 'user', transport: 'stdio', status: 'ready', tools: ['lookup', 'index'], error: null },
        { name: 'ghost', scope: 'project', transport: 'stdio', status: 'failed', tools: [], error: 'spawn failed: not found' },
        { name: 'remote', scope: 'user', transport: 'ws', status: 'unsupported', tools: [], error: 'ws is not supported yet' },
      ],
      plugins: [{ id: 'fmt@builtin', name: 'fmt', version: '1.0.0', description: 'Formats code', kind: 'builtin', enabled: true }],
      skills: [
        { name: 'release-notes', description: 'Drafts release notes', source: 'Project', active: true },
        { name: 'release-notes', description: null, source: 'User', active: false },
      ],
      agents: [],
    })
    const servers = screen.getByRole('list', { name: 'MCP servers' })
    expect(screen.getByText('1/3 ready')).toBeTruthy()
    expect(within(servers).getByText(/user · stdio · 2 tools/)).toBeTruthy()
    fireEvent.click(within(servers).getByRole('button', { name: /ghost/ }))
    expect(within(servers).getByText('spawn failed: not found')).toBeTruthy()
    fireEvent.click(within(servers).getByRole('button', { name: /docs/ }))
    expect(within(servers).getByText('lookup, index')).toBeTruthy()
    expect(within(screen.getByRole('list', { name: 'Plugins' })).getByText('Formats code')).toBeTruthy()
    const skills = screen.getByRole('list', { name: 'Skills' })
    expect(within(skills).getAllByText('release-notes')).toHaveLength(2)
    expect(within(skills).getByText('Drafts release notes')).toBeTruthy()
    expect(screen.getByText('No agent definitions found.')).toBeTruthy()

    fireEvent.change(screen.getByLabelText('Filter extensions'), { target: { value: 'format' } })
    expect(within(screen.getByRole('list', { name: 'Plugins' })).getByText('fmt')).toBeTruthy()
    // No server and no skill is about formatting.
    expect(screen.getAllByText('None match.')).toHaveLength(2)
    fireEvent.change(screen.getByLabelText('Filter extensions'), { target: { value: 'lookup' } })
    expect(within(screen.getByRole('list', { name: 'MCP servers' })).queryByText('ghost')).toBeNull()
    expect(within(screen.getByRole('list', { name: 'MCP servers' })).getByText('docs')).toBeTruthy()
    fireEvent.change(screen.getByLabelText('Filter extensions'), { target: { value: '' } })

    const asked = engine.commands().filter((message) => message.kind === 'command' && message.type === 'list_extensions').length
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }))
    expect(engine.commands().filter((message) => message.kind === 'command' && message.type === 'list_extensions').length).toBe(asked + 1)
  })

  test('a server still starting is asked about again', async () => {
    const { engine } = await start()
    fireEvent.click(screen.getByRole('button', { name: 'Extensions' }))
    const count = () => engine.commands().filter((message) => message.kind === 'command' && message.type === 'list_extensions').length
    await emit(engine, {
      type: 'extensions',
      mcp_servers: [{ name: 'docs', scope: 'user', transport: 'stdio', status: 'starting', tools: [], error: null }],
      plugins: [],
      skills: [],
      agents: [],
    })
    const before = count()
    await waitFor(() => expect(count()).toBe(before + 1), { timeout: 3_500 })
  })

  test('an older engine without history: no session list, chatting still works', async () => {
    const { engine } = await start({}, [])
    expect(screen.getByText(/keeps no session history/)).toBeTruthy()
    expect(sent(engine, 'capabilities')).toBeUndefined()
    type('hello')
    expect(sent(engine, 'submit')).toMatchObject({ text: 'hello' })
  })
})
