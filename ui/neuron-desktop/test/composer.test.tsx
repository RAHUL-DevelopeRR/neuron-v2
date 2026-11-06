import { afterEach, describe, expect, test } from 'bun:test'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import axe from 'axe-core'

import type { ClientMessage } from '@neuron/client'

import { App } from '../src/App'
import { Composer, nextMode } from '../src/components/Composer'
import { availableCommands, transcriptJson, transcriptMarkdown } from '../src/commands'
import { ALL_FEATURES, FakeEngine, fakePlatform, type FakePlatform } from './fake-platform'

afterEach(() => {
  cleanup()
  try {
    localStorage.clear()
  } catch {
    // No storage: nothing to clear.
  }
})

const MORE = ['reports', 'git_views', 'checkpoints', 'model_images']

async function start(
  statusOverrides: Record<string, unknown> = {},
  features: string[] = ALL_FEATURES,
  before: (platform: FakePlatform) => void = () => {},
) {
  const engine = new FakeEngine(statusOverrides, features)
  const platform = fakePlatform(engine)
  before(platform)
  render(<App platform={platform} />)
  await screen.findByLabelText('Message Neuron')
  return { engine, platform }
}

const emit = (engine: FakeEngine, ...events: Parameters<FakeEngine['emit']>[0][]) =>
  act(() => {
    for (const event of events) engine.emit(event)
  })

const box = () => screen.getByLabelText('Message Neuron') as HTMLTextAreaElement

const type = (text: string) => {
  fireEvent.change(box(), { target: { value: text, selectionStart: text.length } })
  fireEvent.keyDown(box(), { key: 'Enter' })
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const sent = (engine: FakeEngine, kind: string): Array<Extract<ClientMessage, { kind: 'command' }> & Record<string, any>> =>
  engine.commands().filter((message) => message.kind === 'command' && message.type === kind) as never

const finishTurn = (engine: FakeEngine) => emit(engine, { type: 'turn_finished' }, engine.status())

const pngFile = (name = 'shot.png') => new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47, 1, 2, 3])], name, { type: 'image/png' })

const paste = (...files: File[]) => fireEvent.paste(box(), { clipboardData: { files, items: [], types: ['Files'] } })

async function expectAccessible(root: Element) {
  const result = await axe.run(root, { rules: { 'color-contrast': { enabled: false } } })
  const serious = result.violations.filter((violation) => violation.impact === 'serious' || violation.impact === 'critical')
  expect(serious.map((violation) => `${violation.id}: ${violation.nodes.map((node) => node.target.join(' ')).join(', ')}`)).toEqual([])
}

