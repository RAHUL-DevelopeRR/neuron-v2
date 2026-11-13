import { mkdirSync, mkdtempSync, rmSync, statSync, writeFileSync } from 'fs'
import os from 'os'
import path from 'path'

import { afterEach, describe, test, expect } from 'bun:test'

import {
  createNewFolder,
  createScratchWorkspace,
  isAtOrAboveHome,
  newFolderNameError,
  projectPickerKeyAction,
  scratchFolderName,
  shouldShowProjectPicker,
} from '../../utils/project-picker'

describe('cli/utils/project-picker', () => {
  test('returns true when start cwd is home directory', () => {
    const root = path.parse(process.cwd()).root
    const homeDir = path.join(root, 'home', 'test-user')

    expect(shouldShowProjectPicker(homeDir, homeDir)).toBe(true)
  })

  test('returns true when start cwd is a parent of home directory', () => {
    const root = path.parse(process.cwd()).root
    const homeDir = path.join(root, 'home', 'test-user')
    const parentDir = path.dirname(homeDir)

    expect(shouldShowProjectPicker(parentDir, homeDir)).toBe(true)
    expect(shouldShowProjectPicker(root, homeDir)).toBe(true)
  })

  test('returns false when start cwd is a child of home directory', () => {
    const root = path.parse(process.cwd()).root
    const homeDir = path.join(root, 'home', 'test-user')
    const childDir = path.join(homeDir, 'projects')

    expect(shouldShowProjectPicker(childDir, homeDir)).toBe(false)
  })

  test('returns false when start cwd is a sibling of home directory', () => {
    const root = path.parse(process.cwd()).root
    const homeDir = path.join(root, 'home', 'test-user')
    const siblingDir = path.join(root, 'home', 'other-user')

    expect(shouldShowProjectPicker(siblingDir, homeDir)).toBe(false)
  })

  test('a resumed session starts in its own folder, without the picker', () => {
    // `neuron --resume X` from home: the picker restarted the engine in the
    // chosen folder without the session, which was then silently not continued.
    const root = path.parse(process.cwd()).root
    const homeDir = path.join(root, 'home', 'test-user')
    expect(shouldShowProjectPicker(homeDir, homeDir, true)).toBe(false)
    expect(shouldShowProjectPicker(root, homeDir, true)).toBe(false)
    expect(shouldShowProjectPicker(homeDir, homeDir, false)).toBe(true)
  })

  test('the git-root banner never offers the home folder or a folder above it', () => {
    // A home folder that is itself a git repo (dotfiles) made the banner offer
    // it, and the engine then refused it as too broad.
    const root = path.parse(process.cwd()).root
    const homeDir = path.join(root, 'home', 'test-user')
    expect(isAtOrAboveHome(homeDir, homeDir)).toBe(true)
    expect(isAtOrAboveHome(root, homeDir)).toBe(true)
    expect(isAtOrAboveHome(path.join(homeDir, 'code', 'app'), homeDir)).toBe(false)
  })
})

