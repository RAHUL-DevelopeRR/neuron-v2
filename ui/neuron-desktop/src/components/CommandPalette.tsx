import { useEffect, useMemo, useRef, useState } from 'react'

import { useFocusTrap } from '../a11y'
import { rankCommands, type PaletteCommand } from '../palette'
import { OTHER_SHORTCUTS, SHORTCUTS, displayKeys, isMac, keysOf } from '../shortcuts'

/** Type to find a chat, a session or an action; Enter runs it. */
export function CommandPalette({ open, onClose, commands }: { open: boolean; onClose: () => void; commands: PaletteCommand[] }) {
  if (!open) return null
  return <PaletteBody onClose={onClose} commands={commands} />
}

function PaletteBody({ onClose, commands }: { onClose: () => void; commands: PaletteCommand[] }) {
  const dialog = useRef<HTMLDivElement>(null)
  useFocusTrap(dialog)
  const [query, setQuery] = useState('')
  const [index, setIndex] = useState(0)
  const shown = useMemo(() => rankCommands(commands, query).slice(0, 50), [commands, query])
  useEffect(() => setIndex(0), [query])
  const current = shown[Math.min(index, shown.length - 1)]
  // The arrows can go past the visible rows: the chosen one is kept in view.
  useEffect(() => {
    if (current) document.getElementById(`palette-${current.id}`)?.scrollIntoView?.({ block: 'nearest' })
  }, [current])

  const run = (command: PaletteCommand | undefined) => {
    if (!command) return
    onClose()
    command.run()
  }

  return (
    <div className="dialog-backdrop palette-backdrop" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <div
        className="dialog palette"
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        ref={dialog}
        onKeyDown={(event) => {
          if (event.key === 'Escape') onClose()
          else if (event.key === 'Enter' && (event.nativeEvent.isComposing || event.keyCode === 229)) return
          else if (event.key === 'ArrowDown') {
            event.preventDefault()
            setIndex((value) => Math.min(value + 1, shown.length - 1))
          } else if (event.key === 'ArrowUp') {
            event.preventDefault()
            setIndex((value) => Math.max(value - 1, 0))
          } else if (event.key === 'Enter') {
            event.preventDefault()
            run(current)
          }
        }}
      >
        <input
          className="palette-input"
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-list"
          aria-autocomplete="list"
          aria-activedescendant={current ? `palette-${current.id}` : undefined}
          aria-label="Type a command, chat or session"
          placeholder="Type a command, chat or session"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          data-autofocus
        />
        {shown.length === 0 && (
          <p className="muted pad small" role="status">
            Nothing matches.
          </p>
        )}
        <ul id="palette-list" role="listbox" aria-label="Results" className="palette-list" hidden={shown.length === 0}>
          {shown.map((command, position) => (
            <li
              key={command.id}
              id={`palette-${command.id}`}
              role="option"
              aria-selected={command === current}
              className={`palette-item${command === current ? ' selected' : ''}`}
              onMouseMove={() => setIndex(position)}
              onClick={() => run(command)}
            >
              <span className="palette-group muted small">{command.group}</span>
              <span className="palette-title">{command.title}</span>
              {command.keys && <kbd>{command.keys}</kbd>}
            </li>
          ))}
        </ul>
      </div>
    </div>
  )
}

/** Every keyboard shortcut, from the same table the key handler reads. */
export function ShortcutsSheet({ open, onClose }: { open: boolean; onClose: () => void }) {
  const dialog = useRef<HTMLDivElement>(null)
  useFocusTrap(dialog, open)
  if (!open) return null
  const mac = isMac()
  return (
    <div className="dialog-backdrop" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <div
        className="dialog small-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="shortcuts-title"
        ref={dialog}
        onKeyDown={(event) => event.key === 'Escape' && onClose()}
      >
        <div className="dialog-title">
          <h2 id="shortcuts-title">Keyboard shortcuts</h2>
          <button className="icon-button" aria-label="Close" onClick={onClose} data-autofocus>
            ✕
          </button>
        </div>
        <dl className="shortcut-list">
          {SHORTCUTS.map((shortcut) => (
            <div key={shortcut.id} className="shortcut-row">
              <dt>{shortcut.title}</dt>
              <dd>
                <kbd>{keysOf(shortcut, mac)}</kbd>
              </dd>
            </div>
          ))}
          {OTHER_SHORTCUTS.map((shortcut) => (
            <div key={shortcut.keys} className="shortcut-row">
              <dt>{shortcut.title}</dt>
              <dd>
                <kbd>{displayKeys(shortcut.keys, mac)}</kbd>
              </dd>
            </div>
          ))}
        </dl>
      </div>
    </div>
  )
}