describe('Composer: the slash registry', () => {
  test('the menu lists what the engine supports, and not terminal-only commands', async () => {
    await start()
    fireEvent.change(box(), { target: { value: '/' } })
    const names = screen.getAllByRole('option').map((option) => /^\/(\S+)/.exec(option.textContent ?? '')?.[1])
    for (const name of ['new', 'copy', 'export', 'help', 'model', 'plan', 'status', 'compact', 'cost', 'login']) expect(names).toContain(name)
    // This engine has no reports, git views or checkpoints.
    for (const name of ['usage', 'doctor', 'changes', 'diff', 'commit', 'rewind']) expect(names).not.toContain(name)
    for (const name of ['exit', 'bash', 'image', 'theme:toggle']) expect(names).not.toContain(name)
  })

  test('features add their commands', async () => {
    await start({}, [...ALL_FEATURES, ...MORE])
    fireEvent.change(box(), { target: { value: '/' } })
    const names = screen.getAllByRole('option').map((option) => /^\/(\S+)/.exec(option.textContent ?? '')?.[1])
    for (const name of ['usage', 'init', 'memory', 'doctor', 'changes', 'diff', 'commit', 'rewind']) expect(names).toContain(name)
  })

  test('an engine without the account or sessions features loses those commands', () => {
    const names = availableCommands(['capabilities']).map((command) => command.name)
    expect(names).not.toContain('login')
    expect(names).not.toContain('logout')
    expect(names).not.toContain('history')
    expect(names).toContain('status')
  })

  test('typing narrows the menu, and an alias finds its command', async () => {
    await start()
    fireEvent.change(box(), { target: { value: '/cl' } })
    expect(screen.getByRole('option', { name: /\/new/ })).toBeTruthy()
    fireEvent.change(box(), { target: { value: '/co' } })
    const names = screen.getAllByRole('option').map((option) => /^\/(\S+)/.exec(option.textContent ?? '')?.[1])
    expect(names).toEqual(expect.arrayContaining(['compact', 'context', 'copy', 'cost']))
  })

  test('engine commands go to the engine; desktop commands do not', async () => {
    const { engine } = await start()
    type('/status')
    expect(sent(engine, 'slash').at(-1)).toEqual({ kind: 'command', type: 'slash', command: 'status', args: '' })
    await finishTurn(engine)
    const before = sent(engine, 'slash').length
    type('/new')
    expect(sent(engine, 'slash').at(-1)).toEqual({ kind: 'command', type: 'slash', command: 'clear', args: '--confirm' })
    expect(sent(engine, 'slash').length).toBe(before + 1)
  })

  test('/plan turns plan mode on; /model opens the picker; /model name switches at once', async () => {
    const { engine } = await start()
    type('/plan')
    expect(sent(engine, 'slash').at(-1)).toEqual({ kind: 'command', type: 'slash', command: 'plan', args: 'on' })
    type('/model')
    expect(await screen.findByRole('listbox', { name: 'Models' })).toBeTruthy()
    type('/model sonnet')
    expect(sent(engine, 'set_model').at(-1)).toEqual({ kind: 'command', type: 'set_model', model: 'sonnet' })
  })

  test('/help lists the commands in the page', async () => {
    await start()
    type('/help')
    const help = await screen.findByRole('region', { name: 'Commands' })
    expect(within(help).getByText('/status')).toBeTruthy()
    expect(within(help).getByText(/Ctrl\+Shift\+M/)).toBeTruthy()
  })
})

describe('Composer: images', () => {
  test('a pasted image is staged, shown, and sent as an attachment', async () => {
    const { engine, platform } = await start({ supports_images: true })
    paste(pngFile())
    await waitFor(() => expect(platform.staged).toEqual([['shot.png', 7, 'image/png']]))
    const remove = await screen.findByRole('button', { name: 'Remove image shot.png' })
    expect(remove).toBeTruthy()
    type('what is this')
    expect(sent(engine, 'submit').at(-1)).toEqual({
      kind: 'command',
      type: 'submit',
      text: 'what is this',
      attachments: [{ kind: 'image', path: '/stage/1-shot.png', media_type: 'image/png' }],
    })
    expect(screen.queryByRole('button', { name: 'Remove image shot.png' })).toBeNull()
  })

  test('a dropped image file is attached, and can be removed', async () => {
    const { engine, platform } = await start({ supports_images: true })
    fireEvent.drop(document.querySelector('.composer-box')!, { dataTransfer: { files: [pngFile('drop.png')], types: ['Files'] } })
    await waitFor(() => expect(platform.staged.length).toBe(1))
    fireEvent.click(await screen.findByRole('button', { name: 'Remove image drop.png' }))
    expect(screen.queryByRole('button', { name: 'Remove image drop.png' })).toBeNull()
    type('hello')
    expect(sent(engine, 'submit').at(-1)?.attachments).toEqual([])
  })

  test('a model that takes no images gets a one-line hint, not an error', async () => {
    const { engine, platform } = await start({ supports_images: false })
    paste(pngFile())
    expect((await screen.findByRole('status')).textContent).toMatch(/can.t read images/)
    expect(screen.queryByRole('alert')).toBeNull()
    expect(platform.staged).toEqual([])
    type('hi')
    expect(sent(engine, 'submit').at(-1)?.attachments).toEqual([])
  })

  test('a model that does not say gets the same hint', async () => {
    const { platform } = await start({})
    paste(pngFile())
    expect((await screen.findByRole('status')).textContent).toMatch(/can.t read images/)
    expect(platform.staged).toEqual([])
  })

  test('files that are not allowed images are refused with a hint', async () => {
    const { platform } = await start({ supports_images: true })
    paste(new File(['<svg/>'], 'a.svg', { type: 'image/svg+xml' }))
    expect((await screen.findByRole('status')).textContent).toMatch(/PNG, JPEG, GIF or WebP/)
    expect(platform.staged).toEqual([])
  })

  test('a staging failure shows its reason and attaches nothing', async () => {
    const { platform } = await start({ supports_images: true }, ALL_FEATURES, (fake) => {
      fake.stageAttachment = () => Promise.reject(new Error('Images can be at most 10 MB.'))
    })
    paste(pngFile())
    expect((await screen.findByRole('status')).textContent).toContain('at most 10 MB')
    expect(screen.queryByRole('button', { name: /Remove image/ })).toBeNull()
  })
})

