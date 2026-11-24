import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'fs'
import os from 'os'
import path from 'path'

import { afterEach, beforeAll, beforeEach, describe, expect, mock, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import { findCommand } from '../../commands/command-registry'
import { SLASH_COMMANDS } from '../../data/slash-commands'
import { initializeThemeStore, useTheme } from '../../hooks/use-theme'
import { useAttachStore } from '../../state/attach-store'
import { useChatStore } from '../../state/chat-store'
import { useEngineStore } from '../../state/engine-store'
import { getPathCompletion } from '../../utils/path-completion'
import { AttachFilePicker, filterEntries, isTypedPath, resolveTypedPath, typedText } from '../attach-file-picker'
import { ChatInputBar, attachButtonLabel } from '../chat-input-bar'
import { PendingAttachmentsBanner } from '../pending-attachments-banner'

import type { RouterParams } from '../../commands/command-registry'
import type { PendingFileAttachment } from '../../types/store'

let cleanup: (() => void) | undefined
let tmp: string

beforeAll(() => {
  initializeThemeStore()
})

beforeEach(() => {
  tmp = mkdtempSync(path.join(os.tmpdir(), 'neuron-attach-ui-'))
  mkdirSync(path.join(tmp, 'docs'))
  writeFileSync(path.join(tmp, 'docs', 'plan.docx'), 'PK\u0000\u0003')
  writeFileSync(path.join(tmp, 'notes.md'), '# hi')
  writeFileSync(path.join(tmp, 'report.pdf'), '%PDF\u0000')
  useChatStore.getState().reset()
  useAttachStore.getState().reset()
  useEngineStore.getState().reset()
})

afterEach(() => {
  cleanup?.()
  cleanup = undefined
  rmSync(tmp, { recursive: true, force: true })
  useChatStore.getState().reset()
  useAttachStore.getState().reset()
})

async function mountInputBar(options: { compact?: boolean; narrow?: boolean; onAttach?: () => void }) {
  const Harness = () => {
    const theme = useTheme()
    const inputRef = React.useRef(null)
    return (
      <ChatInputBar
        inputValue=""
        cursorPosition={0}
        setInputValue={() => {}}
        inputFocused
        inputRef={inputRef}
        inputPlaceholder="composer"
        lastEditDueToNav={false}
        agentMode="DEFAULT"
        toggleAgentMode={() => {}}
        setAgentMode={() => {}}
        hasSlashSuggestions={false}
        hasMentionSuggestions={false}
        hasSuggestionMenu={false}
        slashSuggestionItems={[]}
        agentSuggestionItems={[]}
        fileSuggestionItems={[]}
        slashSelectedIndex={0}
        agentSelectedIndex={0}
        theme={theme}
        terminalHeight={12}
        separatorWidth={70}
        shouldCenterInputVertically={false}
        inputBoxTitle={undefined}
        isCompactHeight={options.compact ?? false}
        isNarrowWidth={options.narrow ?? false}
        handleSubmit={async () => {}}
        onPaste={() => {}}
        onInterruptStream={() => {}}
        onAttachClick={options.onAttach}
      />
    )
  }
  const setup = await createTestRenderer({ width: 70, height: 12 })
  const root = createRoot(setup.renderer)
  cleanup = () => {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
  flushSync(() => root.render(<Harness />))
  await setup.renderOnce()
  return setup
}

/** The cell of `text`'s first character in the frame. */
function locate(frame: string, text: string): { x: number; y: number } | null {
  const lines = frame.split('\n')
  for (let y = 0; y < lines.length; y++) {
    const x = lines[y]!.indexOf(text)
    if (x >= 0) return { x, y }
  }
  return null
}

describe('the Attach button', () => {
  test('shows a paperclip and the word Attach, or just the paperclip when narrow', () => {
    expect(attachButtonLabel(false)).toBe('📎 Attach')
    expect(attachButtonLabel(true)).toBe('📎')
  })

  test('is drawn at the end of the input row and clicking it attaches', async () => {
    let clicks = 0
    const ui = await mountInputBar({ onAttach: () => clicks++ })
    const frame = ui.captureCharFrame()
    const at = locate(frame, 'Attach')
    expect(at).not.toBeNull()
    await ui.mockMouse.click(at!.x + 1, at!.y)
    expect(clicks).toBe(1)
    // Clicking the composer does not.
    await ui.mockMouse.click(4, at!.y)
    expect(clicks).toBe(1)
  })

  test('is in the compact layout too', async () => {
    let clicks = 0
    const ui = await mountInputBar({ compact: true, onAttach: () => clicks++ })
    const at = locate(ui.captureCharFrame(), 'Attach')
    expect(at).not.toBeNull()
    await ui.mockMouse.click(at!.x + 1, at!.y)
    expect(clicks).toBe(1)
  })

  test('is not drawn without a handler', async () => {
    const ui = await mountInputBar({})
    expect(ui.captureCharFrame()).not.toContain('Attach')
  })
})

describe('/attach', () => {
  const params = (inputValue: string): RouterParams => ({
    agentMode: 'DEFAULT',
    inputRef: { current: null },
    inputValue,
    isChainInProgressRef: { current: false },
    isStreaming: false,
    streamMessageIdRef: { current: null },
    addToQueue: mock(() => {}),
    clearMessages: mock(() => {}),
    saveToHistory: mock(() => {}),
    scrollToLatest: mock(() => {}),
    sendMessage: mock(async () => {}),
    setCanProcessQueue: mock(() => {}),
    setInputFocused: mock(() => {}),
    setInputValue: mock(() => {}),
    setMessages: mock(() => {}),
  })

  test('is in the slash menu and is no longer an alias of /image', () => {
    expect(SLASH_COMMANDS.some((command) => command.id === 'attach')).toBe(true)
    expect(findCommand('attach')?.name).toBe('attach')
    expect(findCommand('image')?.aliases).not.toContain('attach')
  })

  test('with no path it opens the file browser where no dialog can open (tests never open one)', async () => {
    useEngineStore.getState().setStatus({ cwd: tmp } as never)
    await findCommand('attach')!.handler(params('/attach'), '')
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(useAttachStore.getState().pickerOpen).toBe(true)
    expect(useAttachStore.getState().pickerStartDir).toBe(tmp)
  })

  test('with a path it attaches that file, quoting optional', async () => {
    useEngineStore.getState().setStatus({ cwd: tmp } as never)
    await findCommand('attach')!.handler(params('/attach report.pdf'), 'report.pdf')
    const [file] = useChatStore.getState().pendingAttachments as PendingFileAttachment[]
    expect(file?.path).toBe(path.join(tmp, 'report.pdf'))
    expect(file?.readWith).toBe('read_document')
  })
})

describe('the attachments banner', () => {
  test('says why a file was not attached, and x dismisses it', async () => {
    useAttachStore.getState().addNotice('deck.pptx was not attached: it is outside the project folder.')
    const setup = await createTestRenderer({ width: 80, height: 8 })
    const root = createRoot(setup.renderer)
    cleanup = () => {
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    }
    flushSync(() => root.render(<PendingAttachmentsBanner />))
    await setup.renderOnce()
    const frame = setup.captureCharFrame()
    expect(frame).toContain('deck.pptx was not attached')
    const close = frame.split('\n').map((line, y) => ({ x: line.lastIndexOf('x'), y, line })).find((row) =>
      row.line.includes('deck.pptx'),
    )!
    await setup.mockMouse.click(close.x, close.y)
    expect(useAttachStore.getState().notices).toEqual([])
  })
})

describe('the in-UI file browser', () => {
  test('typed paths and filters', () => {
    expect(isTypedPath('C:\\Users', 'win32')).toBe(true)
    expect(isTypedPath('D:', 'win32')).toBe(true)
    expect(isTypedPath('/etc', 'linux')).toBe(true)
    expect(isTypedPath('~/x', 'linux')).toBe(true)
    expect(isTypedPath('report', 'linux')).toBe(false)
    expect(resolveTypedPath('~/a', '/ws', '/home/me')).toBe(path.join('/home/me', 'a'))
    expect(
      filterEntries(
        [
          { name: '..', path: '/', isParent: true, isGitRepo: false },
          { name: 'plan.docx', path: '/plan.docx', isParent: false, isGitRepo: false, isFile: true },
          { name: 'notes.md', path: '/notes.md', isParent: false, isGitRepo: false, isFile: true },
        ],
        'PLAN',
      ).map((entry) => entry.name),
    ).toEqual(['..', 'plan.docx'])
    expect(typedText({ name: 'a', sequence: 'a', ctrl: false, meta: false, option: false })).toBe('a')
    expect(typedText({ name: 'c', sequence: '\u0003', ctrl: true, meta: false, option: false })).toBeNull()
  })

  test('path completion finds files too, and only when asked', () => {
    expect(getPathCompletion(path.join(tmp, 'rep'), os.homedir(), { includeFiles: true })).toBe(
      path.join(tmp, 'report.pdf'),
    )
    expect(getPathCompletion(path.join(tmp, 'rep'))).toBeNull()
    expect(getPathCompletion(path.join(tmp, 'do'), os.homedir(), { includeFiles: true })).toBe(
      path.join(tmp, 'docs') + path.sep,
    )
  })

  async function mountPicker() {
    const setup = await createTestRenderer({ width: 80, height: 16, kittyKeyboard: true })
    const root = createRoot(setup.renderer)
    let closes = 0
    cleanup = () => {
      flushSync(() => root.unmount())
      setup.renderer.destroy()
    }
    flushSync(() =>
      root.render(<AttachFilePicker startDir={tmp} onClose={() => (closes += 1)} width={80} maxVisibleRows={8} />),
    )
    const settle = async () => {
      await setup.renderOnce()
      await new Promise((resolve) => setTimeout(resolve, 20))
      await setup.renderOnce()
    }
    await settle()
    return Object.assign(setup, { settle, closes: () => closes })
  }

  test('lists folders then files, and a click on a file attaches it', async () => {
    useEngineStore.getState().setStatus({ cwd: tmp } as never)
    const ui = await mountPicker()
    const frame = ui.captureCharFrame()
    expect(frame).toContain('Attach files')
    expect(frame).toContain(`docs${path.sep}`)
    expect(frame).toContain('notes.md')
    expect(frame.indexOf('docs')).toBeLessThan(frame.indexOf('notes.md'))
    expect(frame).toContain('[Done]')

    const at = locate(frame, 'report.pdf')!
    await ui.mockMouse.click(at.x, at.y)
    await ui.settle()
    const files = useChatStore.getState().pendingAttachments as PendingFileAttachment[]
    expect(files.map((file) => file.readWith)).toEqual(['read_document'])
    expect(ui.captureCharFrame()).toContain('1 attached')
  })

  test('the keyboard opens a folder, attaches with Enter and closes with Esc', async () => {
    useEngineStore.getState().setStatus({ cwd: tmp } as never)
    const ui = await mountPicker()
    await ui.mockInput.typeText('docs')
    await ui.settle()
    ui.mockInput.pressEnter()
    await ui.settle()
    expect(ui.captureCharFrame()).toContain('plan.docx')
    ui.mockInput.pressArrow('down')
    await ui.settle()
    ui.mockInput.pressEnter()
    await ui.settle()
    const files = useChatStore.getState().pendingAttachments as PendingFileAttachment[]
    expect(files.map((file) => path.basename(file.path))).toEqual(['plan.docx'])
    ui.mockInput.pressEscape()
    await ui.settle()
    expect(ui.closes()).toBe(1)
  })
})
