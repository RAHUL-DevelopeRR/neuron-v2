/**
 * L2, review and git: the Changes pane's two scopes, reverting one file,
 * the commit card, the restore-point picker and "open in editor". States are
 * built through `reduceSession`; the controller records what it is asked.
 */
import { afterEach, describe, expect, test } from 'bun:test'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import axe from 'axe-core'

import {
  SessionController,
  initialSession,
  reduceSession,
  type ClientMessage,
  type EngineTransport,
  type RewindMode,
  type SessionAction,
  type SessionState,
  type UiEvent,
} from '@neuron/client'

import { SidePane } from '../src/components/SidePane'
import { FakeEngine, fakePlatform, type FakePlatform } from './fake-platform'

afterEach(cleanup)

async function expectAccessible(root: Element = document.body) {
  const result = await axe.run(root, { rules: { 'color-contrast': { enabled: false } } })
  const serious = result.violations.filter((violation) => violation.impact === 'serious' || violation.impact === 'critical')
  expect(serious.map((violation) => `${violation.id}: ${violation.nodes.map((node) => node.target.join(' ')).join(', ')}`)).toEqual([])
}

const FEATURES = ['capabilities', 'git_views', 'checkpoints']

const ready = (features = FEATURES): UiEvent => ({ type: 'ready', protocol: 2, version: '1.0.0', features })
const changes = (...files: Array<[string, string, number, number]>): UiEvent => ({
  type: 'changes',
  files: files.map(([path, status, added, removed]) => ({ path, status, added, removed })),
})
const MAIN_PATCH = '@@ -1,3 +1,3 @@\n fn main() {\n-    println!("hi");\n+    println!("from the agent");\n }\n'
const diffOf = (path: string, patch = MAIN_PATCH): UiEvent => ({
  type: 'diff',
  title: 'Changes against HEAD on main',
  truncated: false,
  files: [{ path, status: 'modified', added: 1, removed: 1, patch, binary: false }],
})

function build(...events: SessionAction[]): SessionState {
  return events.reduce<SessionState>((state, event) => reduceSession(state, event), initialSession())
}
const busy = (state: SessionState) => reduceSession(state, { type: 'local/submitted', text: 'work' })

/** A controller over a transport that records what the pane sends. */
function recorder() {
  const sent: ClientMessage[] = []
  const transport: EngineTransport = {
    echoesActions: false,
    send: (message: ClientMessage) => void sent.push(message),
    subscribe: () => () => {},
    onClose: () => () => {},
    start: async () => {},
    close: async () => {},
  } as unknown as EngineTransport
  const controller = new SessionController(transport)
  const commands = () => sent.filter((message) => message.kind === 'command') as Array<Record<string, unknown>>
  return { controller, sent, commands }
}

function mount(state: SessionState, platform: FakePlatform = fakePlatform(new FakeEngine()), controller: SessionController | null = recorder().controller) {
  const view = (next: SessionState) => (
    <SidePane
      platform={platform}
      tab="changes"
      onTab={() => {}}
      refreshKey={0}
      changed={new Set()}
      tool={null}
      todos={[]}
      subagents={[]}
      terminalOpened={false}
      state={next}
      controller={controller}
    />
  )
  const rendered = render(view(state))
  return { platform, rerender: (next: SessionState) => rendered.rerender(view(next)) }
}

const pane = () => screen.getByRole('complementary', { name: 'Details' })
const scope = (name: 'This chat' | 'This folder') => within(pane()).getByRole('button', { name })