describe('Composer: @folder', () => {
  test('the @ search lists folders, and a chosen folder is sent as a folder', async () => {
    const { engine } = await start()
    fireEvent.change(box(), { target: { value: 'look in @sr', selectionStart: 11 } })
    const folder = await screen.findByRole('option', { name: '@src/' })
    fireEvent.mouseDown(folder)
    expect(box().value).toBe('look in @src/ ')
    expect(within(screen.getByLabelText('Attached files')).getByText('@src/')).toBeTruthy()
    fireEvent.keyDown(box(), { key: 'Enter' })
    expect(sent(engine, 'submit').at(-1)?.attachments).toEqual([{ kind: 'folder', path: 'src' }])
  })
})

describe('Composer: prompt history', () => {
  test('Up and Down recall the prompts of this project and bring the draft back', async () => {
    const { engine } = await start()
    type('one')
    await finishTurn(engine)
    type('two')
    await finishTurn(engine)
    fireEvent.change(box(), { target: { value: 'draft', selectionStart: 0 } })
    box().setSelectionRange(0, 0)
    fireEvent.keyDown(box(), { key: 'ArrowUp' })
    expect(box().value).toBe('two')
    fireEvent.keyDown(box(), { key: 'ArrowUp' })
    expect(box().value).toBe('one')
    fireEvent.keyDown(box(), { key: 'ArrowUp' })
    expect(box().value).toBe('one')
    fireEvent.keyDown(box(), { key: 'ArrowDown' })
    expect(box().value).toBe('two')
    fireEvent.keyDown(box(), { key: 'ArrowDown' })
    expect(box().value).toBe('draft')
  })

  test('Up in the middle of a draft moves the caret instead', async () => {
    const { engine } = await start()
    type('one')
    await finishTurn(engine)
    fireEvent.change(box(), { target: { value: 'draft', selectionStart: 3 } })
    box().setSelectionRange(3, 3)
    fireEvent.keyDown(box(), { key: 'ArrowUp' })
    expect(box().value).toBe('draft')
  })

  test('the last 100 are kept per project in the store', async () => {
    const earlier = Array.from({ length: 100 }, (_, index) => `old ${index}`)
    const { engine, platform } = await start({}, ALL_FEATURES, (fake) => fake.saved.set('composer.history./work/app', earlier))
    await new Promise((resolve) => setTimeout(resolve, 20))
    type('newest')
    await finishTurn(engine)
    const kept = platform.saved.get('composer.history./work/app') as string[]
    expect(kept.length).toBe(100)
    expect(kept.at(0)).toBe('old 1')
    expect(kept.at(-1)).toBe('newest')
    expect(platform.saved.get('composer.history./work/website')).toBeUndefined()
  })
})

describe('Composer: the queue', () => {
  test('while a turn runs, Enter queues; the queue sends when it is over', async () => {
    const { engine } = await start()
    type('first')
    expect(sent(engine, 'submit').length).toBe(1)
    type('second')
    type('third')
    expect(sent(engine, 'submit').length).toBe(1)
    const queue = screen.getByRole('list', { name: 'Queued messages' })
    expect(within(queue).getAllByRole('listitem').map((item) => item.textContent)).toEqual([
      expect.stringContaining('second'),
      expect.stringContaining('third'),
    ])
    expect(screen.getByRole('button', { name: /stop/i })).toBeTruthy()
    await finishTurn(engine)
    await waitFor(() => expect(sent(engine, 'submit').map((message) => message.text)).toEqual(['first', 'second']))
    // The next turn is running: the third waits.
    expect(within(screen.getByRole('list', { name: 'Queued messages' })).getAllByRole('listitem').length).toBe(1)
    await finishTurn(engine)
    await waitFor(() => expect(sent(engine, 'submit').map((message) => message.text)).toEqual(['first', 'second', 'third']))
    expect(screen.queryByRole('list', { name: 'Queued messages' })).toBeNull()
  })

  test('queued items can be edited and removed', async () => {
    const { engine } = await start()
    type('first')
    type('second')
    type('third')
    fireEvent.click(screen.getByRole('button', { name: 'Remove queued message: third' }))
    fireEvent.click(screen.getByRole('button', { name: 'Edit queued message: second' }))
    const edit = screen.getByLabelText('Edit queued message') as HTMLTextAreaElement
    fireEvent.change(edit, { target: { value: 'second, better' } })
    fireEvent.keyDown(edit, { key: 'Enter' })
    expect(within(screen.getByRole('list', { name: 'Queued messages' })).getByText('second, better')).toBeTruthy()
    await finishTurn(engine)
    await waitFor(() => expect(sent(engine, 'submit').map((message) => message.text)).toEqual(['first', 'second, better']))
  })

  test('Stop stays available and a copy command does not wait', async () => {
    const { engine } = await start()
    type('first')
    fireEvent.click(screen.getByRole('button', { name: /stop/i }))
    expect(sent(engine, 'interrupt').length).toBe(1)
  })
})

