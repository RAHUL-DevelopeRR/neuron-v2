import type { CommitProposalEvent } from '@neuron/client'

/** A drafted commit: edit the message, see what it takes, then commit or cancel. Nothing is committed until the button. */
export function CommitCard({
  proposal,
  message,
  sending,
  error,
  busy,
  onMessage,
  onCommit,
  onCancel,
}: {
  proposal: CommitProposalEvent
  /** What the user wrote; the pane keeps it, so a failed commit does not lose it. */
  message: string
  /** The commit went out and the engine has not answered. */
  sending: boolean
  /** The engine's reason, when the commit failed. */
  error: string | null
  busy: boolean
  onMessage: (message: string) => void
  onCommit: (message: string) => void
  onCancel: () => void
}) {
  const empty = message.trim() === ''
  return (
    <form
      className="commit-card"
      aria-label="Commit"
      aria-busy={sending}
      onSubmit={(event) => {
        event.preventDefault()
        if (!empty && !busy && !sending) onCommit(message.trim())
      }}
    >
      <label className="commit-label" htmlFor="commit-message">
        Commit message
      </label>
      <textarea
        id="commit-message"
        className="commit-message"
        rows={4}
        value={message}
        readOnly={sending}
        onChange={(event) => onMessage(event.target.value)}
      />
      {error && (
        <div className="error-line small" role="alert">
          {error}
        </div>
      )}
      <div className="muted small">{proposal.branch ? `On branch ${proposal.branch}` : 'Not on a branch'}</div>
      {proposal.stage_all && <div className="commit-note">Commits every change (nothing is staged)</div>}
      <ul className="commit-files" aria-label="Files in this commit">
        {proposal.files.map((file) => (
          <li key={file.path}>
            <span className="commit-path" title={file.path}>
              {file.path}
            </span>
            <span className="diff-added">+{file.added}</span> <span className="diff-removed">−{file.removed}</span>
          </li>
        ))}
      </ul>
      <div className="dialog-buttons start">
        <button type="submit" className="primary" disabled={empty || busy || sending}>
          {sending ? 'Committing…' : 'Commit changes'}
        </button>
        <button type="button" disabled={sending} onClick={onCancel}>
          Cancel
        </button>
      </div>
    </form>
  )
}
