import { existsSync, statSync } from 'fs'
import os from 'os'
import path from 'path'

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

import {
  getDirectories,
  hasGitDirectory,
  isWindowsDriveRoot,
  sessionDriveRoots,
} from '../utils/directory-browser'

import type { DirectoryEntry } from '../utils/directory-browser'

export interface UseDirectoryBrowserOptions {
  /** Initial path to browse from (defaults to home directory) */
  initialPath?: string
  /** The platform (tests); defaults to this one. */
  platform?: NodeJS.Platform
  /**
   * Finds the drive roots, off the render path (tests); defaults to one
   * probe of `C:\` to `Z:\` per session.
   */
  probeDrives?: () => Promise<string[]>
}

export interface UseDirectoryBrowserReturn {
  /** Current directory path */
  currentPath: string
  /** Set the current directory path */
  setCurrentPath: (path: string) => void
  /** List of directory entries in the current path */
  directories: DirectoryEntry[]
  /** Whether current directory is a git repository */
  isGitRepo: boolean
  /** Expand ~ to home directory */
  expandPath: (inputPath: string) => string
  /** Try to navigate to a typed path, returns true if successful */
  tryNavigateToPath: (inputPath: string) => boolean
  /** Navigate to a directory entry */
  navigateToDirectory: (entry: DirectoryEntry) => void
}

/**
 * Hook for directory browsing state and navigation.
 * Provides current path state, directory listing, git detection,
 * and navigation utilities.
 */
export function useDirectoryBrowser({
  initialPath,
  platform = process.platform,
  probeDrives = sessionDriveRoots,
}: UseDirectoryBrowserOptions = {}): UseDirectoryBrowserReturn {
  const [currentPath, setCurrentPath] = useState(initialPath ?? os.homedir())

  // A Windows drive root lists the other drives. Probing them can block for
  // tens of seconds (a disconnected network drive), so it never happens while
  // drawing: the folders show at once and the drive rows join them when the
  // probe answers, once per picker.
  const atDriveRoot = platform === 'win32' && isWindowsDriveRoot(currentPath)
  const [drives, setDrives] = useState<readonly string[] | null>(null)
  const probeStarted = useRef(false)
  const mounted = useRef(true)
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])
  useEffect(() => {
    if (!atDriveRoot || probeStarted.current) return
    probeStarted.current = true
    // The answer is for every drive root, so it still holds after a move.
    Promise.resolve()
      .then(probeDrives)
      .then(
        (found) => {
          if (mounted.current) setDrives(found)
        },
        () => {
          if (mounted.current) setDrives([])
        },
      )
  }, [atDriveRoot, probeDrives])

  // Get directories for current path
  const directories = useMemo(
    () => getDirectories(currentPath, { platform, drives: drives ?? [] }),
    [currentPath, platform, drives],
  )

  // Check if current directory has .git
  const isGitRepo = useMemo(() => hasGitDirectory(currentPath), [currentPath])

  // Expand ~ to home directory
  const expandPath = useCallback((inputPath: string): string => {
    if (inputPath.startsWith('~')) {
      return path.join(os.homedir(), inputPath.slice(1))
    }
    return inputPath
  }, [])

  // Try to navigate to a typed path
  const tryNavigateToPath = useCallback(
    (inputPath: string): boolean => {
      const expandedPath = expandPath(inputPath.trim())
      try {
        if (existsSync(expandedPath) && statSync(expandedPath).isDirectory()) {
          setCurrentPath(expandedPath)
          return true
        }
      } catch {
        // Path doesn't exist or can't be accessed
      }
      return false
    },
    [expandPath],
  )

  // Navigate to a directory entry
  const navigateToDirectory = useCallback((entry: DirectoryEntry) => {
    setCurrentPath(entry.path)
  }, [])

  return {
    currentPath,
    setCurrentPath,
    directories,
    isGitRepo,
    expandPath,
    tryNavigateToPath,
    navigateToDirectory,
  }
}
