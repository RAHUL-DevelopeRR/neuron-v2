/**
 * v1.1: branch changes in the side pane, image and media previews in Files,
 * the pull request chip in the chat header, a copy started from a chosen
 * branch, and the question before quitting while chats work.
 */
import { afterEach, describe, expect, test } from 'bun:test'
import { act, cleanup, configure, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

import { initialSession, reduceSession, type SessionState } from '@neuron/client'

import { App } from '../src/App'
import { SidePane, type PaneTab } from '../src/components/SidePane'
import { QuitDialog } from '../src/components/QuitDialog'
import { previewOf, imageMimeType, mediaType } from '../src/mediaFiles'
import type { WorktreeInfo } from '../src/platform'
import { FakeEngine, fakePlatform, type FakePlatform } from './fake-platform'

configure({ asyncUtilTimeout: 5000 })

afterEach(() => {
  cleanup()
  try {
    localStorage.clear()
  } catch {
    // No storage: nothing to clear.
  }
})

const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])
const COPY: WorktreeInfo = { path: '/wt/fix', branch: 'neuron/fix', base: 'main', named: true }
const idle = (): SessionState => reduceSession(initialSession(), { type: 'ready', protocol: 2, version: '1.0.0', features: [] })

function pane(platform: FakePlatform, tab: PaneTab, worktree: WorktreeInfo | null = COPY) {
  return render(
    <SidePane
      platform={platform}
      tab={tab}
      onTab={() => {}}
      refreshKey={0}
      changed={new Set()}
      tool={null}
      todos={[]}
      subagents={[]}
      terminalOpened={false}
      state={idle()}
      controller={null}
      chatKey="c1test"
      worktree={worktree}
    />,
  )
}

const details = () => screen.getByRole('complementary', { name: 'Details' })

// Cases from Freebuff's image-files.test.ts (commit 0dc95cb, Apache-2.0), for the ported function.
describe('which files preview as media', () => {
  test('recognizes supported image extensions without misclassifying text or extensionless paths', () => {
    for (const extension of ['png', 'jpg', 'jpeg', 'gif', 'webp', 'avif', 'svg', 'ico', 'bmp']) {
      expect(imageMimeType(`assets/photo.${extension}`)?.startsWith('image/')).toBe(true)
    }
    expect(imageMimeType('PHOTO.PNG')).toBe('image/png')
    for (const path of ['png', 'src/png', 'a.png/source', 'image.png.ts', 'data.bin', 'README.md']) {
      expect(imageMimeType(path)).toBeUndefined()
    }
  })

  test('video and sound by extension; everything else is text', () => {
    expect(mediaType('clip.webm')).toEqual({ kind: 'video', mimeType: 'video/webm' })
    expect(mediaType('voice.M4A')).toEqual({ kind: 'audio', mimeType: 'audio/mp4' })
    expect(previewOf('logo.svg')).toEqual({ kind: 'image', mimeType: 'image/svg+xml' })
    expect(previewOf('song.mp3')?.kind).toBe('audio')
    expect(previewOf('src/main.rs')).toBeNull()
  })
})

