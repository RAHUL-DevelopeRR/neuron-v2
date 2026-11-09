import { useEffect, useRef, useState } from 'react'

import { diffLines, additionDiff, toolView, type PendingPermission, type PermissionDecision } from '@neuron/client'

import { useFocusTrap } from '../a11y'
import { DiffView } from './DiffView'

/** The dialog must have had focus this long before a key can answer it. */
const FOCUS_GUARD_MS = 700

/** Whether the user is typing something that a stray key would break into. */
function typingText(element: Element | null): boolean {
  return (element instanceof HTMLTextAreaElement || element instanceof HTMLInputElement) && element.value.trim() !== ''
}

/**
 * A tool call waits for the user. Keys: Enter allows once, Esc denies; while focus is
 * inside the dialog, y allows once, a allows for the session (when the engine offers it),
 * n denies and e denies with a note. No key answers with ctrl, meta or alt held, while it
 * repeats, or in the first 700 ms after the dialog got focus, so typing meant for the
 * composer can't answer it. Focus moves to the dialog's title, unless the composer holds
 * text: then the dialog stays visible and announced, and Tab or a click moves into it.
 * Inside, Tab stays, and focus goes back to where it was on close.
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
  const title = useRef<HTMLHeadingElement>(null)
  // The composer holds text: do not take focus away from it.
  const [deferred, setDeferred] = useState(() => typingText(document.activeElement))
  const readyAt = useRef(Date.now() + FOCUS_GUARD_MS)

  // Focus goes to the title (not a button, so a held Enter can't answer it), is kept inside, and goes back after.
  useFocusTrap(dialog, !deferred)

  const answer = useRef(onAnswer)
  answer.current = onAnswer
  const state = useRef({ noting, sessionAllowed, deferred })
  state.current = { noting, sessionAllowed, deferred }

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const current = state.current
      const root = dialog.current
      if (!root) return
      const inside = event.target instanceof Node && root.contains(event.target)
      // Still holding the composer's text: Tab is the way in.
      if (event.key === 'Tab' && current.deferred && !inside) {
        event.preventDefault()
        setDeferred(false)
        return
      }
      if (current.noting || event.ctrlKey || event.metaKey || event.altKey || event.repeat) return
      if (event.target instanceof HTMLTextAreaElement || event.target instanceof HTMLInputElement) return
      if (Date.now() < readyAt.current) return
      // A focused button takes Enter itself.
      if (event.key === 'Enter' && event.target instanceof HTMLButtonElement) return
      const key = event.key.toLowerCase()
      const bare = key === 'y' || key === 'a' || key === 'n' || key === 'e'
      if (bare && !inside) return
      if ((key === 'enter' || key === 'escape') && current.deferred) return
      if (key === 'y' || key === 'enter') answer.current({ kind: 'allow_once' })
      else if (key === 'a' && current.sessionAllowed) answer.current({ kind: 'allow_for_session' })
      else if (key === 'n' || key === 'escape') answer.current({ kind: 'deny', note: null })
      else if (key === 'e') setNoting(true)
      else return
      event.preventDefault()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  return (
    <div className="dialog-backdrop">
      <div
        ref={dialog}
        className="dialog permission"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="permission-title"
        onFocus={() => {
          // Focus is in the dialog now: its keys count from this moment.
          readyAt.current = Date.now() + FOCUS_GUARD_MS
        }}
        onMouseDown={() => setDeferred(false)}
        onBlur={(event) => {
          if (event.relatedTarget || deferred) return
          // Focus fell to the page (a button went away): bring it back.
          setTimeout(() => {
            if (dialog.current && (document.activeElement === document.body || document.activeElement === null)) title.current?.focus()
          }, 0)
        }}
      >
        <h2 id="permission-title" ref={title} tabIndex={-1} data-autofocus>
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
