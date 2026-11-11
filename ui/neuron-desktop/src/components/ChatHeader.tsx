import { useEffect, useId, useRef, useState } from 'react'

import { useFocusTrap } from '../a11y'
import { ChatError, type BranchList, type ChatInfo, type WorktreeStatus } from '../platform'
import { checksSentence, normalizePullRequestChecks, pullRequestLabel, type PullRequestSnapshot, type PullRequestStatus } from '../pullRequest'

/** How often the pull request chip asks again (the host keeps an answer as long). */
const PR_REFRESH_MS = 60_000

const everyMinute = (run: () => void) => {
  const timer = setInterval(run, PR_REFRESH_MS)
  return () => clearInterval(timer)
}

/**
 * v1.1: the pull request of the chat's branch, in the chat header: "PR #12 · draft ·
 * 3/4 checks". It opens the pull request in the browser. Nothing shows when there
 * is none, or when it cannot be known (no GitHub CLI, signed out, offline).
 */
export function PullRequestChip({
  load,
  onOpen,
  every = everyMinute,
}: {
  load: () => Promise<PullRequestSnapshot>
  onOpen: (url: string) => void
  /** Runs `run` on a schedule and returns how to stop it (tests drive it). */
  every?: (run: () => void) => () => void
}) {
  const [pr, setPr] = useState<PullRequestStatus | null>(null)
  const loadRef = useRef(load)
  loadRef.current = load
  useEffect(() => {
    let alive = true
    const ask = () =>
      void loadRef.current().then(
        (snapshot) => alive && setPr(snapshot.pullRequest),
        () => alive && setPr(null),
      )
    ask()
    const stop = every(ask)
    return () => {
      alive = false
      stop()
    }
  }, [every])
  if (!pr) return null
  const checks = pr.state === 'open' ? normalizePullRequestChecks(pr.checks) : 'none'
  return (
    <button
      className={`pr-chip pr-${pr.state}${pr.conflict ? ' pr-conflict' : ''}`}
      data-checks={checks}
      title={`${pr.title}\n${checksSentence(pr)}`}
      aria-label={`Pull request #${pr.number}: ${pr.title}. ${pullRequestLabel(pr)}. Open it in the browser`}
      onClick={() => onOpen(pr.url)}
    >
      {pullRequestLabel(pr)}
    </button>
  )
}

/**
 * v1.1: a new chat in its own copy of the project, started from a branch the
 * person picks (the repository's default branch is chosen first).
 */
export function NewCopyDialog({
  loadBranches,
  onCreate,
  onClose,
}: {
  loadBranches: () => Promise<BranchList>
  onCreate: (start: string) => void
  onClose: () => void
}) {
  const ref = useRef<HTMLDivElement>(null)
  useFocusTrap(ref)
  const selectId = useId()
  const [list, setList] = useState<BranchList | null>(null)
  const [start, setStart] = useState('')
  const [problem, setProblem] = useState<string | null>(null)
  useEffect(() => {
    let alive = true
    loadBranches().then(
      (loaded) => {
        if (!alive) return
        setList(loaded)
        setStart(loaded.default ?? loaded.branches[0] ?? '')
      },
      (failure: Error) => alive && setProblem(failure.message),
    )
    return () => {
      alive = false
    }
    // Read once, when the dialog opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
  return (
    <div className="dialog-backdrop" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <div
        ref={ref}
        className="dialog small-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-copy-title"
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.stopPropagation()
            onClose()
          }
        }}
      >
        <h2 id="new-copy-title">New chat in its own copy</h2>
        <p className="muted">The chat works on a new branch in a copy of the project, so your own folder is not touched. Its first message names the branch.</p>
        {!list && !problem && <p className="muted">Reading the branches…</p>}
        {problem && (
          <p className="error-line" role="alert">
            {problem}
          </p>
        )}
        {list && (
          <div className="field">
            <label htmlFor={selectId}>Start from</label>
            <select id={selectId} value={start} onChange={(event) => setStart(event.target.value)}>
              {list.branches.map((branch) => (
                <option key={branch} value={branch}>
                  {branch === list.default ? `${branch} (default)` : branch}
                </option>
              ))}
            </select>
          </div>
        )}
        <div className="dialog-buttons">
          <button onClick={onClose}>Cancel</button>
          <button className="primary" disabled={!start} onClick={() => onCreate(start)}>
            Make the copy
          </button>
        </div>
      </div>
    </div>
  )
}

/** After a question changes, answers to it wait this long. */
const CONFIRM_DELAY_MS = 400

/**
 * Under the chat's header, for a chat in a copy of the project of its own: the
 * copy's branch, a way into the editor (when the app can open one), and
 * Discard. Nothing for a chat in the project's own folder.
 */
