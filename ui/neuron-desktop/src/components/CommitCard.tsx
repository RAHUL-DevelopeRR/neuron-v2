import { useEffect, useState } from 'react'

import type { CommitProposalEvent } from '@neuron/client'

/** A drafted commit: edit the message, see what it takes, then commit or cancel. Nothing is committed until the button. */
export function CommitCard({
  proposal,
  busy,
  onCommit,
  onCancel,
}: {
  proposal: CommitProposalEvent
  busy: boolean
  onCommit: (message: string) => void
  onCancel: () => void
}) {
  const [message, setMessage] = useState(proposal.message)
  // A new draft replaces what was typed for the old one.
  useEffect(() => setMessage(proposal.message), [proposal])
  const empty = message.trim() === ''
  return (
    <form
      className="commit-card"
      aria-label="Commit"
      onSubmit={(event) => {
        event.preventDefault()
        if (!empty && !busy) onCommit(message.trim())
      }}
    >
      <label className="commit-label" htmlFor="commit-message">
        Commit message
      </label>
      <textarea id="commit-message" className="commit-message" rows={4} value={message} onChange={(event) => setMessage(event.target.value)} />
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
        <button type="submit" className="primary" disabled={empty || busy}>
          Commit changes
        </button>
        <button type="button" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </form>
  )
}
