/*
 * Ported from Freebuff common/src/desktop-cloud/threads.ts (commit 0dc95cb),
 * https://github.com/CodebuffAI/freebuff, Copyright Codebuff, Apache License 2.0
 * (see NOTICE and docs/PROVENANCE.md). Changed for Neuron: the zod schemas became
 * plain types (this package has no runtime dependencies); the cloud/local kinds,
 * funding, git, pull requests, resolved threads, queued runs and the panel layout
 * were dropped; approvals wait like questions; a stopped engine is a run state of
 * its own; and `sessionDigest` builds the digest from a `SessionState`.
 */

/**
 * A chat at a glance, as a list of chats shows it: which group it is in, how far
 * its plan got, and one line of what it is doing. Everything a row shows comes
 * from these fields, so every client words a chat the same way.
 */

import { gatewayProblem, type GatewayProblemKind } from './gateway'
import { openQuestionItem, type SessionState, type TranscriptItem } from './session'

export type ThreadStepState = 'pending' | 'active' | 'done'

/** `stopped`: the chat's engine is not running. */
export type ThreadRunState = 'running' | 'idle' | 'stopped'

export type ThreadOutcome = 'completed' | 'stopped' | 'failed'

export const THREAD_DIGEST_MAX_STEPS = 50
export const THREAD_DIGEST_TEXT_LIMIT = 280

export interface ThreadDigestStep {
  text: string
  state: ThreadStepState
}

/** What the chat waits on the user for: an answer, or an OK for a tool call. */
export interface ThreadWaiting {
  kind: 'question' | 'approval'
  text: string
}

export interface ThreadDigest {
  id: string
  title: string
  runState: ThreadRunState
  /** The last finished turn's outcome; null before the first turn ends. */
  outcome: ThreadOutcome | null
  /** The task list of the current turn, or the last one's when idle. */
  steps: ThreadDigestStep[]
  /** Written by the producer; rows fall back to {@link threadStatusLine}. */
  statusLine: string | null
  /** The first line of the last reply. */
  lastMessage: string | null
  waiting: ThreadWaiting | null
}

export const THREAD_GROUPS = ['waiting', 'working', 'idle'] as const
export type ThreadGroup = (typeof THREAD_GROUPS)[number]

export const THREAD_GROUP_LABELS: Record<ThreadGroup, string> = {
  waiting: 'Needs you',
  working: 'Working',
  idle: 'Idle',
}

/**
 * A question, an approval or a failed turn needs the user even while a turn
 * is still running, so waiting outranks working. A failed turn only waits once
 * the chat is idle: while it runs, it may still recover.
 */
export function threadGroup(digest: ThreadDigest): ThreadGroup {
  if (digest.waiting !== null || (digest.runState === 'idle' && digest.outcome === 'failed')) return 'waiting'
  if (digest.runState === 'running') return 'working'
  return 'idle'
}

/** The row's progress, e.g. 2/4; null when the chat has no plan. */
export function threadStepCount(digest: Pick<ThreadDigest, 'steps'>): { done: number; total: number } | null {
  if (digest.steps.length === 0) return null
  const done = digest.steps.filter((step) => step.state === 'done').length
  return { done, total: digest.steps.length }
}

/** Text as one bounded line; null when nothing is left. */
export function digestText(value: string | null | undefined): string | null {
  if (!value) return null
  const line = value.replace(/\s+/g, ' ').trim()
  if (!line) return null
  return line.length > THREAD_DIGEST_TEXT_LIMIT ? `${line.slice(0, THREAD_DIGEST_TEXT_LIMIT - 1)}…` : line
}

/** The row's one-line status. What waits on the user wins, then the producer's own line, then the shared fallbacks. */
export function threadStatusLine(digest: ThreadDigest): string {
  if (digest.waiting) return digest.waiting.text
  if (digest.statusLine) return digest.statusLine
  if (digest.runState === 'running') {
    const active = digest.steps.find((step) => step.state === 'active')
    return active?.text ?? 'Working'
  }
  if (digest.runState === 'stopped') return 'Not running'
  if (digest.outcome === 'failed') return 'Turn failed'
  if (digest.outcome === 'stopped') return 'Stopped'
  return digest.lastMessage ?? (digest.outcome === null ? 'Not started' : 'Finished')
}

// ---------------------------------------------------------------- from a session

