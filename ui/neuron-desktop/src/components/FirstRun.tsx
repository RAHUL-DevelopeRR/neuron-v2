import { useEffect, useRef, useState, type ReactNode } from 'react'

import { useFocusTrap } from '../a11y'
import type { Project } from '../platform'
import { PixelLogo } from './PixelLogo'

type Step = 'welcome' | 'sign-in' | 'folder'

/**
 * The first launch: a welcome, the Zero-X sign-in when the engine needs one,
 * then the folder to work in. Trust and the starter prompts follow in the
 * window itself.
 */
export function FirstRun({
  needsSignIn,
  signIn,
  current,
  recent,
  canPickFolder,
  onPickFolder,
  onOpenFolder,
  onDone,
}: {
  needsSignIn: boolean
  /** The sign-in panel, shown while `needsSignIn`. */
  signIn: ReactNode
  current: Project | null
  recent: Project[]
  canPickFolder: boolean
  onPickFolder: () => Promise<string | null>
  onOpenFolder: (path: string) => Promise<void>
  onDone: () => void
}) {
  const [step, setStep] = useState<Step>('welcome')
  const [error, setError] = useState<string | null>(null)
  const [opening, setOpening] = useState(false)
  const dialog = useRef<HTMLDivElement>(null)
  useFocusTrap(dialog)

  // Once signed in, the sign-in step is done.
  useEffect(() => {
    if (step === 'sign-in' && !needsSignIn) setStep('folder')
  }, [step, needsSignIn])

  // Focus the new step's first control.
  useEffect(() => {
    dialog.current?.querySelector<HTMLElement>('[data-autofocus]')?.focus()
  }, [step])

  const open = async (path: string | null) => {
    if (!path) return
    setError(null)
    setOpening(true)
    try {
      if (path !== current?.path) await onOpenFolder(path)
      onDone()
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    } finally {
      setOpening(false)
    }
  }

  const others = recent.filter((project) => project.path !== current?.path).slice(0, 4)
  return (
    <div className="first-run" role="dialog" aria-modal="true" aria-labelledby="first-run-title" ref={dialog}>
      <div className="first-run-card">
        <PixelLogo pixel={6} />
        <ol className="steps" aria-label="Setup steps">
          {(['welcome', 'sign-in', 'folder'] as Step[]).map((name, index) => (
            <li key={name} aria-current={step === name ? 'step' : undefined} className={step === name ? 'active' : ''}>
              {index + 1}. {name === 'welcome' ? 'Welcome' : name === 'sign-in' ? 'Sign in' : 'Folder'}
            </li>
          ))}
        </ol>
        {step === 'welcome' && (
          <>
            <h2 id="first-run-title">Welcome to Neuron</h2>
            <p>Neuron is a coding agent that works in your project folders: it reads, edits and runs code, and asks before anything risky.</p>
            <div className="dialog-buttons">
              <button className="primary" data-autofocus onClick={() => setStep(needsSignIn ? 'sign-in' : 'folder')}>
                Get started
              </button>
              <button onClick={onDone}>Skip</button>
            </div>
          </>
        )}
        {step === 'sign-in' && (
          <>
            <h2 id="first-run-title">Sign in</h2>
            {signIn}
          </>
        )}
        {step === 'folder' && (
          <>
            <h2 id="first-run-title">Which folder should Neuron work in?</h2>
            <p className="muted">You can switch folders at any time from Projects.</p>
            <div className="folder-choices">
              {current && (
                <button className="primary" data-autofocus disabled={opening} onClick={() => void open(current.path)}>
                  Use {current.name}
                  <span className="muted small folder-path">{current.path}</span>
                </button>
              )}
              {canPickFolder && (
                <button disabled={opening} onClick={() => void onPickFolder().then(open)} data-autofocus={current ? undefined : true}>
                  Open folder…
                </button>
              )}
              {others.map((project) => (
                <button key={project.path} disabled={opening} onClick={() => void open(project.path)}>
                  {project.name}
                  <span className="muted small folder-path">{project.path}</span>
                </button>
              ))}
            </div>
            {error && (
              <div className="error-line" role="alert">
                {error}
              </div>
            )}
          </>
        )}
      </div>
    </div>
  )
}
