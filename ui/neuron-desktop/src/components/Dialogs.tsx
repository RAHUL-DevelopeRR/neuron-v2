import { useEffect, useState, type ReactNode } from 'react'

import type { Project } from '../platform'
import type { Settings } from '../settings'

function Modal({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => event.key === 'Escape' && onClose()
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])
  return (
    <div className="dialog-backdrop" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <div className="dialog" role="dialog" aria-modal="true" aria-label={title}>
        <div className="dialog-title">
          <h2>{title}</h2>
          <button className="icon-button" aria-label="Close" onClick={onClose}>
            ✕
          </button>
        </div>
        {children}
      </div>
    </div>
  )
}

/** Open a project folder: one of the recent ones, or a path. */
export function ProjectDialog({
  current,
  recent,
  onOpen,
  onClose,
}: {
  current: Project | null
  recent: Project[]
  onOpen: (path: string) => Promise<void>
  onClose: () => void
}) {
  const [path, setPath] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [opening, setOpening] = useState(false)
  const open = async (target: string) => {
    if (!target.trim()) return
    setOpening(true)
    setError(null)
    try {
      await onOpen(target.trim())
      onClose()
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    } finally {
      setOpening(false)
    }
  }
  return (
    <Modal title="Open a project" onClose={onClose}>
      {recent.length > 0 && (
        <ul className="project-list">
          {recent.map((project) => (
            <li key={project.path}>
              <button
                className={`project-row${project.path === current?.path ? ' current' : ''}`}
                disabled={opening || project.path === current?.path}
                onClick={() => void open(project.path)}
              >
                <strong>{project.name}</strong>
                <span className="muted">{project.path}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      <form
        className="path-form"
        onSubmit={(event) => {
          event.preventDefault()
          void open(path)
        }}
      >
        <input
          autoFocus
          aria-label="Project folder"
          placeholder="C:\\path\\to\\project or /home/you/project"
          value={path}
          onChange={(event) => setPath(event.target.value)}
        />
        <button className="primary" type="submit" disabled={opening || !path.trim()}>
          {opening ? 'Opening…' : 'Open'}
        </button>
      </form>
      {error && (
        <div className="error-line" role="alert">
          {error}
        </div>
      )}
      <p className="muted small">Neuron starts a fresh chat in the folder you open; its earlier chats are in the sidebar.</p>
    </Modal>
  )
}

export function SettingsDialog({
  settings,
  onChange,
  onClose,
  version,
  where,
}: {
  settings: Settings
  onChange: (change: Partial<Settings>) => void
  onClose: () => void
  version: string | null
  where: string
}) {
  return (
    <Modal title="Settings" onClose={onClose}>
      <div className="settings">
        <label>
          Theme
          <select
            aria-label="Theme"
            value={settings.theme}
            onChange={(event) => onChange({ theme: event.target.value as Settings['theme'] })}
          >
            <option value="dark">Dark</option>
            <option value="light">Light</option>
            <option value="system">Same as the system</option>
          </select>
        </label>
        <label className="check">
          <input type="checkbox" checked={settings.enterSends} onChange={(event) => onChange({ enterSends: event.target.checked })} />
          Enter sends (Shift+Enter for a new line). Off: Ctrl+Enter sends.
        </label>
        <label className="check">
          <input type="checkbox" checked={settings.showReasoning} onChange={(event) => onChange({ showReasoning: event.target.checked })} />
          Show the model's reasoning when the provider streams it
        </label>
        <label className="check">
          <input type="checkbox" checked={settings.foldTools} onChange={(event) => onChange({ foldTools: event.target.checked })} />
          Fold tool calls (edits and failures stay open)
        </label>
        <p className="muted small">
          Neuron engine {version ?? 'not connected'} on {where}. Models, providers and permissions come from your Neuron settings; the
          model and mode menus under the message box change them for this chat.
        </p>
      </div>
    </Modal>
  )
}
