// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the check
// is named `isAtOrAboveHome`, which the git-root banner uses too, and the
// picker's "New folder" and "No project" helpers are Neuron's own.
// See ui/neuron-tui/PORTING.md.
import { existsSync, mkdirSync } from 'fs'
import os from 'os'
import path from 'path'

/**
 * Whether `dir` is the home folder or a folder above it: too broad a place to
 * work in, which the engine refuses.
 */
export function isAtOrAboveHome(dir: string, homeDir: string = os.homedir()): boolean {
  const relativeToHome = path.relative(dir, homeDir)
  return (
    relativeToHome === '' ||
    (!relativeToHome.startsWith('..') && !path.isAbsolute(relativeToHome))
  )
}

/**
 * Whether the UI starts by asking for a project: from the home folder or above
 * it, unless it was opened to continue a session (`neuron --resume`), which
 * belongs to the folder it was started in and would be lost by moving.
 */
export function shouldShowProjectPicker(
  startCwd: string,
  homeDir: string,
  resumingSession = false,
): boolean {
  return !resumingSession && isAtOrAboveHome(startCwd, homeDir)
}

/** Names Windows keeps for devices, with or without an extension (`nul.txt`). */
const WINDOWS_RESERVED_NAMES = new Set([
  'CON',
  'PRN',
  'AUX',
  'NUL',
  ...Array.from({ length: 9 }, (_, i) => `COM${i + 1}`),
  ...Array.from({ length: 9 }, (_, i) => `LPT${i + 1}`),
])

/** Characters Windows does not allow in a file name (besides the separators). */
const WINDOWS_INVALID_CHARS = /[<>:"|?*\u0000-\u001f]/

/** A name cut to fit a one-line message. */
export function shortName(name: string, max = 24): string {
  return name.length <= max ? name : `${name.slice(0, max - 3)}...`
}

/** The longest folder name the common file systems take. */
const MAX_FOLDER_NAME_LENGTH = 255

/**
 * Why `name` can't be a new folder in `parentDir`, or `null` when it can. The
 * Windows rules apply everywhere, so a project made here can be checked out
 * on Windows too.
 */
export function newFolderNameError(name: string, parentDir: string): string | null {
  if (name.trim() === '') return 'Type a name for the new folder.'
  if (name.includes('/') || name.includes('\\')) {
    return 'A folder name cannot contain / or \\.'
  }
  if (name === '.' || name === '..') return `"${name}" is not a folder name.`
  if (WINDOWS_INVALID_CHARS.test(name)) {
    return 'Remove < > : " | ? * and control characters.'
  }
  if (name.endsWith('.') || name.endsWith(' ')) {
    return 'A folder name cannot end with a dot or a space.'
  }
  if (name.startsWith(' ')) return 'A folder name cannot start with a space.'
  const stem = name.split('.')[0]!.trimEnd().toUpperCase()
  if (WINDOWS_RESERVED_NAMES.has(stem)) {
    return `"${shortName(name, 12)}" is reserved by Windows. Choose another name.`
  }
  if (name.length > MAX_FOLDER_NAME_LENGTH) {
    return `A folder name can be at most ${MAX_FOLDER_NAME_LENGTH} characters.`
  }
  if (existsSync(path.join(parentDir, name))) {
    return `"${shortName(name)}" already exists here.`
  }
  return null
}

export type CreateFolderResult =
  | { ok: true; path: string }
  | { ok: false; error: string }

/** Creates the folder `name` inside `parentDir`, after {@link newFolderNameError}. */
export function createNewFolder(name: string, parentDir: string): CreateFolderResult {
  const error = newFolderNameError(name, parentDir)
  if (error) return { ok: false, error }
  const folder = path.join(parentDir, name)
  try {
    // Not recursive: an existing folder (made since the check) is an error.
    mkdirSync(folder)
    return { ok: true, path: folder }
  } catch (err) {
    return { ok: false, error: `Could not create the folder: ${describeFsError(err)}` }
  }
}

/** The folder under the home folder that holds the scratch workspaces. */
export const SCRATCH_PARENT = 'Neuron'

/** `scratch-2026-11-13`, from the local date. */
export function scratchFolderName(now: Date = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, '0')
  return `scratch-${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`
}

/**
 * Where "No project" works: a new, empty folder `~/Neuron/scratch-<date>`
 * (`-2`, `-3`... when that day's exists), so the tools have a folder of their
 * own and never the home folder itself, which the engine refuses as too broad.
 */
export function createScratchWorkspace(
  homeDir: string = os.homedir(),
  now: Date = new Date(),
): CreateFolderResult {
  const parent = path.join(homeDir, SCRATCH_PARENT)
  const base = scratchFolderName(now)
  try {
    mkdirSync(parent, { recursive: true })
    for (let n = 1; n <= 100; n++) {
      const folder = path.join(parent, n === 1 ? base : `${base}-${n}`)
      try {
        mkdirSync(folder)
      } catch (err) {
        if ((err as NodeJS.ErrnoException)?.code === 'EEXIST') continue
        throw err
      }
      if (isAtOrAboveHome(folder, homeDir)) {
        return { ok: false, error: 'The scratch folder would be the home folder.' }
      }
      return { ok: true, path: folder }
    }
    return { ok: false, error: 'Too many scratch folders for today.' }
  } catch (err) {
    return { ok: false, error: `Could not create a scratch folder: ${describeFsError(err)}` }
  }
}

function describeFsError(err: unknown): string {
  const code = (err as NodeJS.ErrnoException)?.code
  if (code === 'EACCES' || code === 'EPERM') return 'permission denied.'
  if (code === 'EEXIST') return 'it already exists.'
  if (code === 'ENOENT') return 'the folder it goes in no longer exists.'
  if (code === 'ENOSPC') return 'the disk is full.'
  if (code === 'EROFS') return 'the disk is read-only.'
  return err instanceof Error ? err.message : String(err)
}

/** What a key does on the project picker, besides typing and moving in the list. */
export type ProjectPickerKeyAction = 'new_folder' | 'open' | 'no_project' | 'none'

/**
 * The picker's shortcuts. They take Ctrl, because every plain key types into
 * the folder filter.
 */
export function projectPickerKeyAction(key: {
  name?: string
  ctrl?: boolean
  meta?: boolean
  shift?: boolean
}): ProjectPickerKeyAction {
  if (!key.ctrl || key.meta || key.shift) return 'none'
  switch (key.name) {
    case 'n':
      return 'new_folder'
    case 'o':
      return 'open'
    case 'p':
      return 'no_project'
    default:
      return 'none'
  }
}
