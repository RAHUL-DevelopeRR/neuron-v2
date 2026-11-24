import { existsSync, mkdirSync, mkdtempSync, rmSync, statSync, writeFileSync } from 'fs'
import os from 'os'
import path from 'path'

import { afterAll, afterEach, beforeAll, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import { initializeThemeStore } from '../../hooks/use-theme'
import {
  NAMING_HINT,
  PICKER_SHORTCUTS_HINT,
  ProjectPickerScreen,
  selectionHint,
} from '../project-picker-screen'

import type { ProjectSelectOptions } from '../project-picker-screen'

let cleanupRenderer: (() => void) | undefined
const temps: string[] = []
const savedConfigDir = process.env.NEURON_TUI_CONFIG_DIR

const tempDir = () => {
  const dir = mkdtempSync(path.join(os.tmpdir(), 'neuron-picker-ui-'))
  temps.push(dir)
  return dir
}

beforeAll(() => {
  // No recent projects from the machine running the tests.
  process.env.NEURON_TUI_CONFIG_DIR = tempDir()
  initializeThemeStore()
})

afterEach(() => {
  cleanupRenderer?.()
  cleanupRenderer = undefined
})

afterAll(() => {
  if (savedConfigDir === undefined) delete process.env.NEURON_TUI_CONFIG_DIR
  else process.env.NEURON_TUI_CONFIG_DIR = savedConfigDir
  for (const dir of temps.splice(0)) rmSync(dir, { recursive: true, force: true })
})

type Selection = { path: string; options?: ProjectSelectOptions }

async function mountPicker(
  width: number,
  height: number,
  { folders = ['alpha'] }: { folders?: string[] } = {},
) {
  const project = tempDir()
  for (const folder of folders) mkdirSync(path.join(project, folder))
  const home = tempDir()
  const selected: Selection[] = []

  const setup = await createTestRenderer({ width, height, kittyKeyboard: true })
  const root = createRoot(setup.renderer)
  cleanupRenderer = () => {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
  flushSync(() =>
    root.render(
      <ProjectPickerScreen
        initialPath={project}
        homeDir={home}
        onSelectProject={(p, options) => selected.push({ path: p, options })}
      />,
    ),
  )
  const settle = async () => {
    await setup.renderOnce()
    await new Promise((resolve) => setTimeout(resolve, 20))
    await setup.renderOnce()
  }
  await settle()

  return Object.assign(setup, {
    project,
    home,
    selected,
    frame: () => setup.captureCharFrame(),
    async act(fn: () => void | Promise<void>) {
      await fn()
      await settle()
    },
    /** Clicks the middle of the button labelled `label`. */
    async clickButton(label: string) {
      const lines = setup.captureCharFrame().split('\n')
      const y = lines.findIndex((line) => line.includes(`${label} │`))
      if (y < 0) throw new Error(`no "${label}" button on screen`)
      const x = lines[y]!.indexOf(label) + Math.floor(label.length / 2)
      await setup.mockMouse.click(x, y)
      await settle()
    },
    /** Where the row showing `text` is (the first line that has it). */
    rowAt(text: string) {
      const lines = setup.captureCharFrame().split('\n')
      const y = lines.findIndex((line) => line.includes(text))
      if (y < 0) throw new Error(`no row with "${text}" on screen`)
      return { x: lines[y]!.indexOf(text) + 1, y }
    },
    async clickRow(text: string) {
      const { x, y } = this.rowAt(text)
      await setup.mockMouse.click(x, y)
      await settle()
    },
    async doubleClickRow(text: string) {
      const { x, y } = this.rowAt(text)
      await setup.mockMouse.doubleClick(x, y)
      await settle()
    },
    /** The bottom bar: the path Open opens, and the buttons. */
    bar: () =>
      setup
        .captureCharFrame()
        .split('\n')
        .find((line) => line.includes('Open  │')) ?? '',
  })
}

describe('the project picker', () => {
  for (const [width, height] of [
    [80, 24],
    [120, 40],
  ] as const) {
    test(`shows New folder, No project and the shortcuts at ${width}x${height}`, async () => {
      const ui = await mountPicker(width, height)
      const frame = ui.frame()
      expect(frame).toContain(PICKER_SHORTCUTS_HINT)
      // The three buttons, whole, on one line beside the path.
      const buttons = frame.split('\n').find((line) => line.includes('Open  │'))
      expect(buttons).toContain('│ New folder │')
      expect(buttons).toContain('│ No project │')
      expect(buttons).toContain(path.basename(ui.project).slice(-12))
      expect(frame).toContain('alpha')
      for (const line of frame.split('\n')) expect(line.length).toBeLessThanOrEqual(width)
    })
  }

  test('Ctrl+N names a new folder, Enter creates it in the folder browsed and opens it', async () => {
    const ui = await mountPicker(80, 24)
    await ui.act(() => ui.mockInput.pressKey('n', { ctrl: true }))
    expect(ui.frame()).toContain(NAMING_HINT)
    expect(ui.frame()).toContain('Name of the new folder...')

    await ui.act(() => ui.mockInput.typeText('demo'))
    await ui.act(() => ui.mockInput.pressEnter())

    const made = path.join(ui.project, 'demo')
    expect(statSync(made).isDirectory()).toBe(true)
    // Opened as the project at once, not left for Ctrl+O.
    expect(ui.selected).toEqual([{ path: made, options: undefined }])
    expect(ui.frame()).toContain('Created "demo". Opening it...')
  })

  test('a name with a separator or a bad character is refused and opens nothing', async () => {
    const ui = await mountPicker(80, 24)
    await ui.act(() => ui.mockInput.pressKey('n', { ctrl: true }))
    await ui.act(() => ui.mockInput.typeText('a/b'))
    await ui.act(() => ui.mockInput.pressEnter())
    expect(ui.frame()).toContain('A folder name cannot contain /')
    expect(existsSync(path.join(ui.project, 'a'))).toBe(false)
    expect(ui.selected).toEqual([])
  })

  test('a bad name says why and keeps the name; Esc cancels', async () => {
    const ui = await mountPicker(80, 24)
    await ui.act(() => ui.mockInput.pressKey('n', { ctrl: true }))
    await ui.act(() => ui.mockInput.typeText('CON'))
    await ui.act(() => ui.mockInput.pressEnter())
    expect(ui.frame()).toContain('"CON" is reserved by Windows. Choose another name.')
    expect(existsSync(path.join(ui.project, 'CON'))).toBe(false)

    await ui.act(() => ui.mockInput.typeText('X'))
    expect(ui.frame()).not.toContain('reserved by Windows')

    await ui.act(() => ui.mockInput.pressEscape())
    expect(ui.frame()).toContain(PICKER_SHORTCUTS_HINT)
    expect(ui.frame()).toContain('Select project directory...')
    expect(ui.selected).toEqual([])
  })

  test('an existing name is refused', async () => {
    const ui = await mountPicker(80, 24)
    await ui.act(() => ui.mockInput.pressKey('n', { ctrl: true }))
    await ui.act(() => ui.mockInput.typeText('alpha'))
    await ui.act(() => ui.mockInput.pressEnter())
    expect(ui.frame()).toContain('"alpha" already exists here.')
  })

  test('the New folder button starts naming too', async () => {
    const ui = await mountPicker(80, 24)
    await ui.clickButton('New folder')
    expect(ui.frame()).toContain(NAMING_HINT)
  })

  test('No project opens a new scratch folder under ~/Neuron, never home itself', async () => {
    const ui = await mountPicker(80, 24)
    await ui.clickButton('No project')
    expect(ui.selected).toHaveLength(1)
    const chosen = ui.selected[0]!
    expect(chosen.options).toEqual({ scratch: true })
    expect(path.dirname(chosen.path)).toBe(path.join(ui.home, 'Neuron'))
    expect(path.basename(chosen.path)).toMatch(/^scratch-\d{4}-\d{2}-\d{2}$/)
    expect(statSync(chosen.path).isDirectory()).toBe(true)

    // Ctrl+P does the same; the unused folder is used again, not a new one.
    await ui.act(() => ui.mockInput.pressKey('p', { ctrl: true }))
    expect(ui.selected).toHaveLength(2)
    expect(ui.selected[1]!.path).toBe(chosen.path)

    // Once it has files, Alt+P (for VS Code, which keeps Ctrl+P) makes the next.
    writeFileSync(path.join(chosen.path, 'notes.md'), 'hi')
    await ui.act(() => ui.mockInput.pressKey('p', { meta: true }))
    expect(ui.selected).toHaveLength(3)
    expect(ui.selected[2]!.path).toBe(`${chosen.path}-2`)
  })

  test('clicking Open while naming creates the named folder and opens it', async () => {
    const ui = await mountPicker(80, 24)
    await ui.act(() => ui.mockInput.pressKey('n', { ctrl: true }))
    await ui.act(() => ui.mockInput.typeText('demo'))
    await ui.clickButton('Open ')

    const made = path.join(ui.project, 'demo')
    expect(statSync(made).isDirectory()).toBe(true)
    // Not the folder it was made in, and the typed name was not dropped.
    expect(ui.selected).toEqual([{ path: made, options: undefined }])
  })

  test('clicking Open with a bad name opens nothing and says why', async () => {
    const ui = await mountPicker(80, 24)
    await ui.act(() => ui.mockInput.pressKey('n', { ctrl: true }))
    await ui.act(() => ui.mockInput.typeText('alpha'))
    await ui.clickButton('Open ')
    expect(ui.selected).toEqual([])
    expect(ui.frame()).toContain('"alpha" already exists here.')
  })

  test('clicking a folder in the list while naming cancels the name and selects it', async () => {
    const ui = await mountPicker(80, 24)
    await ui.act(() => ui.mockInput.pressKey('n', { ctrl: true }))
    await ui.act(() => ui.mockInput.typeText('demo'))
    await ui.clickRow('alpha')

    expect(ui.frame()).not.toContain(NAMING_HINT)
    expect(ui.frame()).toContain('Select project directory...')
    // Selected, not gone into: alpha is still a row of the list.
    expect(ui.frame()).toContain(selectionHint({ kind: 'row', path: path.join(ui.project, 'alpha') }))
    expect(ui.frame()).toContain('📁   alpha')
    expect(ui.bar()).toContain(`${path.sep}alpha`)
    expect(existsSync(path.join(ui.project, 'demo'))).toBe(false)
    expect(ui.selected).toEqual([])
  })
})

describe('the project picker with the mouse', () => {
  test('one click selects a folder and Open opens it', async () => {
    const ui = await mountPicker(80, 24, { folders: ['alpha', 'beta'] })
    await ui.clickRow('beta')
    const beta = path.join(ui.project, 'beta')
    // Still in the project folder (both rows listed); the bar shows what Open opens.
    expect(ui.frame()).toContain('📁   alpha')
    expect(ui.frame()).toContain(selectionHint({ kind: 'row', path: beta }))
    expect(ui.bar()).toContain(`${path.sep}beta`)
    expect(ui.selected).toEqual([])

    await ui.clickButton('Open ')
    expect(ui.selected).toEqual([{ path: beta, options: undefined }])
  })

  test('a double click goes into a folder, as Enter does', async () => {
    const ui = await mountPicker(80, 24, { folders: ['alpha', 'beta'] })
    mkdirSync(path.join(ui.project, 'beta', 'inner'))
    await ui.doubleClickRow('beta')
    expect(ui.frame()).toContain('inner')
    expect(ui.frame()).not.toContain('alpha')
    expect(ui.bar()).toContain(`${path.sep}beta`)
    // Nothing is selected in the new folder: the shortcuts are back, Open opens beta.
    expect(ui.frame()).toContain(PICKER_SHORTCUTS_HINT)
    expect(ui.selected).toEqual([])
    await ui.clickButton('Open ')
    expect(ui.selected).toEqual([{ path: path.join(ui.project, 'beta'), options: undefined }])
  })

  test('two single clicks on different rows only move the selection', async () => {
    const ui = await mountPicker(80, 24, { folders: ['alpha', 'beta'] })
    await ui.clickRow('alpha')
    await ui.clickRow('beta')
    expect(ui.frame()).toContain('📁   alpha')
    expect(ui.bar()).toContain(`${path.sep}beta`)
  })

  test('the parent row can be selected and opened', async () => {
    const ui = await mountPicker(80, 24)
    await ui.clickRow(' ..  ')
    expect(ui.bar()).toContain(path.basename(path.dirname(ui.project)).slice(-8))
    await ui.clickButton('Open ')
    expect(ui.selected).toEqual([{ path: path.dirname(ui.project), options: undefined }])
  })

  test('Ctrl+O opens the clicked row; any other key gives the keyboard back', async () => {
    const ui = await mountPicker(80, 24, { folders: ['alpha', 'beta'] })
    await ui.clickRow('beta')
    await ui.act(() => ui.mockInput.pressArrow('up'))
    // The selection is dropped: Open is this folder again.
    expect(ui.frame()).toContain(PICKER_SHORTCUTS_HINT)
    expect(ui.bar()).not.toContain(`${path.sep}beta`)
    await ui.act(() => ui.mockInput.pressKey('o', { ctrl: true }))
    expect(ui.selected).toEqual([{ path: ui.project, options: undefined }])

    await ui.clickRow('beta')
    await ui.act(() => ui.mockInput.pressKey('o', { ctrl: true }))
    expect(ui.selected[1]).toEqual({ path: path.join(ui.project, 'beta'), options: undefined })
  })

  test('the scroll wheel scrolls the folder list', async () => {
    const folders = Array.from({ length: 30 }, (_, i) => `folder-${String(i).padStart(2, '0')}`)
    const ui = await mountPicker(80, 24, { folders })
    expect(ui.frame()).not.toContain('folder-29')
    const { x, y } = ui.rowAt('folder-00')
    for (let i = 0; i < 10; i++) await ui.act(() => ui.mockMouse.scroll(x, y, 'down'))
    expect(ui.frame()).not.toContain('folder-00')
    expect(ui.frame()).toMatch(/folder-(1\d|2\d)/)
  })

  test('the row under the pointer is highlighted', async () => {
    const ui = await mountPicker(80, 24, { folders: ['alpha', 'beta'] })
    const background = (text: string) => {
      const { y } = ui.rowAt(text)
      const line = ui.captureSpans().lines[y]!
      const span = line.spans.find((s) => s.text.includes(text))!
      return span.bg.toString()
    }
    const before = background('beta')
    const { x, y } = ui.rowAt('beta')
    await ui.act(() => ui.mockMouse.moveTo(x, y))
    expect(background('beta')).not.toBe(before)
  })
})

describe('the project picker recents with the mouse', () => {
  const recentsFile = () => path.join(process.env.NEURON_TUI_CONFIG_DIR!, 'recent-projects.json')
  afterEach(() => rmSync(recentsFile(), { force: true }))

  test('one click selects a recent project, a double click opens it', async () => {
    const recent = tempDir()
    writeFileSync(recentsFile(), JSON.stringify([{ path: recent, lastOpened: Date.now() }]))
    const ui = await mountPicker(120, 40)
    const shown = path.basename(recent)
    expect(ui.frame()).toContain('Recent:')

    await ui.clickRow('[1]')
    expect(ui.selected).toEqual([])
    expect(ui.frame()).toContain(selectionHint({ kind: 'recent', path: recent }))
    expect(ui.bar()).toContain(shown.slice(-10))
    await ui.clickButton('Open ')
    expect(ui.selected).toEqual([{ path: recent, options: undefined }])

    // Open ended the sequence: the click before it does not pair with this one.
    await ui.clickRow('[1]')
    expect(ui.selected).toHaveLength(1)
    expect(ui.frame()).toContain(selectionHint({ kind: 'recent', path: recent }))
  })

  test('a double click opens a recent project with no click before it', async () => {
    const recent = tempDir()
    writeFileSync(recentsFile(), JSON.stringify([{ path: recent, lastOpened: Date.now() }]))
    const ui = await mountPicker(120, 40)

    await ui.doubleClickRow('[1]')
    expect(ui.selected).toEqual([{ path: recent, options: undefined }])
  })
})
