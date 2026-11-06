import { useEffect, useRef, useState } from 'react'

import { useFocusTrap } from '../a11y'
import type { ChatInfo, WorktreeStatus } from '../platform'

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
}: {
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

  useEffect(() => {
    let cancelled = false
    loadStatus().then(
      (loaded) => !cancelled && setStatus(loaded),
      (failure: Error) => !cancelled && setProblem(failure.message),
    )
    return () => {
      cancelled = true
    }
    // Read once, when the dialog opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

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
    }
  }

  const next = () => (status && status.unmerged > 0 && step === 1 ? setStep(2) : void run(step === 2))

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
          <button onClick={onClose} data-autofocus disabled={working}>
            Cancel
          </button>
          <button className="danger" onClick={next} disabled={!status || working}>
            {step === 1 ? (status && status.unmerged > 0 ? 'Continue' : 'Discard the copy') : 'Delete them and discard'}
          </button>
        </div>
      </div>
    </div>
  )
}