describe('the project picker: new folder and no project', () => {
  const temps: string[] = []
  const tempDir = () => {
    const dir = mkdtempSync(path.join(os.tmpdir(), 'neuron-picker-'))
    temps.push(dir)
    return dir
  }
  afterEach(() => {
    for (const dir of temps.splice(0)) rmSync(dir, { recursive: true, force: true })
  })

  test('a plain name is fine; an empty one asks for a name', () => {
    const dir = tempDir()
    expect(newFolderNameError('my-app', dir)).toBeNull()
    expect(newFolderNameError('My App 2.0', dir)).toBeNull()
    expect(newFolderNameError('', dir)).toBe('Type a name for the new folder.')
    expect(newFolderNameError('   ', dir)).toBe('Type a name for the new folder.')
  })

  test('one folder at a time: no path separators, no . or ..', () => {
    const dir = tempDir()
    expect(newFolderNameError('a/b', dir)).toContain('cannot contain / or \\')
    expect(newFolderNameError('a\\b', dir)).toContain('cannot contain / or \\')
    expect(newFolderNameError('..', dir)).toContain('is not a folder name')
    expect(newFolderNameError('.', dir)).toContain('is not a folder name')
  })

  test('names Windows refuses are refused everywhere', () => {
    const dir = tempDir()
    for (const name of ['CON', 'nul', 'Com1', 'lpt9', 'aux.txt', 'PRN.tar.gz']) {
      expect(newFolderNameError(name, dir)).toContain('reserved by Windows')
    }
    // Only the whole stem is reserved.
    expect(newFolderNameError('console', dir)).toBeNull()
    expect(newFolderNameError('com10', dir)).toBeNull()
    for (const name of ['a<b', 'a>b', 'a:b', 'a"b', 'a|b', 'a?b', 'a*b', 'a\tb']) {
      expect(newFolderNameError(name, dir)).toContain('Remove < > :')
    }
    expect(newFolderNameError('app.', dir)).toBe('A folder name cannot end with a dot or a space.')
    expect(newFolderNameError('app ', dir)).toBe('A folder name cannot end with a dot or a space.')
    expect(newFolderNameError(' app', dir)).toBe('A folder name cannot start with a space.')
    expect(newFolderNameError('x'.repeat(256), dir)).toContain('at most 255')
  })

  test('an existing folder or file is not created over', () => {
    const dir = tempDir()
    mkdirSync(path.join(dir, 'taken'))
    writeFileSync(path.join(dir, 'notes.txt'), '')
    expect(newFolderNameError('taken', dir)).toBe('"taken" already exists here.')
    expect(newFolderNameError('notes.txt', dir)).toBe('"notes.txt" already exists here.')
    // A long name is cut to fit the one-line message.
    const long = 'a-very-long-folder-name-that-goes-on'
    mkdirSync(path.join(dir, long))
    expect(newFolderNameError(long, dir)).toBe('"a-very-long-folder-na..." already exists here.')
  })

  test('createNewFolder makes the folder inside the current one', () => {
    const dir = tempDir()
    const made = createNewFolder('fresh', dir)
    expect(made).toEqual({ ok: true, path: path.join(dir, 'fresh') })
    expect(statSync(path.join(dir, 'fresh')).isDirectory()).toBe(true)
    expect(createNewFolder('fresh', dir)).toEqual({ ok: false, error: '"fresh" already exists here.' })
    expect(createNewFolder('bad/name', dir).ok).toBe(false)
  })

  test('a folder that went away says so instead of throwing', () => {
    const dir = tempDir()
    const gone = path.join(dir, 'gone')
    const made = createNewFolder('child', gone)
    expect(made.ok).toBe(false)
    if (!made.ok) expect(made.error).toStartWith('Could not create the folder:')
  })

  test('no project: a new scratch folder under ~/Neuron, never the home folder', () => {
    const home = tempDir()
    const day = new Date(2026, 10, 3, 23, 59)
    expect(scratchFolderName(day)).toBe('scratch-2026-11-03')

    const first = createScratchWorkspace(home, day)
    expect(first).toEqual({ ok: true, path: path.join(home, 'Neuron', 'scratch-2026-11-03') })
    // A second one the same day is new and empty too.
    const second = createScratchWorkspace(home, day)
    expect(second).toEqual({ ok: true, path: path.join(home, 'Neuron', 'scratch-2026-11-03-2') })
    if (first.ok && second.ok) {
      expect(statSync(first.path).isDirectory()).toBe(true)
      expect(isAtOrAboveHome(first.path, home)).toBe(false)
      expect(path.dirname(path.dirname(second.path))).toBe(home)
    }
  })

  test('no project: a file in the way is an error, not a crash', () => {
    const home = tempDir()
    writeFileSync(path.join(home, 'Neuron'), 'not a folder')
    const result = createScratchWorkspace(home, new Date(2026, 0, 1))
    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toStartWith('Could not create a scratch folder:')
  })

  test('the shortcuts take Ctrl, since plain keys type into the filter', () => {
    expect(projectPickerKeyAction({ name: 'n', ctrl: true })).toBe('new_folder')
    expect(projectPickerKeyAction({ name: 'o', ctrl: true })).toBe('open')
    expect(projectPickerKeyAction({ name: 'p', ctrl: true })).toBe('no_project')
    expect(projectPickerKeyAction({ name: 'n' })).toBe('none')
    expect(projectPickerKeyAction({ name: 'n', ctrl: true, shift: true })).toBe('none')
    expect(projectPickerKeyAction({ name: 'n', ctrl: true, meta: true })).toBe('none')
    expect(projectPickerKeyAction({ name: 'c', ctrl: true })).toBe('none')
  })
})
