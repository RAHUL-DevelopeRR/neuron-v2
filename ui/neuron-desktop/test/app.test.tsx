import { afterEach, describe, expect, test } from 'bun:test'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

import { App } from '../src/App'
import { FakeEngine, fakePlatform } from './fake-platform'

afterEach(cleanup)

async function start(statusOverrides: Record<string, unknown> = {}) {
  const engine = new FakeEngine(statusOverrides)
  const platform = fakePlatform(engine)
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
  fireEvent.change(box, { target: { value: text } })
  fireEvent.keyDown(box, { key: 'Enter' })
}

describe('Neuron Desktop', () => {
  test('a prompt goes to the engine and the reply streams in', async () => {
    const { engine } = await start()
    expect(screen.getByText('main', { exact: false })).toBeTruthy()
    type('explain the build')
    expect(engine.sent).toEqual([{ kind: 'command', type: 'submit', text: 'explain the build', attachments: [] }])
    expect(screen.getByTestId('user-message').textContent).toBe('explain the build')
    expect(screen.getByRole('button', { name: /stop/i })).toBeTruthy()

    await emit(engine, { type: 'text_delta', text: 'It uses **cargo**' })
    await emit(engine, { type: 'text_delta', text: ' and `bun`.' })
    const reply = screen.getByTestId('assistant-message')
    expect(reply.textContent).toBe('It uses cargo and bun.')
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
  })

  test('a slash command goes as a slash command, with suggestions while typing', async () => {
    const { engine } = await start()
    const box = screen.getByLabelText('Message Neuron')
    fireEvent.change(box, { target: { value: '/com' } })
    expect(screen.getByRole('option', { name: /\/compact/ })).toBeTruthy()
    fireEvent.change(box, { target: { value: '/compact' } })
    fireEvent.keyDown(box, { key: 'Enter' })
    fireEvent.keyDown(box, { key: 'Enter' })
    expect(engine.sent).toEqual([{ kind: 'command', type: 'slash', command: 'compact', args: '' }])
  })

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
        request: {
          tool: 'bash',
          current_mode: 'workspace-write',
          required_mode: 'danger-full-access',
          reason: '`rm` is not a known read-only command',
          input,
          session_prefixes: ['rm'],
        },
      },
    )
    const dialog = screen.getByRole('dialog')
    expect(within(dialog).getByText('$ rm -rf target')).toBeTruthy()
    expect(within(dialog).getByText(/not a known read-only command/)).toBeTruthy()
    expect(screen.getByText('waiting for you')).toBeTruthy()
    fireEvent.click(within(dialog).getByRole('button', { name: /allow once/i }))
    expect(engine.sent.at(-1)).toEqual({ kind: 'permission_reply', id: 4, decision: { kind: 'allow_once' } })
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(screen.getByText('running')).toBeTruthy()
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
    await waitFor(() => expect(screen.getByText('denied')).toBeTruthy())
  })

  test('deny with a note', async () => {
    const { engine } = await start()
    type('x')
    const input = '{"path":"src/main.rs","old_string":"hi","new_string":"hello"}'
    await emit(engine, { type: 'tool_call_started', id: 'e', name: 'edit_file', input, cut_off: false }, {
      type: 'permission_requested',
      id: 2,
      request: { tool: 'edit_file', current_mode: 'read-only', required_mode: 'workspace-write', reason: null, input, session_prefixes: [] },
    })
    const dialog = screen.getByRole('dialog')
    expect(within(dialog).getByLabelText('edit to src/main.rs')).toBeTruthy()
    fireEvent.click(within(dialog).getByRole('button', { name: /deny with note/i }))
    fireEvent.change(within(dialog).getByPlaceholderText(/what to do instead/), { target: { value: 'keep hi' } })
    fireEvent.click(within(dialog).getByRole('button', { name: 'Deny with note' }))
    expect(engine.sent.at(-1)).toEqual({ kind: 'permission_reply', id: 2, decision: { kind: 'deny', note: 'keep hi' } })
  })

  test('an edit shows its diff in the tool card and the file in Changes', async () => {
    const { engine } = await start()
    type('rename')
    await emit(
      engine,
      { type: 'tool_call_started', id: 'e1', name: 'edit_file', input: '{"path":"/work/app/src/main.rs","old_string":"println!(\\"hi\\");","new_string":"println!(\\"hello\\");"}', cut_off: false },
      {
        type: 'tool_result',
        id: 'e1',
        name: 'edit_file',
        output: JSON.stringify({ filePath: '/work/app/src/main.rs', diff: '@@ -2,1 +2,1 @@\n-    println!("hi");\n+    println!("hello");\n' }),
        is_error: false,
      },
    )
    const card = screen.getByTestId('tool-card')
    expect(within(card).getByText('src/main.rs')).toBeTruthy()
    const rows = card.querySelectorAll('tr[data-kind]')
    expect([...rows].map((row) => row.getAttribute('data-kind'))).toEqual(['remove', 'add'])
    expect(card.textContent).toContain('println!("hello");')
    const changes = screen.getByLabelText('Details')
    fireEvent.click(within(changes).getByRole('button', { name: 'src/main.rs' }))
    await waitFor(() => expect(screen.getByTestId('file-view').textContent).toContain('fn main()'))
  })

  test('the model picker lists the catalog and switches', async () => {
    const { engine } = await start()
    fireEvent.click(screen.getByRole('button', { name: 'claude-sonnet-4-6' }))
    expect(engine.sent.at(-1)).toEqual({ kind: 'command', type: 'list_models' })
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
    expect(engine.sent.at(-1)).toEqual({ kind: 'command', type: 'set_model', model: 'zx-fast' })
  })

  test('signed out: the sign-in panel, then the device code', async () => {
    const { engine, platform } = await start({ sign_in_required: true, signed_in: false, account: null, session_id: '' })
    expect(screen.getByText('Signed out')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))
    expect(engine.sent.at(-1)).toEqual({ kind: 'command', type: 'slash', command: 'login', args: '' })
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
    await emit(engine, { type: 'session_starting' })
    expect(screen.getByText('Starting your session…')).toBeTruthy()
    await emit(
      engine,
      { type: 'turn_finished' },
      engine.status({
        sign_in_required: false,
        signed_in: true,
        session_id: 'session-2',
        account: { email: 'new@example.com', plan: 'free', source: 'keychain', expires_at: null },
      }),
    )
    expect(screen.getByLabelText('Message Neuron')).toBeTruthy()
    expect(screen.getByText('new@example.com · free')).toBeTruthy()
  })

  test('the file tree opens folders and files', async () => {
    await start()
    const tree = screen.getByRole('navigation', { name: 'Files' })
    fireEvent.click(await within(tree).findByRole('button', { name: /src/ }))
    fireEvent.click(await within(tree).findByRole('button', { name: /main\.rs/ }))
    await waitFor(() => expect(screen.getByTestId('file-view').textContent).toBe('fn main() {\n    println!("hi");\n}\n'))
  })

  test("the agent's question, answered from the composer", async () => {
    const { engine } = await start()
    type('plan it')
    await emit(engine, { type: 'notice', message: 'Question: Which crate?\n  1. api\n  2. tools\nAnswer with 1-2 or in your own words.' })
    expect(screen.getByText("Answering Neuron's question")).toBeTruthy()
    type('/2')
    // Sent as typed, even with a slash: it is an answer, not a command.
    expect(engine.sent.at(-1)).toEqual({ kind: 'command', type: 'submit', text: '/2', attachments: [] })
  })

  test('the engine stopping is shown', async () => {
    const { engine } = await start()
    await act(async () => {
      for (const listener of (engine as unknown as { closeListeners: Set<(c: { code: number; error: string }) => void> }).closeListeners)
        listener({ code: 1, error: 'The Neuron engine stopped (exit code 1).' })
    })
    expect(screen.getByRole('alert').textContent).toContain('exit code 1')
    expect((screen.getByLabelText('Message Neuron') as HTMLTextAreaElement).disabled).toBe(true)
  })
})