describe('the two scopes of the Changes pane', () => {
  test('defaults to this chat when the engine has git_views and changes exist', async () => {
    mount(build(ready(), changes(['src/main.rs', 'modified', 3, 1])))
    expect(scope('This chat').getAttribute('aria-pressed')).toBe('true')
    expect(scope('This folder').getAttribute('aria-pressed')).toBe('false')
    const row = within(pane()).getByRole('button', { name: /src\/main\.rs/ })
    expect(row.textContent).toContain('+3')
    expect(row.textContent).toContain('−1')
  })

  test('defaults to this folder without git_views, or without changes', async () => {
    mount(build(ready(['capabilities']), changes(['src/main.rs', 'modified', 3, 1])))
    // Without git_views there is only the folder, and no toggle.
    expect(within(pane()).queryByRole('group', { name: 'Show changes from' })).toBeNull()
    expect(await within(pane()).findByText('⎇ main · 2 changed')).toBeTruthy()
    cleanup()
    mount(build(ready(), changes()))
    expect(scope('This folder').getAttribute('aria-pressed')).toBe('true')
  })

  test('the toggle switches between the engine list and the folder list', async () => {
    mount(build(ready(), changes(['only-in-chat.rs', 'created', 5, 0])))
    expect(within(pane()).getByRole('button', { name: /only-in-chat\.rs/ })).toBeTruthy()
    fireEvent.click(scope('This folder'))
    expect(await within(pane()).findByRole('button', { name: /notes\.txt/ })).toBeTruthy()
    expect(within(pane()).queryByRole('button', { name: /only-in-chat\.rs/ })).toBeNull()
    fireEvent.click(scope('This chat'))
    expect(within(pane()).getByRole('button', { name: /only-in-chat\.rs/ })).toBeTruthy()
  })

  test('a file in this chat asks the engine for its diff, and shows the answer', async () => {
    const { controller, commands } = recorder()
    const state = build(ready(), changes(['src/main.rs', 'modified', 1, 1]))
    const { rerender } = mount(state, undefined, controller)
    fireEvent.click(within(pane()).getByRole('button', { name: /src\/main\.rs/ }))
    await waitFor(() => expect(commands()).toEqual([{ kind: 'command', type: 'slash', command: 'diff', args: 'src/main.rs' }]))
    rerender(reduceSession(state, diffOf('src/main.rs')))
    expect(within(pane()).getByLabelText('diff of src/main.rs').textContent).toContain('from the agent')
  })

  test('the diff waits while the agent is busy, then loads once idle', async () => {
    const { controller, commands } = recorder()
    const working = busy(build(ready(), changes(['src/main.rs', 'modified', 1, 1])))
    const { rerender } = mount(working, undefined, controller)
    fireEvent.click(within(pane()).getByRole('button', { name: /src\/main\.rs/ }))
    expect(within(pane()).getByText('The diff loads when the agent is idle')).toBeTruthy()
    expect(commands()).toEqual([])
    rerender(reduceSession(working, { type: 'turn_finished' }))
    await waitFor(() => expect(commands().map((command) => command.args)).toEqual(['src/main.rs']))
    expect(within(pane()).queryByText('The diff loads when the agent is idle')).toBeNull()
  })

  test('a new changes event asks for the open file again', async () => {
    const { controller, commands } = recorder()
    const first = build(ready(), changes(['src/main.rs', 'modified', 1, 1]))
    const { rerender } = mount(first, undefined, controller)
    fireEvent.click(within(pane()).getByRole('button', { name: /src\/main\.rs/ }))
    await waitFor(() => expect(commands()).toHaveLength(1))
    rerender(reduceSession(first, changes(['src/main.rs', 'modified', 4, 2])))
    await waitFor(() => expect(commands()).toHaveLength(2))
  })
})

