import { useEffect, useRef, useState } from 'react'

import { diffLines, additionDiff, toolView, type PendingPermission, type PermissionDecision } from '@neuron/client'

import { useKeys } from '../hooks'
import { DiffView } from './DiffView'

/**
 * A tool call waits for the user. Keys: Enter (or y) allows once, a allows for the
 * session (when the engine offers it), Esc (or n) denies, e denies with a note.
 * Keys are ignored for the first 300 ms, so typing in the composer can't answer
 * it. Focus moves into the dialog, stays inside it, and goes back to where it was.
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
  const dialog = useRef<HTMLDivElement>(null)

  // Focus goes to the dialog itself (not a button, so a held Enter can't answer it), and back after.
  useEffect(() => {
    const before = document.activeElement instanceof HTMLElement ? document.activeElement : null
    dialog.current?.focus()
    return () => {
      if (before?.isConnected) before.focus()
    }
  }, [])

  useKeys(!noting, (event) => {
    if (event.target instanceof HTMLTextAreaElement || event.target instanceof HTMLInputElement) return
    // A focused button takes Enter itself.
    if (event.key === 'Enter' && event.target instanceof HTMLButtonElement) return
    const key = event.key.toLowerCase()
    if (key === 'y' || key === 'enter') onAnswer({ kind: 'allow_once' })
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
      <div
        ref={dialog}
        tabIndex={-1}
        className="dialog permission"
        role="dialog"
        aria-modal="true"
        aria-labelledby="permission-title"
        onKeyDown={(event) => {
          if (event.key !== 'Tab') return
          const items = [...event.currentTarget.querySelectorAll<HTMLElement>('button, textarea, input, [href]')].filter((item) => !item.hasAttribute('disabled'))
          if (items.length === 0) return
          const first = items[0]
          const last = items[items.length - 1]
          const at = document.activeElement
          if (event.shiftKey && (at === first || at === event.currentTarget)) {
            event.preventDefault()
            last.focus()
          } else if (!event.shiftKey && at === last) {
            event.preventDefault()
            first.focus()
          }
        }}
      >
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
                } else if (event.key === 'Escape') {
                  // Esc leaves the note; it does not deny.
                  event.preventDefault()
                  event.stopPropagation()
                  setNoting(false)
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
            <button className="primary" aria-keyshortcuts="Enter" onClick={() => onAnswer({ kind: 'allow_once' })}>
              Allow once <kbd>Enter</kbd>
            </button>
            {sessionAllowed && (
              <button onClick={() => onAnswer({ kind: 'allow_for_session' })} title={request.session_prefixes.join(', ')}>
                Allow <code>{request.session_prefixes.join(', ')}</code> this session <kbd>a</kbd>
              </button>
            )}
            <button className="danger" aria-keyshortcuts="Escape" onClick={() => onAnswer({ kind: 'deny', note: null })}>
              Deny <kbd>Esc</kbd>
            </button>
            <button onClick={() => setNoting(true)}>
              Deny with a note <kbd>e</kbd>
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
