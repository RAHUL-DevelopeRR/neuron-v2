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
    // The rest of Microsoft's list: COM0/LPT0, superscript 1-3, the console names.
    for (const name of [
      'COM0',
      'lpt0',
      'COM\u00b9',
      'com\u00b2.txt',
      'LPT\u00b3',
      'CONIN$',
      'conout$',
      'CONOUT$.log',
    ]) {
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
  })

  test('a name too long for the file system or for Windows paths is refused', () => {
    const dir = tempDir()
    const tooLong = 'The name is too long. Choose a shorter one.'
    expect(newFolderNameError('x'.repeat(256), dir, 'linux')).toBe(tooLong)
    // ext4 counts UTF-8 bytes: 200 CJK characters are 600 bytes.
    const cjk = '\u4e2d'
    expect(newFolderNameError(cjk.repeat(200), dir, 'linux')).toBe(tooLong)
    expect(newFolderNameError(cjk.repeat(85), dir, 'linux')).toBeNull()
    expect(newFolderNameError(cjk.repeat(86), dir, 'linux')).toBe(tooLong)
    // On Windows the whole path must stay usable by tools without long paths.
    const deep = path.join(dir, 'p'.repeat(150))
    expect(newFolderNameError('n'.repeat(120), deep, 'win32')).toBe(
      'The path is too long for Windows. Use a shorter name.',
    )
    expect(newFolderNameError('n'.repeat(120), deep, 'linux')).toBeNull()
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

    const firstPath = path.join(home, 'Neuron', 'scratch-2026-11-03')
    const first = createScratchWorkspace(home, day)
    expect(first).toEqual({ ok: true, path: firstPath })
    expect(statSync(firstPath).isDirectory()).toBe(true)
    expect(isAtOrAboveHome(firstPath, home)).toBe(false)

    // Pressed again while it is unused (only the engine's .neuron in it): the
    // same folder, so no empty folders pile up and no new trust prompt.
    expect(createScratchWorkspace(home, day)).toEqual(first)
    mkdirSync(path.join(firstPath, '.neuron'))
    expect(createScratchWorkspace(home, day)).toEqual(first)

    // Once it has work in it, the next is a new folder, reused in turn.
    writeFileSync(path.join(firstPath, 'notes.md'), 'hi')
    const second = createScratchWorkspace(home, day)
    expect(second).toEqual({ ok: true, path: `${firstPath}-2` })
    expect(createScratchWorkspace(home, day)).toEqual(second)
    if (second.ok) expect(path.dirname(path.dirname(second.path))).toBe(home)
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

  test('Alt+P is a second key for no project (VS Code keeps Ctrl+P)', () => {
    expect(projectPickerKeyAction({ name: 'p', meta: true })).toBe('no_project')
    expect(projectPickerKeyAction({ name: 'p', option: true })).toBe('no_project')
    expect(projectPickerKeyAction({ name: 'p', meta: true, ctrl: true })).toBe('none')
    expect(projectPickerKeyAction({ name: 'p', meta: true, shift: true })).toBe('none')
    expect(projectPickerKeyAction({ name: 'n', meta: true })).toBe('none')
    expect(projectPickerKeyAction({ name: 'o', meta: true })).toBe('none')
  })
})