const cycle = (target: Element) => fireEvent.keyDown(target, { key: 'M', ctrlKey: true, shiftKey: true })

describe('Composer: permission mode', () => {
  test('Shift+Tab is left to the browser', async () => {
    const { engine } = await start()
    const before = sent(engine, 'slash').length
    const allowed = fireEvent.keyDown(box(), { key: 'Tab', shiftKey: true })
    expect(allowed).toBe(true)
    expect(sent(engine, 'slash').length).toBe(before)
  })

  test('Ctrl+Shift+M cycles edit, plan and read only, and never full access', async () => {
    const { engine } = await start()
    const last = () => sent(engine, 'slash').at(-1)
    cycle(box())
    expect(last()).toEqual({ kind: 'command', type: 'slash', command: 'plan', args: 'on' })
    await emit(engine, engine.status({ permission_mode: 'workspace-write', plan_mode: true }))
    cycle(box())
    expect(sent(engine, 'slash').slice(-2)).toEqual([
      { kind: 'command', type: 'slash', command: 'plan', args: 'off' },
      { kind: 'command', type: 'slash', command: 'permissions', args: 'read-only' },
    ])
    await emit(engine, engine.status({ permission_mode: 'read-only', plan_mode: false }))
    cycle(box())
    expect(last()).toEqual({ kind: 'command', type: 'slash', command: 'permissions', args: 'workspace-write' })
  })

  test('the cycle leaves full access for read only, and nothing leads into it', () => {
    expect(nextMode('danger-full-access', false)).toEqual({ mode: 'read-only', plan: false })
    for (const mode of ['read-only', 'workspace-write']) {
      let at = { mode, plan: false }
      for (let i = 0; i < 8; i++) {
        at = nextMode(at.mode, at.plan)
        expect(at.mode).not.toBe('danger-full-access')
      }
    }
  })

  test('Ctrl+Shift+M does nothing while a turn runs, and the hints name it', async () => {
    const { engine } = await start()
    type('go')
    const before = sent(engine, 'slash').length
    cycle(box())
    expect(sent(engine, 'slash').length).toBe(before)
    expect(screen.getByRole('button', { name: 'Permission mode' }).getAttribute('title')).toContain('Ctrl+Shift+M')
  })
})

