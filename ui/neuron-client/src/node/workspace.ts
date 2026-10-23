/**
 * Read-only views of the workspace for a UI: files by name (for `@` mentions)
 * and git changes with their diffs. Git runs the way the engine runs its own
 * git (runtime/src/git_context.rs): no optional locks, no fsmonitor, no
 * external diff or textconv, and nothing at all in an untrusted repository
 * whose own config names a program for git to run.
 */

import { promises as fs } from 'fs'
import path from 'path'

const SKIPPED_DIRS = new Set(['.git', 'node_modules', 'target', 'dist', 'build', '.neuron', '.next', '.venv', '__pycache__'])
const MAX_SCANNED = 20_000

/** Workspace files whose path contains `query` (case-insensitive), best matches first. */
export async function searchFiles(root: string, query: string, limit = 50): Promise<string[]> {
  const needle = query.trim().toLowerCase().replace(/\\/g, '/')
  const found: Array<{ path: string; score: number }> = []
  let scanned = 0
  async function walk(dir: string, relative: string): Promise<void> {
    if (scanned >= MAX_SCANNED) return
    let entries: import('fs').Dirent[]
    try {
      entries = await fs.readdir(dir, { withFileTypes: true })
    } catch {
      return
    }
    for (const entry of entries) {
      if (scanned >= MAX_SCANNED) return
      const entryPath = relative ? `${relative}/${entry.name}` : entry.name
      if (entry.isDirectory()) {
        if (!SKIPPED_DIRS.has(entry.name)) await walk(path.join(dir, entry.name), entryPath)
        continue
      }
      scanned++
      const lower = entryPath.toLowerCase()
      const at = lower.indexOf(needle)
      if (needle && at < 0) continue
      const nameAt = entry.name.toLowerCase().indexOf(needle)
      // A match in the file name beats one in a folder name; shorter paths first.
      found.push({ path: entryPath, score: (nameAt >= 0 ? 0 : 1000) + at + entryPath.length / 100 })
    }
  }
  await walk(root, '')
  return found
    .sort((a, b) => a.score - b.score)
    .slice(0, limit)
    .map((entry) => entry.path)
}

export interface GitChange {
  /** Relative to the workspace, with `/`. */
  path: string
  /** Porcelain v1 XY: index then work tree (`M`, `A`, `D`, `R`, `?`, `U`, ` `). */
  index: string
  worktree: string
}

export interface GitStatusView {
  branch: string | null
  changes: GitChange[]
}

const PROGRAM_CONFIG = String.raw`^(core\.fsmonitor|filter\..*\.(clean|smudge|process)|diff\.external|diff\..*\.(command|textconv))$`

function git(root: string, args: string[]): { ok: boolean; stdout: string; stderr: string; code: number | null } {
  const child = Bun.spawnSync(
    ['git', '--no-optional-locks', '-c', 'core.fsmonitor=false', '-c', 'log.showSignature=false', ...args],
    { cwd: root, env: { ...process.env, GIT_TERMINAL_PROMPT: '0', GIT_OPTIONAL_LOCKS: '0' }, stdout: 'pipe', stderr: 'pipe' },
  )
  return {
    ok: child.exitCode === 0,
    stdout: child.stdout.toString(),
    stderr: child.stderr.toString(),
    code: child.exitCode,
  }
}

/**
 * Refuses an untrusted repository whose own (local or worktree) config names a
 * program git would run while reading status or diffs. Reading config runs nothing.
 */
function assertSafeRepository(root: string, trusted: boolean): void {
  if (trusted) return
  const config = Bun.spawnSync(['git', 'config', '--show-scope', '--get-regexp', PROGRAM_CONFIG], {
    cwd: root,
    stdout: 'pipe',
    stderr: 'pipe',
  })
  if (config.exitCode === 1) return
  const repoScoped =
    config.exitCode !== 0 ||
    config.stdout
      .toString()
      .split('\n')
      .some((line) => ['local', 'worktree'].includes(line.split('\t')[0]))
  if (repoScoped) {
    throw new Error("This repository's own git config runs programs for status and diffs; trust the workspace to see its changes.")
  }
}

export function gitStatus(root: string, trusted: boolean): GitStatusView | null {
  assertSafeRepository(root, trusted)
  const result = git(root, ['status', '--porcelain=v1', '-z', '--branch', '--untracked-files=all'])
  if (!result.ok) return null
  const records = result.stdout.split('\0').filter(Boolean)
  let branch: string | null = null
  const changes: GitChange[] = []
  for (let index = 0; index < records.length; index++) {
    const record = records[index]
    if (record.startsWith('## ')) {
      branch = record.slice(3).split('...')[0].replace(/^No commits yet on /, '') || null
      continue
    }
    const xy = record.slice(0, 2)
    changes.push({ path: record.slice(3), index: xy[0], worktree: xy[1] })
    // A rename's source path follows it as its own record.
    if (xy[0] === 'R' || xy[0] === 'C') index++
  }
  return { branch, changes }
}

/** The change to one file against HEAD (staged and unstaged), as a unified diff. */
export function gitDiff(root: string, file: string, trusted: boolean): string {
  assertSafeRepository(root, trusted)
  if (file.startsWith('-') || path.isAbsolute(file) || file.split(/[\\/]/).includes('..')) {
    throw new Error('That path is outside the workspace.')
  }
  const head = git(root, ['diff', 'HEAD', '--no-ext-diff', '--no-textconv', '--no-color', '--', file])
  if (head.ok) return head.stdout
  // No commits yet: everything is staged or untracked.
  const cached = git(root, ['diff', '--cached', '--no-ext-diff', '--no-textconv', '--no-color', '--', file])
  return cached.ok ? cached.stdout : ''
}
