import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'fs'
import os from 'os'
import path from 'path'

import { afterEach, beforeEach, describe, expect, test } from 'bun:test'

import { prepareUserMessage } from '../../hooks/helpers/send-message'
import { setProjectRoot } from '../../project-files'
import { useAttachStore } from '../../state/attach-store'
import { useChatStore } from '../../state/chat-store'
import {
  ATTACHMENTS_DIR,
  attachFiles,
  attachedKeys,
  attachmentKey,
  resolveAttachPath,
  startAttach,
  waitForAttachCopies,
} from '../attach-files'

import type { SpawnedDialog } from '../file-dialog'
import type { PendingFileAttachment } from '../../types/store'

let tmp: string
let workspace: string
let outside: string

beforeEach(() => {
  tmp = mkdtempSync(path.join(os.tmpdir(), 'neuron-attach-'))
  workspace = path.join(tmp, 'ws')
  outside = path.join(tmp, 'elsewhere')
  mkdirSync(workspace)
  mkdirSync(outside)
  // A sibling whose name starts with the workspace's must still be outside.
  mkdirSync(path.join(tmp, 'ws2'))
  writeFileSync(path.join(workspace, 'notes.md'), '# Notes\nhello\n')
  writeFileSync(path.join(workspace, 'report.pdf'), '%PDF-1.7\u0000binary')
  writeFileSync(path.join(workspace, 'app.wasm'), Buffer.from([0, 97, 115, 109]))
  writeFileSync(path.join(workspace, 'long.log'), 'x'.repeat(200 * 1024))
  writeFileSync(path.join(workspace, 'shot.png'), Buffer.from([137, 80, 78, 71, 0]))
  writeFileSync(path.join(outside, 'deck.pptx'), 'PK\u0000\u0003')
  writeFileSync(path.join(outside, 'todo.txt'), 'buy milk')
  writeFileSync(path.join(tmp, 'ws2', 'sheet.xlsx'), 'PK\u0000\u0003')
  useChatStore.getState().reset()
  useAttachStore.getState().reset()
})

afterEach(() => {
  rmSync(tmp, { recursive: true, force: true })
  useChatStore.getState().reset()
  useAttachStore.getState().reset()
})

const files = () =>
  useChatStore
    .getState()
    .pendingAttachments.filter((a): a is PendingFileAttachment => a.kind === 'file')

const waitForReads = () => new Promise((resolve) => setTimeout(resolve, 10))