/** What a failed turn needs, in words: never the service's own message, which may name whoever served it. */
const PROBLEM_LINE: Record<GatewayProblemKind, string> = {
  sign_in: 'Signed out: sign in again',
  upgrade: 'Needs a plan upgrade',
  quota: 'Out of quota for now',
  busy: 'The service was busy: try again',
  unreachable: 'Could not reach the service',
}

const STEP_STATE: Record<string, ThreadStepState> = { completed: 'done', in_progress: 'active' }

type Tool = Extract<TranscriptItem, { kind: 'tool' }>

/** The latest thing a running turn does, in plain words; null when nothing is going on. */
function activityLine(turn: readonly TranscriptItem[]): string | null {
  for (let index = turn.length - 1; index >= 0; index--) {
    const item = turn[index]
    if (item.kind === 'tool') {
      if (item.subagent?.running) return digestText(`Sub-agent: ${item.subagent.description}`)
      if (item.state === 'running' || item.state === 'writing' || item.state === 'awaiting_permission') {
        return toolLine(item)
      }
      return null
    }
    if (item.kind === 'reasoning') return item.streaming ? 'Thinking' : null
    if (item.kind === 'assistant') return item.streaming ? 'Writing a reply' : null
    if (item.kind === 'user') return null
  }
  return null
}

const toolLine = (tool: Tool): string | null => digestText(tool.view.target ? `${tool.view.verb} ${tool.view.target}` : tool.view.verb)

/** How the last finished turn ended; null before one did. */
function outcomeOf(state: SessionState, turn: readonly TranscriptItem[], started: boolean): ThreadOutcome | null {
  if (!started) return null
  if (state.problem || turn.some((item) => item.kind === 'error')) return 'failed'
  if (turn.some((item) => item.kind === 'tool' && item.state === 'cut_off')) return 'stopped'
  return 'completed'
}

/**
 * The digest of one chat, from its session: steps from the task list, a pending
 * approval or question as what it waits on, a turn that ended in an error as
 * failed, and the latest activity as the status line.
 */
export function sessionDigest(state: SessionState, chat: { id: string; title: string }): ThreadDigest {
  // The current turn: everything after the last prompt the user sent.
  let start = -1
  for (let index = state.items.length - 1; index >= 0; index--) {
    const item = state.items[index]
    if (item.kind === 'user' && item.prompt !== false) {
      start = index
      break
    }
  }
  const turn = state.items.slice(start + 1)
  const runState: ThreadRunState = state.connection === 'exited' ? 'stopped' : state.busy ? 'running' : 'idle'

  const steps: ThreadDigestStep[] = []
  for (const todo of state.todos.slice(0, THREAD_DIGEST_MAX_STEPS)) {
    const stepState = STEP_STATE[todo.status] ?? 'pending'
    const text = digestText(stepState === 'active' ? todo.active_form ?? todo.content : todo.content)
    if (text) steps.push({ text, state: stepState })
  }

  let waiting: ThreadWaiting | null = null
  const permission = state.permissions[0]
  if (permission) {
    const tool = permission.toolKey ? state.items.find((item): item is Tool => item.kind === 'tool' && item.key === permission.toolKey) : undefined
    const what = (tool && toolLine(tool)) ?? digestText(permission.request.tool)
    waiting = { kind: 'approval', text: digestText(`Waiting for your OK: ${what}`)! }
  } else {
    const question = openQuestionItem(state)
    const text = question ? digestText(question.text) : null
    if (text) waiting = { kind: 'question', text }
  }

  const outcome = runState === 'running' ? null : outcomeOf(state, turn, start >= 0 || turn.length > 0)

  let statusLine: string | null = null
  if (runState === 'running') statusLine = activityLine(turn)
  else if (outcome === 'failed') {
    const error = [...turn].reverse().find((item) => item.kind === 'error')
    const problem = state.problem ?? (error?.kind === 'error' ? error.problem ?? gatewayProblem({ message: error.text }) : null)
    statusLine = problem ? PROBLEM_LINE[problem.kind] : null
  }

  let lastMessage: string | null = null
  for (let index = turn.length - 1; index >= 0 && lastMessage === null; index--) {
    const item = turn[index]
    if (item.kind === 'assistant' && !item.streaming) lastMessage = digestText(item.text.split('\n').find((line) => line.trim()) ?? '')
  }

  return { id: chat.id, title: chat.title, runState, outcome, steps, statusLine, lastMessage, waiting }
}