describe('Branch changes in the side pane', () => {
  test('a chat in its own copy lists what its branch changed since it left its base', async () => {
    const platform = fakePlatform(new FakeEngine(), { multiChat: true })
    platform.fakeChats!.branchState = {
      branch: 'neuron/fix',
      base: 'main',
      commits: 2,
      files: [
        { path: 'src/a.ts', status: 'modified', adds: 3, dels: 1, binary: false, untracked: false },
        { path: 'logo.png', status: 'added', adds: null, dels: null, binary: true, untracked: false },
        { path: 'new.txt', status: 'added', adds: 2, dels: 0, binary: false, untracked: true },
      ],
      totals: { files: 3, adds: 5, dels: 1 },
      truncated: false,
    }
    pane(platform, 'changes')
    fireEvent.click(within(details()).getByRole('button', { name: 'Branch changes' }))
    const summary = await within(details()).findByText(/since main/)
    expect(summary.textContent).toContain('neuron/fix')
    expect(summary.textContent).toContain('2 commits')
    expect(summary.textContent).toContain('3 files')
    expect(summary.textContent).toContain('+5')
    expect(summary.textContent).toContain('−1')
    const row = within(details()).getByRole('button', { name: /src\/a\.ts/ })
    expect(row.textContent).toContain('+3')
    expect(within(details()).getByRole('button', { name: /new\.txt/ }).textContent).toContain('new')
    // Nothing here reverts: the branch's commits are not one file's state.
    expect(within(details()).queryAllByRole('button', { name: 'Revert' }).length).toBe(0)
    fireEvent.click(row)
    await waitFor(() => expect(platform.fakeChats!.branchDiffs).toEqual([['c1test', 'src/a.ts']]))
    expect((await within(details()).findByLabelText('diff of src/a.ts')).textContent).toContain('added in src/a.ts')
  })

  test('an empty branch says so, and a failure is shown in words', async () => {
    const platform = fakePlatform(new FakeEngine(), { multiChat: true })
    const { unmount } = pane(platform, 'changes')
    fireEvent.click(within(details()).getByRole('button', { name: 'Branch changes' }))
    expect(await within(details()).findByText('Nothing has changed on this branch since it left main.')).toBeTruthy()
    unmount()
    platform.fakeChats!.branchError = 'Could not find where this branch left main.'
    pane(platform, 'changes')
    fireEvent.click(within(details()).getByRole('button', { name: 'Branch changes' }))
    expect(await within(details()).findByText('Could not find where this branch left main.')).toBeTruthy()
  })

  test('a chat in the project folder has no branch scope', () => {
    const platform = fakePlatform(new FakeEngine(), { multiChat: true })
    pane(platform, 'changes', null)
    expect(within(details()).queryAllByRole('button', { name: 'Branch changes' }).length).toBe(0)
  })
})

describe('image and media previews in Files', () => {
  const open = async (platform: FakePlatform, name: string) => {
    pane(platform, 'files')
    fireEvent.click(await within(details()).findByRole('button', { name: new RegExp(name.replace('.', '\\.')) }))
  }

  test('an image shows as an image, read through the media path', async () => {
    const platform = fakePlatform(new FakeEngine())
    platform.media.set('logo.png', PNG)
    await open(platform, 'logo.png')
    const image = await within(details()).findByRole('img', { name: 'logo.png' })
    expect(image.getAttribute('src')?.startsWith('blob:')).toBe(true)
    expect(platform.mediaReads).toEqual(['logo.png'])
    expect(within(details()).queryAllByText(/Binary file/).length).toBe(0)
  })

  test('an svg is an image, never markup in the page', async () => {
    const platform = fakePlatform(new FakeEngine())
    platform.media.set('icon.svg', new TextEncoder().encode('<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>'))
    await open(platform, 'icon.svg')
    await within(details()).findByRole('img', { name: 'icon.svg' })
    expect(details().querySelectorAll('svg script').length).toBe(0)
  })

  test('video and sound get players', async () => {
    const platform = fakePlatform(new FakeEngine())
    platform.media.set('clip.mp4', new Uint8Array([0, 0, 0, 24]))
    await open(platform, 'clip.mp4')
    await waitFor(() => expect(details().querySelectorAll('video[controls]').length).toBe(1))
  })

  test('a file too large to show says so', async () => {
    const platform = fakePlatform(new FakeEngine())
    platform.media.set('huge.png', PNG)
    platform.readMedia = async () => {
      throw new Error('This file is too large to show here (the most is 10 MB).')
    }
    await open(platform, 'huge.png')
    expect(await within(details()).findByText('This file is too large to show here (the most is 10 MB).')).toBeTruthy()
  })
})

