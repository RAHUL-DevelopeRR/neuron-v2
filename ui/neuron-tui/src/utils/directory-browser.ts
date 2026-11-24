import { readdirSync, statSync } from 'fs'
import path from 'path'

export type DirectoryEntry = {
  name: string
  path: string
  isParent: boolean
  isGitRepo: boolean
  /** A file, listed only when files were asked for (the attach browser). */
  isFile?: boolean
}

export interface DirectoryListingOptions {
  /** List files too, after the folders (hidden files stay hidden). */
  includeFiles?: boolean
}

/**
 * Get directory entries for a given path, including parent directory option.
 * Skips hidden directories (those starting with '.'). With `includeFiles`,
 * the files follow the folders.
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

  const listed = [...childEntries, ...files]
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