describe('attachFiles', () => {
  test('a text file is read into a file attachment', async () => {
    const outcome = attachFiles(['notes.md'], { root: workspace })
    expect(outcome.refused).toEqual([])
    await waitForReads()
    const [file] = files()
    expect(file?.path).toBe(path.join(workspace, 'notes.md'))
    expect(file?.readWith).toBeUndefined()
    expect(file?.content).toContain('hello')
    expect(file?.status).toBe('ready')
  })

  test('a document goes by path, for read_document', () => {
    attachFiles([path.join(workspace, 'report.pdf')], { root: workspace })
    const [file] = files()
    expect(file?.readWith).toBe('read_document')
    expect(file?.content).toBe('')
    expect(file?.note).toContain('document')
  })

  test('a binary file that is not a document is refused: the engine cannot read it', () => {
    const outcome = attachFiles(['app.wasm'], { root: workspace })
    expect(files()).toEqual([])
    expect(outcome.refused).toEqual(['app.wasm was not attached: it is a binary file Neuron cannot read.'])
    expect(useAttachStore.getState().notices[0]?.text).toBe(outcome.refused[0])
  })

  test('text too long to inline whole goes by path, for read_file to read all of it', () => {
    attachFiles(['long.log'], { root: workspace })
    const [file] = files()
    expect(file?.readWith).toBe('read_file')
    expect(file?.content).toBe('')
  })

  test('a document outside the workspace is copied into it, git-ignored, and the copy attached', async () => {
    const picked = [path.join(outside, 'deck.pptx'), path.join(tmp, 'ws2', 'sheet.xlsx')]
    const outcome = attachFiles(picked, { root: workspace })
    expect(outcome.refused).toEqual([])
    expect(outcome.copied).toEqual(picked)
    // Shown at once, holding the send while the copy is made.
    expect(files().map((file) => file.status)).toEqual(['processing', 'processing'])
    await waitForAttachCopies()
    const attached = files()
    expect(attached.map((file) => file.status)).toEqual(['ready', 'ready'])
    expect(attached.map((file) => file.sourcePath)).toEqual(picked)
    expect(attached.map((file) => file.filename)).toEqual(['deck.pptx', 'sheet.xlsx'])
    expect(attached.map((file) => file.readWith)).toEqual(['read_document', 'read_document'])
    for (const file of attached) {
      expect(path.dirname(path.dirname(file.path))).toBe(path.join(workspace, ATTACHMENTS_DIR))
      expect(readFileSync(file.path)).toEqual(readFileSync(file.sourcePath!))
      expect(file.note).toStartWith('copied · document')
    }
    expect(attached[0]!.path).not.toBe(attached[1]!.path)
    expect(readFileSync(path.join(workspace, ATTACHMENTS_DIR, '.gitignore'), 'utf8')).toContain('\n*\n')
  })

  test('long text outside the workspace is copied in for read_file', async () => {
    writeFileSync(path.join(outside, 'server.log'), 'y'.repeat(200 * 1024))
    attachFiles([path.join(outside, 'server.log')], { root: workspace })
    await waitForAttachCopies()
    const [file] = files()
    expect(file?.readWith).toBe('read_file')
    expect(file?.sourcePath).toBe(path.join(outside, 'server.log'))
    expect(existsSync(file!.path)).toBe(true)
  })

  test('a copy that fails drops the card and says why', async () => {
    const outcome = attachFiles([path.join(outside, 'deck.pptx')], {
      root: workspace,
      copy: async () => {
        throw Object.assign(new Error('no space'), { code: 'ENOSPC' })
      },
    })
    expect(outcome.copied).toHaveLength(1)
    await waitForAttachCopies()
    expect(files()).toEqual([])
    expect(useAttachStore.getState().notices.map((notice) => notice.text)).toEqual([
      'deck.pptx was not attached: it could not be copied into the project (ENOSPC).',
    ])
  })

  test('a file outside the workspace attached twice is copied once, and counts as attached', async () => {
    attachFiles([path.join(outside, 'deck.pptx')], { root: workspace })
    await waitForAttachCopies()
    attachFiles([path.join(outside, 'deck.pptx')], { root: workspace })
    await waitForAttachCopies()
    expect(files()).toHaveLength(1)
    expect(attachedKeys().has(attachmentKey(path.join(outside, 'deck.pptx')))).toBe(true)
  })

  test('a text file outside the workspace is still inlined', async () => {
    attachFiles([path.join(outside, 'todo.txt')], { root: workspace })
    await waitForReads()
    expect(files()[0]?.content).toBe('buy milk')
  })

  test('an image is refused while images cannot reach the model', () => {
    const outcome = attachFiles(['shot.png'], { root: workspace, imagesReachModel: false })
    expect(useChatStore.getState().pendingAttachments).toEqual([])
    expect(outcome.refused[0]).toContain("can't see images")
  })

  test('an image takes the image path once images reach the model', () => {
    attachFiles(['shot.png'], { root: workspace, imagesReachModel: true })
    expect(useChatStore.getState().pendingAttachments[0]?.kind).toBe('image')
  })

  test('several files at once, each removable', () => {
    const outcome = attachFiles(['report.pdf', 'long.log'], { root: workspace })
    expect(outcome.attached).toHaveLength(2)
    useChatStore.getState().removePendingAttachment(path.join(workspace, 'report.pdf'))
    expect(files().map((file) => path.basename(file.path))).toEqual(['long.log'])
  })

  test('the same file twice is attached once', () => {
    attachFiles(['report.pdf', `.${path.sep}report.pdf`], { root: workspace })
    expect(files()).toHaveLength(1)
  })

  test.if(process.platform === 'win32')('on Windows, the same file in another case is attached once', () => {
    attachFiles(['report.pdf'], { root: workspace })
    attachFiles(['REPORT.PDF'], { root: workspace })
    expect(files()).toHaveLength(1)
    expect(attachmentKey(path.join(workspace, 'REPORT.PDF'))).toBe(attachmentKey(path.join(workspace, 'report.pdf')))
  })

  test('paths are taken literally: brackets, parentheses, quotes and spaces in names', () => {
    const odd = path.join(workspace, '[old] (v2)')
    mkdirSync(odd)
    writeFileSync(path.join(odd, "a 'b' {c}.pdf"), '%PDF\u0000')
    writeFileSync(path.join(workspace, '(final) deck.pptx'), 'PK\u0000')
    const picked = [path.join(odd, "a 'b' {c}.pdf"), path.join(workspace, '(final) deck.pptx')]
    const outcome = attachFiles(picked, { root: workspace })
    expect(outcome.refused).toEqual([])
    expect(files().map((file) => file.path)).toEqual(picked)
  })

  test('a leading ~ is home', () => {
    expect(resolveAttachPath('~/x.pdf', workspace, '/home/me')).toBe(path.resolve('/home/me', 'x.pdf'))
    expect(resolveAttachPath('~', workspace, '/home/me')).toBe('/home/me')
    expect(resolveAttachPath('docs/x.pdf', workspace)).toBe(path.join(workspace, 'docs', 'x.pdf'))
  })

  test('a missing file says so', () => {
    expect(attachFiles(['nope.txt'], { root: workspace }).refused[0]).toContain('nope.txt was not attached')
  })
})

