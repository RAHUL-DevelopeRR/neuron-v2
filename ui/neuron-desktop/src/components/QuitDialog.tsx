import { useRef } from 'react'

import { useFocusTrap } from '../a11y'
import type { QuitRequest } from '../platform'

const chats = (count: number) => `${count} ${count === 1 ? 'chat' : 'chats'}`

/** "2 chats are still working. Quit anyway?", counting the chats waiting for the user too. */
export function quitQuestion({ working, waiting }: Pick<QuitRequest, 'working' | 'waiting'>): string {
  const verb = (count: number) => (count === 1 ? 'is' : 'are')
  if (working > 0 && waiting > 0) return `${chats(working)} ${verb(working)} still working and ${waiting} ${verb(waiting)} waiting for you. Quit anyway?`
  if (waiting > 0) return `${chats(waiting)} ${verb(waiting)} waiting for you. Quit anyway?`
  return `${chats(working)} ${verb(working)} still working. Quit anyway?`
}

/**
 * v1.1: the window was closed while chats work or wait for the user. Keep working is
 * the default (Escape too); Quit closes the app and stops their engines.
 */
export function QuitDialog({ request, onAnswer }: { request: QuitRequest; onAnswer: (quit: boolean) => void }) {
  const ref = useRef<HTMLDivElement>(null)
  useFocusTrap(ref)
  return (
    <div className="dialog-backdrop">
      <div
        ref={ref}
        className="dialog small-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="quit-title"
        aria-describedby="quit-body"
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.stopPropagation()
            onAnswer(false)
          }
        }}
      >
        <h2 id="quit-title">Quit Neuron?</h2>
        <p id="quit-body">{quitQuestion(request)}</p>
        <p className="muted small">Quitting stops their work where it is. Their chats can be continued later.</p>
        <div className="dialog-buttons">
          <button data-autofocus onClick={() => onAnswer(false)}>
            Keep working
          </button>
          <button className="danger" onClick={() => onAnswer(true)}>
            Quit
          </button>
        </div>
      </div>
    </div>
  )
}
