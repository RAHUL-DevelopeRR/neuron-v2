import { useState } from 'react'

import { diffLines, additionDiff, toolView, type PendingPermission, type PermissionDecision } from '@neuron/client'

import { useKeys } from '../hooks'
import { DiffView } from './DiffView'

/**
 * A tool call waits for the user. Keys: y allows once, a allows for the
 * session (when the engine offers it), n denies, e denies with a note. Keys
 * are ignored for the first 300 ms, so typing in the composer can't answer it.
 */
export function PermissionDialog({
  pending,
  cwd,
  onAnswer,
}: {
  pending: PendingPermission
  cwd?: string | null
  onAnswer: (decision: PermissionDecision) => void
}) {
  const { request } = pending
  const [noting, setNoting] = useState(false)
  const [note, setNote] = useState('')
  const view = toolView(request.tool, request.input, cwd)
  const sessionAllowed = request.session_prefixes.length > 0

  useKeys(!noting, (event) => {
    if (event.target instanceof HTMLTextAreaElement || event.target instanceof HTMLInputElement) return
    const key = event.key.toLowerCase()
    if (key === 'y') onAnswer({ kind: 'allow_once' })
    else if (key === 'a' && sessionAllowed) onAnswer({ kind: 'allow_for_session' })
    else if (key === 'n' || key === 'escape') onAnswer({ kind: 'deny', note: null })
    else if (key === 'e') {
      event.preventDefault()
      setNoting(true)
    } else return
    event.preventDefault()
  })

  return (
    <div className="dialog-backdrop">
      <div className="dialog permission" role="dialog" aria-modal="true" aria-labelledby="permission-title">
        <h2 id="permission-title">
          Allow <code>{request.tool}</code>?
        </h2>
        {request.reason && <p className="permission-reason">{request.reason}</p>}
        <p className="muted">
          Needs <strong>{request.required_mode}</strong>; the session is in {request.current_mode}.
        </p>
        <div className="permission-preview">
          {view.kind === 'terminal' && <pre className="tool-command">$ {view.target}</pre>}
          {view.kind === 'edit' && <DiffView diff={view.diff ?? diffLines('', '')} label={`edit to ${view.target}`} />}
          {view.kind === 'write' && (
            <>
              <div className="muted">Write {view.target}</div>
              <DiffView diff={view.diff ?? additionDiff('')} label={`write to ${view.target}`} />
            </>
          )}
          {view.kind !== 'terminal' && view.kind !== 'edit' && view.kind !== 'write' && (
            <pre className="tool-output">{prettyJson(request.input)}</pre>
          )}
        </div>
        {noting ? (
          <form
            className="deny-note"
            onSubmit={(event) => {
              event.preventDefault()
              onAnswer({ kind: 'deny', note: note.trim() || null })
            }}
          >
            <textarea
              autoFocus
              value={note}
              placeholder="Tell Neuron what to do instead"
              onChange={(event) => setNote(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter' && !event.shiftKey) {
                  event.preventDefault()
                  onAnswer({ kind: 'deny', note: note.trim() || null })
                }
              }}
            />
            <div className="dialog-buttons">
              <button type="button" onClick={() => setNoting(false)}>
                Back
              </button>
              <button type="submit" className="danger">
                Deny with note
              </button>
            </div>
          </form>
        ) : (
          <div className="dialog-buttons">
            <button className="primary" onClick={() => onAnswer({ kind: 'allow_once' })}>
              Allow once <kbd>y</kbd>
            </button>
            {sessionAllowed && (
              <button onClick={() => onAnswer({ kind: 'allow_for_session' })} title={request.session_prefixes.join(', ')}>
                Allow <code>{request.session_prefixes.join(', ')}</code> this session <kbd>a</kbd>
              </button>
            )}
            <button className="danger" onClick={() => onAnswer({ kind: 'deny', note: null })}>
              Deny <kbd>n</kbd>
            </button>
            <button onClick={() => setNoting(true)}>
              Deny with note <kbd>e</kbd>
            </button>
          </div>
        )}
      </div>
    </div>
  )
}

function prettyJson(text: string): string {
  try {
    return JSON.stringify(JSON.parse(text), null, 2)
  } catch {
    return text
  }
}
