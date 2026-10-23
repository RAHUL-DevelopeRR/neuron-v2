import { useRef } from 'react'

import { useFocusTrap } from '../a11y'
import { TRUST_COPY } from '../copy'
import { LINKS } from '../links'

/**
 * Before the agent works in a folder it hasn't been allowed in: blocks the
 * window until the user allows it or opens another folder. `preview` lists
 * what the folder's own settings would turn on, when the engine reports it.
 */
export function TrustModal({
  folder,
  gitRoot,
  preview,
  working,
  canPickFolder,
  onAllow,
  onElsewhere,
  onOpenLink,
}: {
  folder: string
  /** `null` when the engine did not say; equal to `folder` when it is the root. */
  gitRoot: string | null
  preview: string[]
  working: boolean
  canPickFolder: boolean
  onAllow: () => void
  onElsewhere: () => void
  onOpenLink: (url: string) => void
}) {
  const dialog = useRef<HTMLDivElement>(null)
  useFocusTrap(dialog)
  return (
    <div className="dialog-backdrop trust-backdrop">
      <div className="dialog trust-dialog" role="dialog" aria-modal="true" aria-labelledby="trust-title" aria-describedby="trust-access" ref={dialog}>
        <h2 id="trust-title">{TRUST_COPY.title}</h2>
        <div className="trust-folder">
          <code>{folder}</code>
          {gitRoot && <span className="muted small"> (git root: {gitRoot === folder ? 'this folder' : gitRoot})</span>}
        </div>
        <p id="trust-access">{TRUST_COPY.access}</p>
        {preview.length > 0 && (
          <>
            <p>{TRUST_COPY.alsoTurnsOn}</p>
            <ul className="trust-preview">
              {preview.map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
          </>
        )}
        <p className="muted">
          {TRUST_COPY.caution}{' '}
          <a
            href={LINKS.trust}
            onClick={(event) => {
              event.preventDefault()
              onOpenLink(LINKS.trust)
            }}
          >
            {TRUST_COPY.why}
          </a>
        </p>
        <div className="dialog-buttons trust-buttons">
          <button className="primary" onClick={onAllow} disabled={working} data-autofocus>
            {working ? 'Allowing…' : TRUST_COPY.allow}
          </button>
          {canPickFolder && (
            <button onClick={onElsewhere} disabled={working}>
              {TRUST_COPY.elsewhere}
            </button>
          )}
        </div>
      </div>
    </div>
  )
}