describe('the question before quitting', () => {
  test('the words count the chats, and Keep working is the safe default', () => {
    const answers: boolean[] = []
    const { rerender } = render(<QuitDialog request={{ id: 1, working: 2, waiting: 0 }} onAnswer={(quit) => answers.push(quit)} />)
    expect(screen.getByRole('alertdialog').textContent).toContain('2 chats are still working. Quit anyway?')
    expect(document.activeElement?.textContent).toBe('Keep working')
    rerender(<QuitDialog request={{ id: 2, working: 0, waiting: 1 }} onAnswer={(quit) => answers.push(quit)} />)
    expect(screen.getByRole('alertdialog').textContent).toContain('1 chat is waiting for you. Quit anyway?')
    rerender(<QuitDialog request={{ id: 3, working: 1, waiting: 1 }} onAnswer={(quit) => answers.push(quit)} />)
    expect(screen.getByRole('alertdialog').textContent).toContain('1 chat is still working and 1 is waiting for you. Quit anyway?')
    fireEvent.keyDown(screen.getByRole('alertdialog'), { key: 'Escape' })
    fireEvent.click(screen.getByRole('button', { name: 'Quit' }))
    expect(answers).toEqual([false, true])
  })

  test('the app asks when the host asks, and sends the answer back once', async () => {
    const platform = fakePlatform(new FakeEngine(), { multiChat: true })
    render(<App platform={platform} />)
    await screen.findByLabelText('Message Neuron')
    act(() => platform.requestQuit({ id: 7, working: 1, waiting: 0 }))
    const dialog = await screen.findByRole('alertdialog', { name: /Quit Neuron/ })
    fireEvent.click(within(dialog).getByRole('button', { name: 'Keep working' }))
    expect(platform.quitAnswers).toEqual([[7, false]])
    expect(screen.queryAllByRole('alertdialog').length).toBe(0)
    act(() => platform.requestQuit({ id: 8, working: 1, waiting: 0 }))
    fireEvent.click(within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Quit' }))
    expect(platform.quitAnswers).toEqual([[7, false], [8, true]])
  })
})

describe('the chat header and new copies', () => {
  test('a pull request for the chat\'s branch shows as a chip that opens it', async () => {
    const platform = fakePlatform(new FakeEngine(), { multiChat: true })
    platform.fakeChats!.pr = {
      pullRequest: {
        url: 'https://github.com/acme/app/pull/12',
        number: 12,
        title: 'Fix the login redirect',
        state: 'open',
        draft: true,
        conflict: false,
        checks: [{ state: 'SUCCESS' }, { state: 'SUCCESS' }, { state: 'SUCCESS' }, { state: 'PENDING' }],
      },
    }
    render(<App platform={platform} />)
    const chip = await screen.findByRole('button', { name: /pull request #12/i })
    expect(chip.textContent).toBe('PR #12 · draft · 3/4 checks')
    fireEvent.click(chip)
    expect(platform.opened).toEqual(['https://github.com/acme/app/pull/12'])
  })

  test('a copy can be started from a chosen branch', async () => {
    const platform = fakePlatform(new FakeEngine(), { multiChat: true })
    render(<App platform={platform} />)
    await screen.findByLabelText('Message Neuron')
    fireEvent.keyDown(window, { key: 'k', ctrlKey: true })
    fireEvent.click(await screen.findByRole('option', { name: /from a branch/ }))
    const dialog = await screen.findByRole('dialog', { name: 'New chat in its own copy' })
    const select = (await within(dialog).findByLabelText('Start from')) as HTMLSelectElement
    expect(select.value).toBe('main')
    fireEvent.change(select, { target: { value: 'feature/login' } })
    fireEvent.click(within(dialog).getByRole('button', { name: 'Make the copy' }))
    await waitFor(() => expect(platform.fakeChats!.opened.at(-1)).toEqual({ path: '/work/app', worktree: { start: 'feature/login' } }))
  })
})
