import { useRef } from 'react'

import { TRUST_DOCS_URL, trustCopy, trustPreviewLines, trustRootNote, type TrustPreviewEvent } from '@neuron/client'

import { useFocusTrap } from '../a11y'

/**
 * Before the agent works in a folder it hasn't been allowed in: blocks the
 * window until the user allows it or opens another folder. The wording is the
 * terminal's too (`trustCopy`); `preview`, when the engine sends one, lists
 * what the folder's own settings would turn on.
 */
export function TrustModal({
  folder,
  preview,
  working,
  canPickFolder,
  onAllow,
  onElsewhere,
  onOpenLink,
}: {
  folder: string
  /** `null` before the engine answered, or from engines without `trust_preview`. */
  preview: TrustPreviewEvent | null
  working: boolean
  canPickFolder: boolean
  onAllow: () => void
  onElsewhere: () => void
  onOpenLink: (url: string) => void
}) {
  const dialog = useRef<HTMLDivElement>(null)
  useFocusTrap(dialog)
  const lines = preview ? trustPreviewLines(preview) : []
  const note = preview ? trustRootNote(preview) : ''
  return (
    <div className="dialog-backdrop trust-backdrop">
      <div className="dialog trust-dialog" role="dialog" aria-modal="true" aria-labelledby="trust-title" aria-describedby="trust-access" ref={dialog}>
        <h2 id="trust-title">{trustCopy.title}</h2>
        <div className="trust-folder">
          <code>{folder}</code>
          {note && <span className="muted small"> {note}</span>}
        </div>
        <p id="trust-access">{trustCopy.access}</p>
        {preview &&
          (lines.length > 0 ? (
            <>
              <p>{trustCopy.turnsOnHeading}</p>
              <ul className="trust-preview" aria-label="What this folder turns on">
                {lines.map((line) => (
                  <li key={line}>{line}</li>
                ))}
              </ul>
            </>
          ) : (
            <p className="muted">{trustCopy.turnsOnNothing}</p>
          ))}
        {preview?.session_only && <p className="muted">{trustCopy.sessionOnly}</p>}
        <p className="muted">
          {trustCopy.caution} {trustCopy.whyLabel}{' '}
          <a
            href={TRUST_DOCS_URL}
            onClick={(event) => {
              event.preventDefault()
              onOpenLink(TRUST_DOCS_URL)
            }}
          >
            {trustCopy.whyLink}
          </a>
        </p>
        <div className="dialog-buttons trust-buttons">
          <button className="primary" onClick={onAllow} disabled={working} data-autofocus>
            {working ? trustCopy.restarting : trustCopy.allow}
          </button>
          {canPickFolder && (
            <button onClick={onElsewhere} disabled={working}>
              {trustCopy.openOther}
            </button>
          )}
        </div>
      </div>
    </div>
  )
}