describe('Composer: keys and menus', () => {
  test('Enter while an IME composes a word does not send', async () => {
    const { engine } = await start()
    fireEvent.change(box(), { target: { value: 'konnichi', selectionStart: 8 } })
    fireEvent.keyDown(box(), { key: 'Enter', isComposing: true })
    fireEvent.keyDown(box(), { key: 'Enter', keyCode: 229 })
    expect(sent(engine, 'submit').length).toBe(0)
    fireEvent.keyDown(box(), { key: 'Enter' })
    expect(sent(engine, 'submit').length).toBe(1)
  })

  test('Esc closes the open menu first, and stops the turn only when none is open', async () => {
    const { engine } = await start()
    type('go')
    fireEvent.change(box(), { target: { value: '/st', selectionStart: 3 } })
    expect(screen.getByRole('listbox', { name: 'Commands' })).toBeTruthy()
    fireEvent.keyDown(box(), { key: 'Escape' })
    expect(screen.queryByRole('listbox', { name: 'Commands' })).toBeNull()
    expect(sent(engine, 'interrupt').length).toBe(0)
    fireEvent.keyDown(box(), { key: 'Escape' })
    expect(sent(engine, 'interrupt').length).toBe(1)
  })

  test('the box is a combobox that names its list and the active option', async () => {
    await start()
    fireEvent.change(box(), { target: { value: '/', selectionStart: 1 } })
    const field = screen.getByRole('combobox', { name: 'Message Neuron' })
    expect(field.getAttribute('aria-expanded')).toBe('true')
    const list = screen.getByRole('listbox', { name: 'Commands' })
    expect(field.getAttribute('aria-controls')).toBe(list.id)
    const options = within(list).getAllByRole('option')
    expect(field.getAttribute('aria-activedescendant')).toBe(options[0].id)
    fireEvent.keyDown(box(), { key: 'ArrowDown' })
    expect(field.getAttribute('aria-activedescendant')).toBe(options[1].id)
    expect(new Set(options.map((option) => option.id)).size).toBe(options.length)
    fireEvent.change(box(), { target: { value: 'plain', selectionStart: 5 } })
    expect(field.getAttribute('aria-expanded')).toBe('false')
    expect(field.getAttribute('aria-activedescendant')).toBeNull()
  })

  test('while a question is open, images and @ attach nothing and a hint says why', async () => {
    const { engine, platform } = await start({ supports_images: true })
    type('go')
    await emit(engine, { type: 'question', id: 2, question: 'Which?', options: [] })
    paste(pngFile())
    expect(await screen.findByText(/Answer the question first/)).toBeTruthy()
    expect(platform.staged).toEqual([])
    expect(screen.queryByRole('button', { name: 'Remove image shot.png' })).toBeNull()
    fireEvent.change(box(), { target: { value: 'see @src', selectionStart: 8 } })
    expect(screen.getByText(/Files cannot go with an answer/)).toBeTruthy()
    expect(screen.queryByRole('listbox', { name: 'Files' })).toBeNull()
  })

  test('attachments staged before a question stay for the next message', async () => {
    const { engine } = await start({ supports_images: true })
    type('go')
    paste(pngFile())
    await screen.findByRole('button', { name: 'Remove image shot.png' })
    await emit(engine, { type: 'question', id: 2, question: 'Which?', options: [] })
    type('the answer')
    expect(sent(engine, 'answer').at(-1)).toEqual({ kind: 'command', type: 'answer', id: 2, text: 'the answer' })
    expect(screen.getByRole('button', { name: 'Remove image shot.png' })).toBeTruthy()
  })
})

describe('Composer: the queue pauses', () => {
  test('Stop pauses the queue; Resume sends it', async () => {
    const { engine } = await start()
    type('first')
    type('second')
    fireEvent.click(screen.getByRole('button', { name: /stop/i }))
    await finishTurn(engine)
    expect(await screen.findByText(/Queue paused/)).toBeTruthy()
    await new Promise((resolve) => setTimeout(resolve, 50))
    expect(sent(engine, 'submit').map((message) => message.text)).toEqual(['first'])
    fireEvent.click(screen.getByRole('button', { name: 'Resume' }))
    await waitFor(() => expect(sent(engine, 'submit').map((message) => message.text)).toEqual(['first', 'second']))
    expect(screen.queryByText(/Queue paused/)).toBeNull()
  })

  test('Esc pauses it too', async () => {
    const { engine } = await start()
    type('first')
    type('second')
    fireEvent.keyDown(box(), { key: 'Escape' })
    await finishTurn(engine)
    expect(await screen.findByText(/Queue paused/)).toBeTruthy()
    expect(sent(engine, 'submit').length).toBe(1)
  })

  function renderBare(props: Partial<React.ComponentProps<typeof Composer>> = {}) {
    const sentTexts: string[] = []
    const base = {
      busy: false,
      disabled: false,
      questionWaiting: false,
      enterSends: true,
      searchFiles: async () => [],
      onSend: (text: string) => sentTexts.push(text),
      onInterrupt: () => {},
      sessionId: 'a',
    }
    const view = render(<Composer {...base} {...props} />)
    const again = (next: Partial<React.ComponentProps<typeof Composer>>) => view.rerender(<Composer {...base} {...props} {...next} />)
    const field = () => screen.getByLabelText('Message Neuron') as HTMLTextAreaElement
    const enter = (value: string) => {
      fireEvent.change(field(), { target: { value, selectionStart: value.length } })
      fireEvent.keyDown(field(), { key: 'Enter' })
    }
    return { sentTexts, again, enter }
  }

  test('a failed turn holds the queue', async () => {
    const { sentTexts, again, enter } = renderBare({ busy: true })
    enter('queued')
    again({ busy: true, turnFailed: true })
    again({ busy: false, turnFailed: true })
    expect(await screen.findByText(/Queue paused/)).toBeTruthy()
    expect(sentTexts).toEqual([])
    fireEvent.click(screen.getByRole('button', { name: 'Resume' }))
    await waitFor(() => expect(sentTexts).toEqual(['queued']))
  })

  test('a new session id clears the queue', async () => {
    const { sentTexts, again, enter } = renderBare({ busy: true })
    enter('queued')
    expect(screen.getByRole('list', { name: 'Queued messages' })).toBeTruthy()
    again({ busy: true, sessionId: 'b' })
    await waitFor(() => expect(screen.queryByRole('list', { name: 'Queued messages' })).toBeNull())
    again({ busy: false, sessionId: 'b' })
    await new Promise((resolve) => setTimeout(resolve, 30))
    expect(sentTexts).toEqual([])
  })
})

