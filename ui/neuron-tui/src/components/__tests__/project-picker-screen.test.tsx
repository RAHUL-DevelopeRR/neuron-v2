import { existsSync, mkdirSync, mkdtempSync, rmSync, statSync } from 'fs'
import os from 'os'
import path from 'path'

import { afterAll, afterEach, beforeAll, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import { initializeThemeStore } from '../../hooks/use-theme'
import { PICKER_SHORTCUTS_HINT, ProjectPickerScreen } from '../project-picker-screen'

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

async function mountPicker(width: number, height: number) {
  const project = tempDir()
  mkdirSync(path.join(project, 'alpha'))
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

  test('Ctrl+N names a new folder, Enter creates it and moves into it, Ctrl+O opens it', async () => {
    const ui = await mountPicker(80, 24)
    await ui.act(() => ui.mockInput.pressKey('n', { ctrl: true }))
    expect(ui.frame()).toContain('Enter creates the folder here, Esc cancels.')
    expect(ui.frame()).toContain('Name of the new folder...')

    await ui.act(() => ui.mockInput.typeText('demo'))
    await ui.act(() => ui.mockInput.pressEnter())

    const made = path.join(ui.project, 'demo')
    expect(statSync(made).isDirectory()).toBe(true)
    expect(ui.frame()).toContain('Created "demo". Press Ctrl+O to open it.')
    // The picker is now in the new folder: the bar shows it, and the filter is clear.
    const bar = ui.frame().split('\n').find((line) => line.includes('Open  │'))
    expect(bar).toContain(`${path.sep}demo`)
    expect(ui.frame()).toContain('Select project directory...')
    expect(ui.frame()).not.toContain('alpha')

    await ui.act(() => ui.mockInput.pressKey('o', { ctrl: true }))
    expect(ui.selected).toEqual([{ path: made, options: undefined }])
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
    expect(ui.frame()).toContain('Enter creates the folder here, Esc cancels.')
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

    // Ctrl+P does the same, in a folder of its own.
    await ui.act(() => ui.mockInput.pressKey('p', { ctrl: true }))
    expect(ui.selected).toHaveLength(2)
    expect(ui.selected[1]!.path).toBe(`${chosen.path}-2`)
  })
})
