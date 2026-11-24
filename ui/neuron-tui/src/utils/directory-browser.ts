import { existsSync, readdirSync, statSync } from 'fs'
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

export interface DirectoryListingOptions {
  /** List files too, after the folders (hidden files stay hidden). */
  includeFiles?: boolean
  /** The platform the rows are for (tests); defaults to this one. */
  platform?: NodeJS.Platform
  /** Whether a drive root such as `D:\` exists (tests); defaults to the disk. */
  driveExists?: (root: string) => boolean
}

/** The project picker's name for the same options. */
export type GetDirectoriesOptions = DirectoryListingOptions

const DRIVE_ROOT = /^[A-Za-z]:[\\/]?$/

/** Whether `dir` is a Windows drive's root, such as `C:\`. */
export function isWindowsDriveRoot(dir: string): boolean {
  return DRIVE_ROOT.test(dir)
}

/**
 * The other drives' roots, `C:\` to `Z:\` (A and B are floppy letters, slow to
 * probe on some machines), less the drive `dir` is on.
 */
export function listOtherDrives(
  dir: string,
  driveExists: (root: string) => boolean = defaultDriveExists,
): string[] {
  const current = dir.slice(0, 1).toUpperCase()
  const drives: string[] = []
  for (let code = 'C'.charCodeAt(0); code <= 'Z'.charCodeAt(0); code++) {
    const letter = String.fromCharCode(code)
    if (letter === current) continue
    const root = `${letter}:\\`
    if (driveExists(root)) drives.push(root)
  }
  return drives
}

function defaultDriveExists(root: string): boolean {
  try {
    return existsSync(root)
  } catch {
    return false
  }
}

/**
 * Get directory entries for a given path, including parent directory option.
 * Skips hidden directories (those starting with '.'). At a Windows drive's
 * root, where there is no parent, the other drives are listed first, so the
 * mouse can reach them as the keyboard reaches a typed `D:\`. With
 * `includeFiles`, the files follow the folders.
 */
export function getDirectories(
  dirPath: string,
  options: DirectoryListingOptions = {},
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
  const drives: DirectoryEntry[] =
    platform === 'win32' && isWindowsDriveRoot(dirPath)
      ? listOtherDrives(dirPath, options.driveExists).map((root) => ({
          name: root,
          path: root,
          isParent: false,
          isGitRepo: false,
          isDrive: true,
        }))
      : []

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