describe('the prompt for attached files', () => {
  test('a document is named by path with the tool to read it; text is inlined', async () => {
    setProjectRoot(workspace)
    attachFiles(['report.pdf', 'notes.md'], { root: workspace })
    await waitForReads()
    const { finalContent } = await prepareUserMessage({
      content: 'summarise these',
      agentMode: 'DEFAULT',
      attachments: useChatStore.getState().pendingAttachments,
      deps: {
        setMessages: () => {},
        lastMessageMode: 'DEFAULT',
        setLastMessageMode: () => {},
        scrollToLatest: () => {},
        setHasReceivedPlanResponse: () => {},
      },
    })
    expect(finalContent).toContain(
      `[File: ${path.join(workspace, 'report.pdf')}]\n(an attached document: read it with the read_document tool.)`,
    )
    expect(finalContent).toContain(`[File: ${path.join(workspace, 'notes.md')}]\n# Notes`)
    expect(finalContent).not.toContain('%PDF')
  })

  test('a copied document names the copy and where it came from', async () => {
    setProjectRoot(workspace)
    attachFiles([path.join(outside, 'deck.pptx')], { root: workspace })
    await waitForAttachCopies()
    const [file] = files()
    const { finalContent } = await prepareUserMessage({
      content: 'what is in the deck',
      agentMode: 'DEFAULT',
      attachments: useChatStore.getState().pendingAttachments,
      deps: {
        setMessages: () => {},
        lastMessageMode: 'DEFAULT',
        setLastMessageMode: () => {},
        scrollToLatest: () => {},
        setHasReceivedPlanResponse: () => {},
      },
    })
    expect(finalContent).toContain(
      `[File: ${file!.path}]\n(an attached document, copied from ${path.join(outside, 'deck.pptx')}: read it with the read_document tool.)`,
    )
  })
})

function dialogReturning(stdout: string, code = 0): () => SpawnedDialog {
  return () => ({
    exited: Promise.resolve(code),
    stdout: Promise.resolve(stdout),
    stderr: Promise.resolve(''),
    kill: () => {},
  })
}

const linuxDesktop = {
  platform: 'linux' as const,
  env: { DISPLAY: ':0' },
  which: (command: string) => (command === 'zenity' ? '/usr/bin/zenity' : null),
}

describe('startAttach', () => {
  test('with no dialog possible, the in-UI browser opens at the workspace', async () => {
    const result = await startAttach({ root: workspace, platform: 'linux', env: {}, which: () => null, spawner: dialogReturning('') })
    expect(result).toBe('picker')
    expect(useAttachStore.getState().pickerOpen).toBe(true)
    expect(useAttachStore.getState().pickerStartDir).toBe(workspace)
  })

  test('the dialog attaches what was picked', async () => {
    const picked = `${path.join(workspace, 'report.pdf')}\n${path.join(workspace, 'long.log')}\n`
    const result = await startAttach({ root: workspace, ...linuxDesktop, spawner: dialogReturning(picked) })
    expect(result).toBe('dialog')
    expect(files().map((file) => file.readWith)).toEqual(['read_document', 'read_file'])
    expect(useAttachStore.getState().dialogOpen).toBe(false)
  })

  test('a cancelled dialog attaches nothing and opens nothing', async () => {
    const result = await startAttach({ root: workspace, ...linuxDesktop, spawner: dialogReturning('', 1) })
    expect(result).toBe('dialog')
    expect(useChatStore.getState().pendingAttachments).toEqual([])
    expect(useAttachStore.getState().pickerOpen).toBe(false)
  })

  test('a dialog that fails falls back to the in-UI browser, saying why', async () => {
    const result = await startAttach({ root: workspace, ...linuxDesktop, spawner: dialogReturning('', 3) })
    expect(result).toBe('picker')
    expect(useAttachStore.getState().pickerOpen).toBe(true)
    expect(useAttachStore.getState().notices[0]?.text).toContain('The file dialog did not work')
  })
})