describe('Composer: reasoning effort', () => {
  test('a new level is a launch option, and applies by restarting once the turn is over', async () => {
    const { engine, platform } = await start()
    type('think')
    fireEvent.change(screen.getByLabelText('Reasoning effort'), { target: { value: 'high' } })
    expect(platform.launches.at(-1)?.reasoningEffort).toBe('high')
    expect(platform.restarts).toEqual([])
    expect(screen.getByText(/after this turn/i)).toBeTruthy()
    await finishTurn(engine)
    await waitFor(() => expect(platform.restarts).toEqual(['session-1']))
  })

  test('when idle it restarts at once; the default clears the option', async () => {
    const { platform } = await start()
    fireEvent.change(screen.getByLabelText('Reasoning effort'), { target: { value: 'low' } })
    // No prompt yet: a fresh chat, nothing to resume.
    await waitFor(() => expect(platform.restarts).toEqual([null]))
    fireEvent.change(screen.getByLabelText('Reasoning effort'), { target: { value: '' } })
    expect(platform.launches.at(-1)?.reasoningEffort).toBeUndefined()
  })
})

describe('Approvals', () => {
  const input = '{"command":"git push"}'
  async function ask(engine: FakeEngine, session_prefixes: string[] = []) {
    type('x')
    await emit(
      engine,
      { type: 'tool_call_started', id: 't', name: 'bash', input, cut_off: false },
      { type: 'permission_requested', id: 7, request: { tool: 'bash', current_mode: 'workspace-write', required_mode: 'danger-full-access', reason: null, input, session_prefixes } },
    )
    await new Promise((resolve) => setTimeout(resolve, 750))
  }
  const reply = (engine: FakeEngine) => engine.sent.at(-1)

  test('focus moves into the dialog, and Enter allows once', async () => {
    const { engine } = await start()
    await ask(engine)
    const dialog = screen.getByRole('alertdialog')
    expect(dialog.contains(document.activeElement)).toBe(true)
    fireEvent.keyDown(document.activeElement ?? window, { key: 'Enter' })
    expect(reply(engine)).toEqual({ kind: 'permission_reply', id: 7, decision: { kind: 'allow_once' } })
  })

  test('Esc denies', async () => {
    const { engine } = await start()
    await ask(engine)
    fireEvent.keyDown(document.activeElement ?? window, { key: 'Escape' })
    expect(reply(engine)).toEqual({ kind: 'permission_reply', id: 7, decision: { kind: 'deny', note: null } })
  })

  test('keys meant for something else never answer', async () => {
    const { engine } = await start()
    await ask(engine, ['git'])
    const target = document.activeElement ?? window
    const before = engine.sent.length
    for (const init of [
      { key: 'Enter', ctrlKey: true },
      { key: 'Enter', metaKey: true },
      { key: 'y', altKey: true },
      { key: 'Enter', repeat: true },
      { key: 'Escape', repeat: true },
      { key: 'n', ctrlKey: true },
    ]) {
      fireEvent.keyDown(target, init)
    }
    expect(engine.sent.length).toBe(before)
    expect(screen.getByRole('alertdialog')).toBeTruthy()
  })

  test('bare letters work only while focus is inside the dialog', async () => {
    const { engine } = await start()
    await ask(engine)
    const before = engine.sent.length
    fireEvent.keyDown(document.body, { key: 'y' })
    fireEvent.keyDown(window, { key: 'n' })
    expect(engine.sent.length).toBe(before)
    fireEvent.keyDown(document.activeElement ?? window, { key: 'y' })
    expect(reply(engine)).toEqual({ kind: 'permission_reply', id: 7, decision: { kind: 'allow_once' } })
  })

  test('refocusing the dialog restarts the guard', async () => {
    const { engine } = await start()
    await ask(engine)
    const dialog = screen.getByRole('alertdialog')
    const button = within(dialog).getAllByRole('button')[0]
    act(() => button.focus())
    fireEvent.keyDown(dialog, { key: 'Escape' })
    expect(screen.getByRole('alertdialog')).toBeTruthy()
    await new Promise((resolve) => setTimeout(resolve, 750))
    fireEvent.keyDown(dialog, { key: 'Escape' })
    expect(reply(engine)).toEqual({ kind: 'permission_reply', id: 7, decision: { kind: 'deny', note: null } })
  })

  test('a composer that holds text keeps focus; Tab moves into the dialog', async () => {
    const { engine } = await start()
    type('first')
    box().focus()
    fireEvent.change(box(), { target: { value: 'half a sentence', selectionStart: 15 } })
    const input = '{"command":"git push"}'
    await emit(
      engine,
      { type: 'tool_call_started', id: 't', name: 'bash', input, cut_off: false },
      { type: 'permission_requested', id: 7, request: { tool: 'bash', current_mode: 'workspace-write', required_mode: 'danger-full-access', reason: null, input, session_prefixes: [] } },
    )
    await new Promise((resolve) => setTimeout(resolve, 750))
    const dialog = screen.getByRole('alertdialog')
    expect(document.activeElement).toBe(box())
    // Keys typed into the composer never answer.
    const before = engine.sent.length
    fireEvent.keyDown(box(), { key: 'Enter' })
    fireEvent.keyDown(box(), { key: 'y' })
    expect(engine.sent.filter((m) => m.kind === 'permission_reply')).toEqual([])
    expect(engine.sent.length).toBe(before)
    fireEvent.keyDown(box(), { key: 'Tab' })
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true))
  })

  test('focus that falls to the page goes back into the dialog', async () => {
    const { engine } = await start()
    await ask(engine)
    const dialog = screen.getByRole('alertdialog')
    act(() => within(dialog).getAllByRole('button')[0].focus())
    act(() => (document.activeElement as HTMLElement | null)?.blur())
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true))
  })

  test('the buttons show their keys', async () => {
    const { engine } = await start()
    await ask(engine)
    const dialog = screen.getByRole('alertdialog')
    expect(within(dialog).getByRole('button', { name: /allow once/i }).textContent).toContain('Enter')
    expect(within(dialog).getByRole('button', { name: /^deny esc/i }).textContent).toContain('Esc')
  })

  test('deny with a note sends the note, and Esc in the note goes back instead of denying', async () => {
    const { engine } = await start()
    await ask(engine)
    const dialog = screen.getByRole('alertdialog')
    fireEvent.click(within(dialog).getByRole('button', { name: /deny with a note/i }))
    const note = within(dialog).getByPlaceholderText('Tell Neuron what to do instead')
    fireEvent.keyDown(note, { key: 'Escape' })
    expect(screen.getByRole('alertdialog')).toBeTruthy()
    expect(within(dialog).queryByPlaceholderText('Tell Neuron what to do instead')).toBeNull()
    fireEvent.click(within(dialog).getByRole('button', { name: /deny with a note/i }))
    const again = within(dialog).getByPlaceholderText('Tell Neuron what to do instead')
    fireEvent.change(again, { target: { value: 'use a branch instead' } })
    fireEvent.keyDown(again, { key: 'Enter' })
    expect(reply(engine)).toEqual({ kind: 'permission_reply', id: 7, decision: { kind: 'deny', note: 'use a branch instead' } })
  })

  test('Tab stays inside the dialog', async () => {
    const { engine } = await start()
    await ask(engine, ['git'])
    const dialog = screen.getByRole('alertdialog')
    const buttons = within(dialog).getAllByRole('button')
    buttons.at(-1)!.focus()
    fireEvent.keyDown(buttons.at(-1)!, { key: 'Tab' })
    expect(document.activeElement).toBe(buttons[0])
    fireEvent.keyDown(buttons[0], { key: 'Tab', shiftKey: true })
    expect(document.activeElement).toBe(buttons.at(-1)!)
  })

  test('the dialog and the composer pass axe', async () => {
    const { engine } = await start({ supports_images: true })
    paste(pngFile())
    await screen.findByRole('button', { name: 'Remove image shot.png' })
    type('/help')
    type('first')
    type('queued')
    await expectAccessible(document.querySelector('.composer')!)
    await ask(engine, ['git'])
    await expectAccessible(screen.getByRole('alertdialog'))
  }, 30_000)
})

