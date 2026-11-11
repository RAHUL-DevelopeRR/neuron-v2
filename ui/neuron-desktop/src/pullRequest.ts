/*
 * Ported from Freebuff, common/src/desktop-cloud/pull-request.ts, commit 0dc95cb
 * (https://github.com/CodebuffAI/freebuff). Copyright Codebuff, Apache License 2.0.
 * Changed for Neuron: plain types instead of zod schemas; `conflict` and the raw
 * `checks` list (from `gh pr view`, read by src-tauri/src/pr.rs) added to the
 * status; the per-check rule split out as `checkState`, and `checkCounts` and
 * `pullRequestLabel` added for the chat header's chip. See ../NOTICE.
 */

export type PullRequestChecks = 'pending' | 'passed' | 'failed' | 'none' | 'unknown'

/** One entry of GitHub's status check rollup: a check run or a commit status. */
export interface PullRequestCheck {
  status?: string | null
  state?: string | null
  conclusion?: string | null
}

export interface PullRequestStatus {
  url: string
  number: number
  title: string
  state: 'open' | 'merged' | 'closed'
  draft: boolean
  /** The pull request no longer merges cleanly. */
  conflict: boolean
  /** `null` when GitHub sent no list. */
  checks: PullRequestCheck[] | null
}

export interface PullRequestSnapshot {
  pullRequest: PullRequestStatus | null
  /** The GitHub CLI could not answer (not installed, signed out, offline). */
  unavailable?: boolean
}

const FAILED = ['FAILURE', 'ERROR', 'TIMED_OUT', 'CANCELLED', 'ACTION_REQUIRED', 'STARTUP_FAILURE', 'STALE']
const PENDING = ['PENDING', 'EXPECTED', 'QUEUED', 'IN_PROGRESS', 'WAITING', 'REQUESTED', 'PENDING_DEPLOYMENT']
const PASSED = ['SUCCESS', 'NEUTRAL', 'SKIPPED']

/** One check's state: a check run's conclusion once completed, else its status; a commit status's state. */
export function checkState(item: unknown): PullRequestChecks {
  if (!item || typeof item !== 'object') return 'unknown'
  const check = item as Record<string, unknown>
  const status = typeof check.status === 'string' ? check.status.toUpperCase() : null
  const raw = check.state ?? (status === 'COMPLETED' ? check.conclusion : status)
  const state = typeof raw === 'string' ? raw.toUpperCase() : null
  if (FAILED.includes(String(state))) return 'failed'
  if (PENDING.includes(String(state))) return 'pending'
  if (PASSED.includes(String(state))) return 'passed'
  return 'unknown'
}

/** GitHub combines CheckRun and StatusContext entries in statusCheckRollup. */
export function normalizePullRequestChecks(value: unknown): PullRequestChecks {
  if (!Array.isArray(value)) return 'unknown'
  if (!value.length) return 'none'
  const states = value.map(checkState)
  return states.includes('failed') ? 'failed' : states.includes('pending') ? 'pending' : states.includes('unknown') ? 'unknown' : 'passed'
}

/** How many checks passed, out of all; `null` when there is no list. */
export function checkCounts(value: unknown): { passed: number; total: number } | null {
  if (!Array.isArray(value)) return null
  return { passed: value.filter((item) => checkState(item) === 'passed').length, total: value.length }
}

/** "PR #12 · draft · 3/4 checks": the number, where it stands, and its checks while it is open. */
export function pullRequestLabel(pr: PullRequestStatus): string {
  const parts = [`PR #${pr.number}`, pr.state !== 'open' ? pr.state : pr.draft ? 'draft' : 'open']
  if (pr.state === 'open') {
    if (pr.conflict) parts.push('conflicts')
    const counts = checkCounts(pr.checks)
    if (counts && counts.total > 0) parts.push(`${counts.passed}/${counts.total} checks`)
  }
  return parts.join(' · ')
}

/** The checks in a sentence, for the chip's tooltip. */
export function checksSentence(pr: PullRequestStatus): string {
  const counts = checkCounts(pr.checks)
  if (!counts || counts.total === 0) return 'No checks reported.'
  const failed = (pr.checks ?? []).filter((item) => checkState(item) === 'failed').length
  const pending = (pr.checks ?? []).filter((item) => checkState(item) === 'pending').length
  const words = [`${counts.passed} of ${counts.total} checks passed`]
  if (failed) words.push(`${failed} failed`)
  if (pending) words.push(`${pending} still running`)
  return `${words.join(', ')}.`
}