export function ChatContextBar({
  info,
  onDiscard,
  onOpenInEditor,
}: {
  info: ChatInfo
  onDiscard: () => void
  /** Hidden when absent: the app wires it where it can open an editor. */
  onOpenInEditor?: () => void
}) {
  if (!info.worktree) return null
  return (
    <div className="chat-context" role="group" aria-label="This chat works in its own copy of the project">
      <span className="chat-branch" title={info.worktree.path}>
        <span aria-hidden>⎇ </span>
        {info.worktree.branch}
      </span>
      <span className="muted small">in its own copy, made from {info.worktree.base}</span>
      <span className="spacer" />
      {onOpenInEditor && (
        <button className="link-button" onClick={onOpenInEditor}>
          Open in editor
        </button>
      )}
      <button className="link-button danger-link" onClick={onDiscard}>
        Discard
      </button>
    </div>
  )
}

const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`

/**
 * Discarding a copy takes up to two answers. First it shows where the copy is
 * and how many files have changes in it; if its branch also holds saved
 * changes the main folder lacks, a second step asks again. Cancel is the
 * default. Nothing is removed until the last answer.
 */
export function DiscardDialog({
  info,
  loadStatus,
  discard,
  onClose,
  notice,
}: {
  /** A line before the question, when the person came here by closing the chat. */
  notice?: string
  info: ChatInfo
  loadStatus: () => Promise<WorktreeStatus>
  discard: (options: { force: boolean; deleteUnmerged: boolean }) => Promise<void>
  onClose: () => void
}) {
  const dialog = useRef<HTMLDivElement>(null)
  useFocusTrap(dialog)
  const [status, setStatus] = useState<WorktreeStatus | null>(null)
  const [step, setStep] = useState<1 | 2>(1)
  const [working, setWorking] = useState(false)
  const [problem, setProblem] = useState<string | null>(null)
  const cancel = useRef<HTMLButtonElement>(null)
  // When the question last changed: a double click or a double Enter is one answer, not two.
  const changedAt = useRef(0)
  const alive = useRef(true)
  const opened = useRef(true)
  useEffect(
    () => () => {
      alive.current = false
    },
    [],
  )

  const load = () =>
    loadStatus().then(
      (loaded) => alive.current && setStatus(loaded),
      (failure: Error) => alive.current && setProblem(failure.message),
    )
  // Read once, when the dialog opens.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => void load(), [])

  // Each question starts with the safe answer in focus.
  useEffect(() => {
    // Opening is not a change: the first question can be answered at once.
    if (opened.current) opened.current = false
    else changedAt.current = Date.now()
    cancel.current?.focus()
  }, [step])

  const run = async (deleteUnmerged: boolean) => {
    if (!status) return
    setWorking(true)
    setProblem(null)
    try {
      // `force` only for a copy with changes the person has just been shown.
      await discard({ force: status.dirty > 0, deleteUnmerged })
      onClose()
    } catch (failure) {
      setProblem(failure instanceof Error ? failure.message : String(failure))
      setWorking(false)
      // The copy changed since it was shown: look again and ask from the start.
      if (failure instanceof ChatError && (failure.code === 'dirty' || failure.code === 'unmerged')) {
        setStep(1)
        void load()
      }
    }
  }

  const next = () => {
    if (Date.now() - changedAt.current < CONFIRM_DELAY_MS) return
    if (status && status.unmerged > 0 && step === 1) setStep(2)
    else void run(step === 2)
  }

  return (
    <div className="dialog-backdrop" onMouseDown={(event) => event.target === event.currentTarget && !working && onClose()}>
      <div
        className="dialog small-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="discard-title"
        aria-describedby="discard-text"
        ref={dialog}
        onKeyDown={(event) => event.key === 'Escape' && !working && onClose()}
      >
        <h2 id="discard-title">{step === 1 ? 'Discard this copy?' : 'Delete its saved changes too?'}</h2>
        {notice && <p id="discard-notice">{notice}</p>}
        <div id="discard-text">
          {!status && !problem && <p className="muted">Looking at the copy…</p>}
          {status && step === 1 && (
            <>
              <p>
                This removes the copy of the project at <code className="path">{status.path}</code> and its branch <code>{status.branch}</code>. Your own
                folder is not touched.
              </p>
              {status.dirty > 0 && (
                <p>
                  <strong>{plural(status.dirty, 'file has', 'files have')} changes</strong> in it that are not saved to the branch. They will be lost.
                </p>
              )}
              {status.dirty === 0 && status.unmerged === 0 && <p className="muted">Nothing in it would be lost.</p>}
            </>
          )}
          {status && step === 2 && (
            <p>
              The branch <code>{status.branch}</code> has <strong>{plural(status.unmerged, 'saved change', 'saved changes')}</strong> that your own folder
              does not have. Discarding the copy deletes {status.unmerged === 1 ? 'it' : 'them'} for good.
            </p>
          )}
          {problem && (
            <p className="error-line" role="alert">
              {problem}
            </p>
          )}
        </div>
        <div className="dialog-buttons">
          <button ref={cancel} onClick={onClose} data-autofocus disabled={working}>
            Cancel
          </button>
          <button key={step} className="danger" onClick={next} disabled={!status || working}>
            {step === 1 ? (status && status.unmerged > 0 ? 'Continue' : 'Discard the copy') : 'Delete them and discard'}
          </button>
        </div>
      </div>
    </div>
  )
}
