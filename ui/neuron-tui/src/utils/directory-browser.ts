import { readdirSync, statSync } from 'fs'
import { access } from 'fs/promises'
import path from 'path'

export type DirectoryEntry = {
  name: string
  path: string
  isParent: boolean
  isGitRepo: boolean
  /** A file, listed only when files were asked for (the attach browser). */
  isFile?: boolean
  /** Another drive's root, listed at a Windows drive root (`D:\`). */
  isDrive?: boolean
}

export interface GetDirectoriesOptions {
  /** List files too, after the folders (hidden files stay hidden). */
  includeFiles?: boolean
  /** The platform the rows are for (tests); defaults to this one. */
  platform?: NodeJS.Platform
  /**
   * The drive roots found so far (from {@link probeDrives}); at a drive root
   * the others are listed. Nothing here touches a drive: a disconnected
   * network drive can take tens of seconds to answer, so drives are only
   * ever probed off the render path.
   */
  drives?: readonly string[]
  /** Whether a drive root such as `D:\` exists (tests only: it runs synchronously). */
  driveExists?: (root: string) => boolean
}

/** The attach browser's name for the same options. */
export type DirectoryListingOptions = GetDirectoriesOptions

const DRIVE_ROOT = /^[A-Za-z]:[\\/]?$/

/** Whether `dir` is a Windows drive's root, such as `C:\`. */
export function isWindowsDriveRoot(dir: string): boolean {
  return DRIVE_ROOT.test(dir)
}

/** The drive roots probed, `C:\` to `Z:\`: A and B are floppy letters, slow to probe on some machines. */
const DRIVE_ROOTS = Array.from(
  { length: 24 },
  (_, i) => `${String.fromCharCode('C'.charCodeAt(0) + i)}:\\`,
)

/**
 * The other drives' roots, `C:\` to `Z:\`, less the drive `dir` is on, by
 * asking `driveExists` about each one in turn (synchronously: tests only).
 */
export function listOtherDrives(dir: string, driveExists: (root: string) => boolean): string[] {
  return otherDrives(
    dir,
    DRIVE_ROOTS.filter((root) => driveExists(root)),
  )
}

/** `drives` less the drive `dir` is on. */
export function otherDrives(dir: string, drives: readonly string[]): string[] {
  const current = dir.slice(0, 1).toUpperCase()
  return drives.filter((root) => root.slice(0, 1).toUpperCase() !== current)
}

/** How long one drive may take to answer before it is left out. */
export const DRIVE_PROBE_TIMEOUT_MS = 1500

export interface ProbeDrivesOptions {
  /** Whether a drive root answers (tests); defaults to an async access check. */
  exists?: (root: string) => Promise<boolean>
  /** How long each drive may take; one that takes longer is left out. */
  timeoutMs?: number
}

/**
 * The drive roots, `C:\` to `Z:\`, that answer: all probed at once, off the
 * JS thread, each given {@link DRIVE_PROBE_TIMEOUT_MS}. A drive that fails or
 * does not answer in time (a disconnected network share) is left out, so the
 * whole probe never takes much longer than one timeout. Never rejects.
 */
export async function probeDrives({
  exists = driveAnswers,
  timeoutMs = DRIVE_PROBE_TIMEOUT_MS,
}: ProbeDrivesOptions = {}): Promise<string[]> {
  const roots = DRIVE_ROOTS
  const answers = await Promise.all(
    roots.map((root) =>
      withTimeout(
        Promise.resolve().then(() => exists(root)),
        timeoutMs,
        false,
      ),
    ),
  )
  return roots.filter((_, index) => answers[index] === true)
}

let sessionDrives: Promise<string[]> | null = null

/** {@link probeDrives}, once per session: later calls share the first answer. */
export function sessionDriveRoots(): Promise<string[]> {
  sessionDrives ??= probeDrives()
  return sessionDrives
}

async function driveAnswers(root: string): Promise<boolean> {
  try {
    await access(root)
    return true
  } catch {
    return false
  }
}

/** `promise`'s value, or `fallback` when it rejects or takes longer than `ms`. */
function withTimeout<T>(promise: Promise<T>, ms: number, fallback: T): Promise<T> {
  return new Promise((resolve) => {
    // Not unref'd: Bun never fires an unref'd timer when nothing else keeps
    // the loop alive, so the probe would never answer. It lasts `ms` at most,
    // and quitting exits the process outright (exitCliCleanly).
    const timer = setTimeout(() => resolve(fallback), ms)
    promise.then(
      (value) => {
        clearTimeout(timer)
        resolve(value)
      },
      () => {
        clearTimeout(timer)
        resolve(fallback)
      },
    )
  })
}

/**
 * Get directory entries for a given path, including parent directory option.
 * Skips hidden directories (those starting with '.'). At a Windows drive's
 * root, where there is no parent, the other drives in `options.drives` are
 * listed first, so the mouse can reach them as the keyboard reaches a typed
 * `D:\`. No drive is probed here (see {@link probeDrives}). With
 * `includeFiles`, the files follow the folders.
 */
export function getDirectories(
  dirPath: string,
  options: GetDirectoriesOptions = {},
): DirectoryEntry[] {
  const entries: DirectoryEntry[] = []
  const files: DirectoryEntry[] = []

  // Add parent directory option if not at filesystem root
  const parentDir = path.dirname(dirPath)
  if (parentDir !== dirPath) {
    entries.push({
      name: '..',
      path: parentDir,
      isParent: true,
      isGitRepo: false,
    })
  }

  try {
    const items = readdirSync(dirPath)
    for (const item of items) {
      // Skip hidden directories
      if (item.startsWith('.')) continue

      const fullPath = path.join(dirPath, item)
      try {
        const stat = statSync(fullPath)
        if (stat.isDirectory()) {
          entries.push({
            name: item,
            path: fullPath,
            isParent: false,
            isGitRepo: hasGitDirectory(fullPath),
          })
        } else if (options.includeFiles && stat.isFile()) {
          files.push({
            name: item,
            path: fullPath,
            isParent: false,
            isGitRepo: false,
            isFile: true,
          })
        }
      } catch {
        // Skip items we can't stat
      }
    }
  } catch {
    // If we can't read the directory, just return parent option
  }

  // Sort non-parent entries alphabetically (case-insensitive)
  const parentEntry = entries.find((e) => e.isParent)
  const childEntries = entries.filter((e) => !e.isParent)
  const byName = (a: DirectoryEntry, b: DirectoryEntry) =>
    a.name.toLowerCase().localeCompare(b.name.toLowerCase())
  childEntries.sort(byName)
  files.sort(byName)

  const platform = options.platform ?? process.platform
  const atDriveRoot = platform === 'win32' && isWindowsDriveRoot(dirPath)
  const driveRoots = !atDriveRoot
    ? []
    : options.drives
      ? otherDrives(dirPath, options.drives)
      : options.driveExists
        ? listOtherDrives(dirPath, options.driveExists)
        : []
  const drives: DirectoryEntry[] = driveRoots.map((root) => ({
    name: root,
    path: root,
    isParent: false,
    isGitRepo: false,
    isDrive: true,
  }))

  const listed = [...drives, ...childEntries, ...files]
  return parentEntry ? [parentEntry, ...listed] : listed
}

/**
 * Check if a directory contains a .git subdirectory.
 */
export function hasGitDirectory(dirPath: string): boolean {
  try {
    const gitPath = path.join(dirPath, '.git')
    return statSync(gitPath).isDirectory()
  } catch {
    return false
  }
}
