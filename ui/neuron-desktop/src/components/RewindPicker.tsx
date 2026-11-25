import { useEffect, useRef, useState } from 'react'

import { type CheckpointInfo, type RewindMode, type RewoundEvent, type SessionController, type SessionState } from '@neuron/client'

import { useFocusTrap } from '../a11y'

type RewindController = Pick<SessionController, 'requestCheckpoints' | 'rewind'>

const MODES: Array<[RewindMode, string, string]> = [
  ['code_and_chat', 'Code and chat', 'Put the files back and take the chat back to that point.'],
  ['chat_only', 'Chat only', 'Take the chat back to that point. Your files stay as they are.'],
  ['code_only', 'Code only', 'Put the files back. The chat stays as it is.'],
]

const when = (ms: number) => new Date(ms).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' })
const plural = (count: number, word: string) => `${count} ${word}${count === 1 ? '' : 's'}`

function FileList({ title, files }: { title: string; files: string[] }) {
  if (files.length === 0) return null
  return (
    <section className="rewind-block">
      <h4>{title}</h4>
      <ul>
        {files.map((file) => (
          <li key={file}>
            <code>{file}</code>
          </li>
        ))}
      </ul>
    </section>
  )
}

/** What a rewind did or would do, in plain words. */
function Outcome({ result }: { result: RewoundEvent }) {
  return (
    <>
      <FileList title={result.dry_run ? 'These files will be put back' : 'Put back'} files={result.restored} />
      <FileList title={result.dry_run ? 'These files will be removed (they did not exist then)' : 'Removed (they did not exist then)'} files={result.removed} />
      {result.restored.length === 0 && result.removed.length === 0 && result.not_restored.length === 0 && (
        <p className="muted">No files change.</p>
      )}
      {result.not_restored.length > 0 && (
        <section className="rewind-block">
          <h4>{result.dry_run ? "These files can't be put back" : "These files couldn't be put back"}</h4>
          <ul>
            {result.not_restored.map((file) => (
              <li key={file.path}>
                <code>{file.path}</code> because {file.reason}
              </li>
            ))}
          </ul>
        </section>
      )}
      {result.shell_commands.length > 0 && (
        <section className="rewind-block">
          <h4>Commands that can&apos;t be undone</h4>
          <p className="muted small">Neuron ran these since then. What they did stays.</p>
          <ul>
            {result.shell_commands.map((command, index) => (
              <li key={index}>
                <code>{command}</code>
              </li>
            ))}
          </ul>
        </section>
      )}
      {result.dry_run && result.removed_turns > 0 && (
        <p className="muted">The chat goes back too: the last {plural(result.removed_turns, 'message')} and everything after {result.removed_turns === 1 ? 'it' : 'them'} are dropped.</p>
      )}
    </>
  )
}

/**
 * Go back to an earlier point of this chat. A preview (a dry run) comes first;
 * only the confirm after it changes anything.
 */
export function RewindPicker({ state, controller, onClose }: { state: SessionState; controller: RewindController; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null)
  useFocusTrap(ref)
  const [pick, setPick] = useState<number | null>(null)
  const [mode, setMode] = useState<RewindMode>('code_and_chat')
  /** What the last request was, and the answer that was there before it. */
  const [asked, setAsked] = useState<{ id: number; mode: RewindMode; dry: boolean; before: RewoundEvent | null } | null>(null)

  useEffect(() => {
    controller.requestCheckpoints()
    // Once, when the picker opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const answer = asked && state.rewound !== asked.before && state.rewound?.checkpoint === asked.id && state.rewound.mode === asked.mode && state.rewound.dry_run === asked.dry ? state.rewound : null
  const done = answer && !answer.dry_run ? answer : null
  const preview = answer?.dry_run && answer.checkpoint === pick && answer.mode === mode ? answer : null
  const points: CheckpointInfo[] = state.checkpoints ?? []
  const chosen = points.find((point) => point.id === pick) ?? null

  const send = (dry: boolean) => {
    if (pick === null) return
    setAsked({ id: pick, mode, dry, before: state.rewound })
    controller.rewind(pick, mode, dry)
  }

  return (
    <div className="dialog-backdrop">
      <div
        ref={ref}
        className="dialog rewind-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="rewind-title"
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.stopPropagation()
            onClose()
          }
        }}
      >
        <h2 id="rewind-title">Restore point</h2>
        {/* Always there, so a screen reader hears the text when it arrives. */}
        <div role="status" className={done ? 'rewind-summary' : 'sr-only'}>
          {done && (
            <p>
              Done. Put back {plural(done.restored.length, 'file')}, removed {done.removed.length}.
              {done.prompt ? ' Your message is back in the message box, ready to edit and send again.' : ''}
            </p>
          )}
        </div>
        {done ? (
          <>
            <Outcome result={done} />
            <div className="dialog-buttons">
              <button className="primary" data-autofocus onClick={onClose}>
                Close
              </button>
            </div>
          </>
        ) : (
          <>
            {state.busy && <p className="muted">You can go back once the agent is idle.</p>}
            {state.checkpoints === null && <p className="muted">Loading restore points…</p>}
            {state.checkpoints !== null && points.length === 0 && <p className="muted">No restore points yet. Neuron saves one each time you send a message.</p>}
            {points.length > 0 && (
              <div role="radiogroup" aria-label="Restore point" className="rewind-points">
                {points.map((point) => (
                  <label key={point.id} className={`rewind-point${pick === point.id ? ' selected' : ''}`}>
                    <input type="radio" name="restore-point" checked={pick === point.id} onChange={() => setPick(point.id)} />
                    <span className="rewind-point-body">
                      <span className="rewind-prompt">{point.prompt_preview || '(no text)'}</span>
                      <span className="muted small">
                        {when(point.created_at_ms)} · {plural(point.files.length, 'file')} changed
                        {point.shell_commands.length > 0 && ` · ${plural(point.shell_commands.length, 'command')} run`}
                      </span>
                      {point.files.length > 0 && <span className="muted small">{point.files.join(', ')}</span>}
                      {point.shell_commands.length > 0 && <span className="muted small">ran {point.shell_commands.join(', ')}</span>}
                    </span>
                  </label>
                ))}
              </div>
            )}
            {points.length > 0 && (
              <fieldset className="rewind-modes">
                <legend>What to go back</legend>
                {MODES.map(([id, label, hint]) => (
                  <label key={id} className="rewind-mode">
                    <input type="radio" name="restore-mode" checked={mode === id} onChange={() => setMode(id)} />
                    <span>
                      <strong>{label}</strong> <span className="muted small">{hint}</span>
                    </span>
                  </label>
                ))}
              </fieldset>
            )}
            {preview && (
              <section className="rewind-preview" aria-label="What would change">
                <Outcome result={preview} />
              </section>
            )}
            {chosen && !preview && !state.busy && <p className="muted small">Preview first to see what would change.</p>}
            <div className="dialog-buttons">
              <button onClick={() => send(true)} disabled={pick === null || state.busy}>
                Preview
              </button>
              <button className="primary" onClick={() => send(false)} disabled={!preview || state.busy}>
                Restore
              </button>
              <button onClick={onClose}>Cancel</button>
            </div>
          </>
        )}
      </div>
    </div>
  )
}