describe('/copy and /export', () => {
  async function chat() {
    const started = await start()
    type('explain the build')
    await emit(started.engine, { type: 'text_delta', text: 'It uses **cargo**.' }, { type: 'text_block_end' })
    await finishTurn(started.engine)
    return started
  }

  test('/copy puts the chat on the clipboard as Markdown', async () => {
    const copied: string[] = []
    Object.defineProperty(navigator, 'clipboard', { value: { writeText: async (text: string) => void copied.push(text) }, configurable: true })
    await chat()
    type('/copy')
    await waitFor(() => expect(copied.length).toBe(1))
    expect(copied[0]).toContain('## You')
    expect(copied[0]).toContain('explain the build')
    expect(copied[0]).toContain('## Neuron')
    expect(copied[0]).toContain('It uses **cargo**.')
    expect((await screen.findByRole('status')).textContent).toMatch(/copied/i)
  })

  test('/export saves Markdown through the save dialog; the app gives no path', async () => {
    const { platform } = await chat()
    type('/export')
    await waitFor(() => expect(platform.exports.length).toBe(1))
    expect(platform.exports[0].name).toMatch(/\.md$/)
    expect(platform.exports[0].content).toContain('explain the build')
    expect((await screen.findByRole('status')).textContent).toContain('/saved/chat.md')
  })

  test('/export json saves the raw messages', async () => {
    const { platform } = await chat()
    type('/export json')
    await waitFor(() => expect(platform.exports.length).toBe(1))
    expect(platform.exports[0].name).toMatch(/\.json$/)
    const parsed = JSON.parse(platform.exports[0].content)
    expect(parsed.items.map((item: { kind: string }) => item.kind)).toEqual(['user', 'assistant'])
  })

  test('a cancelled save says nothing was saved', async () => {
    const { platform } = await chat()
    platform.exportPath = null
    type('/export')
    await waitFor(() => expect(platform.exports.length).toBe(1))
    expect((await screen.findByRole('status')).textContent).toMatch(/not saved/i)
  })

  test('/copy and /export run while a turn is running', async () => {
    const { platform } = await start()
    type('go')
    type('/export')
    await waitFor(() => expect(platform.exports.length).toBe(1))
  })

  test('the Markdown handles tools, notices, errors and fences inside output', () => {
    const markdown = transcriptMarkdown(
      [
        { kind: 'user', key: 'u', text: 'run it', attachments: [{ kind: 'file', path: 'a.ts' }, { kind: 'image', path: '/s/x.png', media_type: 'image/png' }] },
        { kind: 'tool', key: 't', id: 't', name: 'bash', input: '{"command":"echo ```"}', state: 'done', view: { kind: 'terminal', target: 'echo' }, output: 'out ``` text', permissionId: null },
        { kind: 'notice', key: 'n', text: 'a notice' },
        { kind: 'error', key: 'e', text: 'it broke' },
      ] as never,
      'Fix the build',
    )
    expect(markdown.startsWith('# Fix the build')).toBe(true)
    expect(markdown).toContain('a.ts')
    expect(markdown).toContain('bash')
    expect(markdown).toContain('````')
    expect(markdown).toContain('it broke')
    expect(JSON.parse(transcriptJson([] as never)).items).toEqual([])
  })
})
