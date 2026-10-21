// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: `~`
// completions keep their `~` form whichever separator HOME uses, and a
// trailing `/` counts as a separator on Windows. See ui/neuron-tui/PORTING.md.
import { existsSync, readdirSync, statSync } from 'fs'
import os from 'os'
import path from 'path'

const isWindows = process.platform === 'win32'

const endsWithSeparator = (value: string): boolean =>
  isWindows ? /[\\/]$/.test(value) : value.endsWith('/')

/**
 * `completed`, an absolute path under `homeDir`, back in the `~` form the user
 * typed. HOME may use either separator (`C:/Users/me` from a POSIX-style HOME
 * on Windows, where the joined path has backslashes), so both are normalized
 * before comparing; after the `~`, the separator the user typed is kept.
 */
function toTildeForm(
  completed: string,
  inputPath: string,
  homeDir: string,
): string {
  const home = path.normalize(homeDir).replace(/[\\/]+$/, '')
  const normalized = path.normalize(completed)
  const fold = (value: string) => (isWindows ? value.toLowerCase() : value)
  if (!fold(normalized).startsWith(fold(home))) return completed
  const rest = normalized.slice(home.length)
  // `/home/meg` is not under `/home/me`.
  if (rest && !/^[\\/]/.test(rest)) return completed
  const typed = inputPath[1]
  const typedSeparator = typed === '/' || typed === '\\' ? typed : null
  return '~' + (typedSeparator ? rest.replace(/[\\/]/g, typedSeparator) : rest)
}

/**
 * Get path completions for tab completion.
 * Returns the completed path or null if no completion available.
 *
 * Behavior:
 * - Empty input returns null
 * - Expands ~ to home directory
 * - Single match: returns completed path with trailing slash
 * - Multiple matches: returns common prefix (no trailing slash)
 * - No matches: returns null
 * - Hidden directories are skipped unless user typed a dot
 *
 * @param homeDir - The home directory `~` stands for (tests pass their own).
 */
export function getPathCompletion(
  inputPath: string,
  homeDir: string = os.homedir(),
): string | null {
  if (!inputPath) return null

  // Expand ~ to home directory
  let expandedPath = inputPath
  if (expandedPath.startsWith('~')) {
    expandedPath = path.join(homeDir, expandedPath.slice(1))
  }

  // If the path ends with /, we're completing inside that directory
  // Otherwise, we're completing the last component
  let parentDir: string
  let partial: string

  if (endsWithSeparator(expandedPath)) {
    parentDir = expandedPath
    partial = ''
  } else {
    parentDir = path.dirname(expandedPath)
    partial = path.basename(expandedPath).toLowerCase()
  }

  // Check if parent directory exists
  try {
    if (!existsSync(parentDir) || !statSync(parentDir).isDirectory()) {
      return null
    }
  } catch {
    return null
  }

  // List directories in parent that match the partial
  try {
    const items = readdirSync(parentDir)
    const matches: string[] = []

    for (const item of items) {
      // Skip hidden files unless user typed a dot
      if (item.startsWith('.') && !partial.startsWith('.')) continue

      const fullPath = path.join(parentDir, item)
      try {
        if (!statSync(fullPath).isDirectory()) continue
      } catch {
        continue
      }

      if (item.toLowerCase().startsWith(partial)) {
        matches.push(item)
      }
    }

    if (matches.length === 0) return null

    // If exactly one match, complete to it with trailing slash
    if (matches.length === 1) {
      const completed = path.join(parentDir, matches[0]) + path.sep
      // Convert back to ~ format if it was originally using ~
      return inputPath.startsWith('~')
        ? toTildeForm(completed, inputPath, homeDir)
        : completed
    }

    // Multiple matches - find common prefix
    const sortedMatches = matches.sort()
    const first = sortedMatches[0].toLowerCase()
    const last = sortedMatches[sortedMatches.length - 1].toLowerCase()
    let commonLength = partial.length

    while (
      commonLength < first.length &&
      first[commonLength] === last[commonLength]
    ) {
      commonLength++
    }

    // If we can extend beyond what's already typed
    if (commonLength > partial.length) {
      // Use the case from the first match
      const commonPrefix = sortedMatches[0].slice(0, commonLength)
      const completed = path.join(parentDir, commonPrefix)
      // Convert back to ~ format if it was originally using ~
      return inputPath.startsWith('~')
        ? toTildeForm(completed, inputPath, homeDir)
        : completed
    }

    return null
  } catch {
    return null
  }
}