describe('reverting one file', () => {
  const open = async () => {
    const platform = fakePlatform(new FakeEngine())
    const { controller, commands } = recorder()
    mount(build(ready(['capabilities'])), platform, controller)
    await within(pane()).findByRole('button', { name: /src\/main\.rs/ })
    return { platform, commands }
  }
  const rowOf = (path: string) => within(pane()).getByRole('button', { name: new RegExp(path.replace('.', '\\.')) }).closest('li') as HTMLElement

  test('cancel makes no call', async () => {
    const { platform } = await open()
    fireEvent.click(within(rowOf('src/main.rs')).getByRole('button', { name: 'Revert' }))
    const dialog = screen.getByRole('dialog', { name: /Revert src\/main\.rs/ })
    expect(dialog.textContent).toContain('last commit')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(platform.reverted).toEqual([])
  })

  test('confirm reverts a tracked file and refreshes the list', async () => {
    const { platform } = await open()
    fireEvent.click(within(rowOf('src/main.rs')).getByRole('button', { name: 'Revert' }))
    fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Revert file' }))
    await waitFor(() => expect(platform.reverted).toEqual(['src/main.rs']))
    await waitFor(() => expect(pane().textContent).not.toContain('src/main.rs'), { timeout: 5000 })
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  test('an untracked file says it will be deleted', async () => {
    const { platform } = await open()
    fireEvent.click(within(rowOf('notes.txt')).getByRole('button', { name: 'Revert' }))
    const dialog = screen.getByRole('dialog', { name: /Delete notes\.txt/ })
    expect(dialog.textContent).toContain('new file')
    expect(dialog.textContent).toContain('deleted')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete file' }))
    await waitFor(() => expect(platform.reverted).toEqual(['notes.txt']))
  })

  test('a refused revert shows the host message and keeps the list', async () => {
    const { platform } = await open()
    platform.gitRevert = async () => {
      throw new Error('That file has no changes to revert.')
    }
    fireEvent.click(within(rowOf('src/main.rs')).getByRole('button', { name: 'Revert' }))
    fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Revert file' }))
    expect(await within(pane()).findByText('That file has no changes to revert.')).toBeTruthy()
    expect(within(pane()).getByRole('button', { name: /src\/main\.rs/ })).toBeTruthy()
  })

  test('the confirmation passes axe', async () => {
    await open()
    fireEvent.click(within(rowOf('src/main.rs')).getByRole('button', { name: 'Revert' }))
    await expectAccessible(screen.getByRole('dialog'))
  })
})

describe('open in editor', () => {
  test('each file in the list has a button', async () => {
    const platform = fakePlatform(new FakeEngine())
    mount(build(ready(['capabilities'])), platform)
    const row = (await within(pane()).findByRole('button', { name: /notes\.txt/ })).closest('li') as HTMLElement
    fireEvent.click(within(row).getByRole('button', { name: 'Open in editor' }))
    await waitFor(() => expect(platform.editorOpens).toEqual(['notes.txt']))
  })

  test('a failure is shown', async () => {
    const platform = fakePlatform(new FakeEngine())
    platform.openInEditor = async () => {
      throw new Error('That file is not there.')
    }
    mount(build(ready(), changes(['gone.rs', 'modified', 1, 0])), platform)
    fireEvent.click(within(pane()).getByRole('button', { name: 'Open in editor' }))
    expect(await within(pane()).findByText('That file is not there.')).toBeTruthy()
  })

  test('the file preview has one too', async () => {
    const platform = fakePlatform(new FakeEngine())
    render(
      <SidePane
        platform={platform}
        tab="files"
        onTab={() => {}}
        refreshKey={0}
        changed={new Set()}
        tool={null}
        todos={[]}
        subagents={[]}
        terminalOpened={false}
      />,
    )
    const tree = await screen.findByRole('navigation', { name: 'Files' })
    fireEvent.click(await within(tree).findByRole('button', { name: /README\.md/ }))
    await screen.findByTestId('file-view')
    fireEvent.click(screen.getByRole('button', { name: 'Open in editor' }))
    await waitFor(() => expect(platform.editorOpens).toEqual(['README.md']))
  })
})

const PROPOSAL: UiEvent = {
  type: 'commit_proposal',
  message: 'Greet from the agent',
  branch: 'main',
  files: [
    { path: 'src/main.rs', status: 'modified', added: 2, removed: 1 },
    { path: 'src/new.rs', status: 'created', added: 9, removed: 0 },
  ],
  stage_all: true,
}

describe('the commit card', () => {
  test('the Commit button asks the engine for a draft, and is off while busy', () => {
    const { controller, commands } = recorder()
    const idle = build(ready(), changes(['a.rs', 'modified', 1, 0]))
    const { rerender } = mount(idle, undefined, controller)
    fireEvent.click(within(pane()).getByRole('button', { name: 'Commit' }))
    expect(commands()).toEqual([{ kind: 'command', type: 'slash', command: 'commit', args: '' }])
    rerender(busy(idle))
    expect((within(pane()).getByRole('button', { name: 'Commit' }) as HTMLButtonElement).disabled).toBe(true)
  })

  test('no Commit button without git_views', () => {
    mount(build(ready(['capabilities'])))
    expect(within(pane()).queryByRole('button', { name: 'Commit' })).toBeNull()
  })

  test('shows the message, branch, files and the stage-all note', () => {
    mount(build(ready(), changes(['src/main.rs', 'modified', 2, 1]), PROPOSAL))
    const card = within(pane()).getByRole('form', { name: 'Commit' })
    expect((within(card).getByRole('textbox', { name: 'Commit message' }) as HTMLTextAreaElement).value).toBe('Greet from the agent')
    expect(card.textContent).toContain('main')
    expect(card.textContent).toContain('src/new.rs')
    expect(card.textContent).toContain('+9')
    expect(card.textContent).toContain('Commits every change (nothing is staged)')
  })

  test('only staged changes: no stage-all note', () => {
    mount(build(ready(), { ...(PROPOSAL as object), stage_all: false } as UiEvent))
    expect(within(pane()).getByRole('form', { name: 'Commit' }).textContent).not.toContain('nothing is staged')
  })

  test('commit sends the edited message and closes the card', () => {
    const { controller, commands } = recorder()
    const proposed = build(ready(), PROPOSAL)
    const { rerender } = mount(proposed, undefined, controller)
    const box = within(pane()).getByRole('textbox', { name: 'Commit message' })
    fireEvent.change(box, { target: { value: 'My own words' } })
    fireEvent.click(within(pane()).getByRole('button', { name: 'Commit changes' }))
    expect(commands()).toEqual([{ kind: 'command', type: 'commit', message: 'My own words' }])
    // The controller reports the proposal as done with: the card stays, committing, until the engine answers.
    const sentState = reduceSession(proposed, { type: 'local/commit_sent', message: 'My own words' })
    rerender(sentState)
    expect(within(pane()).getByRole('button', { name: 'Committing…' })).toBeTruthy()
    expect((within(pane()).getByRole('textbox', { name: 'Commit message' }) as HTMLTextAreaElement).value).toBe('My own words')
    rerender(reduceSession(sentState, { type: 'notice', message: 'Committed 1a2b3c4 on main' }))
    expect(within(pane()).queryByRole('form', { name: 'Commit' })).toBeNull()
    expect(within(pane()).getByRole('status').textContent).toContain('Committed 1a2b3c4 on main')
  })

  test('a failed commit shows the card again with the message and the error, and can be retried', () => {
    const { controller, commands } = recorder()
    const proposed = build(ready(), PROPOSAL)
    const { rerender } = mount(proposed, undefined, controller)
    fireEvent.change(within(pane()).getByRole('textbox', { name: 'Commit message' }), { target: { value: 'My own words' } })
    fireEvent.click(within(pane()).getByRole('button', { name: 'Commit changes' }))
    const sentState = reduceSession(proposed, { type: 'local/commit_sent', message: 'My own words' })
    rerender(reduceSession(sentState, { type: 'error', message: 'git commit failed: hook rejected it' }))
    const card = within(pane()).getByRole('form', { name: 'Commit' })
    expect((within(card).getByRole('textbox', { name: 'Commit message' }) as HTMLTextAreaElement).value).toBe('My own words')
    expect(within(card).getByRole('alert').textContent).toContain('hook rejected it')
    fireEvent.click(within(card).getByRole('button', { name: 'Commit changes' }))
    expect(commands().at(-1)).toEqual({ kind: 'command', type: 'commit', message: 'My own words' })
    // The old error does not count for the retry.
    expect(within(pane()).queryByRole('alert')).toBeNull()
  })

  test('the outcome belongs to one commit and goes when the next draft arrives', () => {
    const proposed = build(ready(), PROPOSAL)
    const { rerender } = mount(proposed, undefined, recorder().controller)
    fireEvent.click(within(pane()).getByRole('button', { name: 'Commit changes' }))
    const sentState = reduceSession(proposed, { type: 'local/commit_sent', message: 'x' })
    const done = reduceSession(sentState, { type: 'notice', message: 'Committed 1a2b3c4 on main' })
    rerender(done)
    expect(within(pane()).getByRole('status').textContent).toContain('Committed')
    rerender(reduceSession(done, { ...(PROPOSAL as object), message: 'Second draft' } as UiEvent))
    expect(within(pane()).queryByText(/Committed 1a2b3c4/)).toBeNull()
    expect((within(pane()).getByRole('textbox', { name: 'Commit message' }) as HTMLTextAreaElement).value).toBe('Second draft')
  })

  test('commit is off for an empty message', () => {
    mount(build(ready(), PROPOSAL))
    fireEvent.change(within(pane()).getByRole('textbox', { name: 'Commit message' }), { target: { value: '   ' } })
    expect((within(pane()).getByRole('button', { name: 'Commit changes' }) as HTMLButtonElement).disabled).toBe(true)
  })

  test('cancel dismisses without sending', () => {
    const { controller, commands } = recorder()
    mount(build(ready(), PROPOSAL), undefined, controller)
    fireEvent.click(within(pane()).getByRole('button', { name: 'Cancel' }))
    expect(commands()).toEqual([])
    expect(controller.getState().commitProposal).toBeNull()
  })

  test('an engine error after the commit shows in the card', () => {
    const { controller } = recorder()
    const proposed = build(ready(), PROPOSAL)
    const { rerender } = mount(proposed, undefined, controller)
    fireEvent.click(within(pane()).getByRole('button', { name: 'Commit changes' }))
    const sentState = reduceSession(proposed, { type: 'local/commit_sent', message: 'x' })
    rerender(reduceSession(sentState, { type: 'error', message: 'git commit failed: hook rejected it' }))
    expect(within(pane()).getByRole('alert').textContent).toContain('git commit failed: hook rejected it')
  })

  test('the card passes axe', async () => {
    mount(build(ready(), PROPOSAL))
    await expectAccessible(pane())
  })
})

const CHECKPOINTS: UiEvent = {
  type: 'checkpoints',
  items: [
    { id: 2, created_at_ms: Date.UTC(2026, 10, 6, 12, 0), prompt_preview: 'Rename the helper', files: ['src/lib.rs'], shell_commands: ['cargo test'] },
    { id: 1, created_at_ms: Date.UTC(2026, 10, 6, 11, 0), prompt_preview: 'Add the greeting', files: ['src/main.rs', 'README.md'], shell_commands: [] },
  ],
}
const dryRun = (over: Record<string, unknown> = {}): UiEvent =>
  ({
    type: 'rewound',
    checkpoint: 1,
    mode: 'code_and_chat',
    dry_run: true,
    restored: ['src/main.rs'],
    removed: ['src/new.rs'],
    not_restored: [{ path: 'big.bin', reason: 'it was too large to save' }],
    shell_commands: ['cargo test'],
    removed_turns: 2,
    prompt: 'Add the greeting',
    ...over,
  }) as UiEvent

describe('the restore-point picker', () => {
  const openPicker = (state: SessionState, controller = recorder()) => {
    const view = mount(state, undefined, controller.controller)
    fireEvent.click(within(pane()).getByRole('button', { name: 'Restore point…' }))
    return { ...view, ...controller }
  }

  test('no button without the checkpoints feature', () => {
    mount(build(ready(['capabilities'])))
    expect(within(pane()).queryByRole('button', { name: 'Restore point…' })).toBeNull()
  })

  test('opening asks for the list and shows each point newest first', () => {
    const { commands, rerender } = openPicker(build(ready()))
    expect(commands()).toEqual([{ kind: 'command', type: 'slash', command: 'rewind', args: '' }])
    expect(screen.getByRole('dialog', { name: 'Restore point' })).toBeTruthy()
    rerender(build(ready(), CHECKPOINTS))
    const points = within(screen.getByRole('radiogroup', { name: 'Restore point' })).getAllByRole('radio')
    expect(points.map((point) => point.getAttribute('aria-label') ?? point.closest('label')?.textContent)).toEqual([
      expect.stringContaining('Rename the helper'),
      expect.stringContaining('Add the greeting'),
    ])
    const text = screen.getByRole('dialog').textContent ?? ''
    expect(text).toContain('src/lib.rs')
    expect(text).toContain('cargo test')
  })

  test('a dry run comes first, then the confirm runs the real rewind', () => {
    const base = build(ready(), CHECKPOINTS)
    const { commands, rerender } = openPicker(base)
    const dialog = screen.getByRole('dialog')
    // Nothing to confirm before a point is chosen and previewed.
    expect((within(dialog).getByRole('button', { name: 'Restore' }) as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(within(dialog).getByRole('radio', { name: /Add the greeting/ }))
    fireEvent.click(within(dialog).getByRole('radio', { name: /Code and chat/ }))
    fireEvent.click(within(dialog).getByRole('button', { name: 'Preview' }))
    expect(commands().at(-1)).toEqual({ kind: 'command', type: 'rewind', checkpoint: 1, mode: 'code_and_chat', dry_run: true })
    expect((within(dialog).getByRole('button', { name: 'Restore' }) as HTMLButtonElement).disabled).toBe(true)

    rerender(reduceSession(base, dryRun()))
    const preview = within(screen.getByRole('region', { name: 'What would change' }))
    expect(preview.getByText('src/main.rs')).toBeTruthy()
    expect(preview.getByText('src/new.rs')).toBeTruthy()
    expect(preview.getByText(/because it was too large to save/).textContent).toContain('big.bin')
    expect(preview.getByText(/cargo test/).closest('section')?.textContent).toContain("can't be undone")
    fireEvent.click(screen.getByRole('button', { name: 'Restore' }))
    expect(commands().at(-1)).toEqual({ kind: 'command', type: 'rewind', checkpoint: 1, mode: 'code_and_chat', dry_run: false })
  })

  test('each mode is sent as chosen', () => {
    const cases: Array<[RegExp, RewindMode]> = [
      [/Chat only/, 'chat_only'],
      [/Code only/, 'code_only'],
    ]
    for (const [label, mode] of cases) {
      const { commands } = openPicker(build(ready(), CHECKPOINTS))
      const dialog = screen.getByRole('dialog')
      fireEvent.click(within(dialog).getByRole('radio', { name: /Rename the helper/ }))
      fireEvent.click(within(dialog).getByRole('radio', { name: label }))
      fireEvent.click(within(dialog).getByRole('button', { name: 'Preview' }))
      expect(commands().at(-1)).toEqual({ kind: 'command', type: 'rewind', checkpoint: 2, mode, dry_run: true })
      cleanup()
    }
  })

  test('changing the choice after a preview asks for a new preview', () => {
    const base = build(ready(), CHECKPOINTS)
    const { rerender } = openPicker(base)
    fireEvent.click(screen.getByRole('radio', { name: /Add the greeting/ }))
    fireEvent.click(screen.getByRole('button', { name: 'Preview' }))
    rerender(reduceSession(base, dryRun()))
    expect((screen.getByRole('button', { name: 'Restore' }) as HTMLButtonElement).disabled).toBe(false)
    fireEvent.click(screen.getByRole('radio', { name: /Chat only/ }))
    expect((screen.getByRole('button', { name: 'Restore' }) as HTMLButtonElement).disabled).toBe(true)
  })

  test('after a real rewind: a short summary, and the prompt is back in the composer state', () => {
    const base = build(ready(), CHECKPOINTS)
    const { rerender } = openPicker(base)
    fireEvent.click(screen.getByRole('radio', { name: /Add the greeting/ }))
    fireEvent.click(screen.getByRole('button', { name: 'Preview' }))
    const previewed = reduceSession(base, dryRun())
    rerender(previewed)
    fireEvent.click(screen.getByRole('button', { name: 'Restore' }))
    const done = reduceSession(previewed, dryRun({ dry_run: false }))
    expect(done.unsent?.text).toBe('Add the greeting')
    rerender(done)
    const summary = screen.getByRole('status')
    expect(summary.textContent).toContain('Put back 1 file')
    expect(summary.textContent).toContain('removed 1')
    expect(summary.textContent).toContain('back in the message box')
    fireEvent.click(screen.getByRole('button', { name: 'Close' }))
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  test('busy: the picker says to wait and offers no preview', () => {
    openPicker(busy(build(ready(), CHECKPOINTS)))
    expect(screen.getByRole('dialog').textContent).toContain('agent is idle')
    fireEvent.click(screen.getByRole('radio', { name: /Add the greeting/ }))
    expect((screen.getByRole('button', { name: 'Preview' }) as HTMLButtonElement).disabled).toBe(true)
  })

  test('no checkpoints yet: a plain message', () => {
    openPicker(build(ready(), { type: 'checkpoints', items: [] }))
    expect(screen.getByRole('dialog').textContent).toContain('No restore points yet')
  })

  test('Escape closes it, and the dialog passes axe with a preview', async () => {
    const base = build(ready(), CHECKPOINTS)
    const { rerender } = openPicker(base)
    fireEvent.click(screen.getByRole('radio', { name: /Add the greeting/ }))
    rerender(reduceSession(base, dryRun()))
    await expectAccessible(screen.getByRole('dialog'))
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' })
    expect(screen.queryByRole('dialog')).toBeNull()
  })
})

// Keeps `act` imported for state changes outside testing-library helpers.
void act
