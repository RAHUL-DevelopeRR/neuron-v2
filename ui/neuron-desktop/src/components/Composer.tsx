import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'

import type { Attachment } from '@neuron/client'

/** The slash commands the engine runs for a UI (engine.rs `run_slash`). */
export const ENGINE_COMMANDS: Array<[string, string]> = [
  ['help', 'What Neuron can do'],
  ['status', 'Session, model and context'],
  ['context', 'What fills the context window'],
  ['cost', 'Tokens and cost so far'],
  ['compact', 'Summarise the conversation to free context'],
  ['model', 'Show or switch the model'],
  ['permissions', 'Show or switch the permission mode'],
  ['plan', 'Plan mode: read-only, plan first'],
  ['clear', 'Start a fresh chat (--confirm)'],
  ['resume', 'Continue a saved chat'],
  ['login', 'Sign in to Zero-X'],
  ['logout', 'Sign out of Zero-X'],
]

/** The permission modes, as the mode menu offers them. */
export const MODES: Array<{ id: string; label: string; help: string }> = [
  { id: 'read-only', label: 'Read only', help: 'Reads and searches; asks before any change' },
  { id: 'workspace-write', label: 'Edit files', help: 'Edits in this project; asks before commands that reach further' },
  { id: 'danger-full-access', label: 'Full access', help: 'Runs anything without asking. Use with care' },
]

type Suggestion = { kind: 'command'; value: string; help: string } | { kind: 'file'; value: string }

/** The `@word` being typed at the caret, with where it starts. */
function mentionAt(text: string, caret: number): { start: number; query: string } | null {
  const before = text.slice(0, caret)
  const match = /(^|\s)@([^\s@]*)$/.exec(before)
  return match ? { start: caret - match[2].length - 1, query: match[2] } : null
}

export function Composer({
  busy,
  disabled,
  questionWaiting,
  enterSends,
  searchFiles,
  toolbar,
  onSend,
  onInterrupt,
}: {
  busy: boolean
  disabled: boolean
  /** The agent asked something: the next message answers it. */
  questionWaiting: boolean
  enterSends: boolean
  searchFiles: (query: string) => Promise<string[]>
  /** The mode and model menus. */
  toolbar?: ReactNode
  onSend: (text: string, attachments: Attachment[]) => void
  onInterrupt: () => void
}) {
  const [text, setText] = useState('')
  const [caret, setCaret] = useState(0)
  const [files, setFiles] = useState<string[]>([])
  const [found, setFound] = useState<string[]>([])
  const [pick, setPick] = useState(0)
  const box = useRef<HTMLTextAreaElement>(null)
  const mention = mentionAt(text, caret)

  useEffect(() => {
    if (!mention) {
      setFound([])
      return
    }
    let cancelled = false
    const timer = setTimeout(() => {
      searchFiles(mention.query).then(
        (paths) => !cancelled && setFound(paths.slice(0, 8)),
        () => !cancelled && setFound([]),
      )
    }, 120)
    return () => {
      cancelled = true
      clearTimeout(timer)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mention?.query, mention?.start, searchFiles])

  const suggestions = useMemo<Suggestion[]>(() => {
    if (mention) return found.map((value) => ({ kind: 'file', value }))
    const command = /^\/(\S*)$/.exec(text)
    if (!command || questionWaiting) return []
    return ENGINE_COMMANDS.filter(([name]) => name.startsWith(command[1].toLowerCase())).map(([value, help]) => ({
      kind: 'command',
      value,
      help,
    }))
  }, [mention, found, text, questionWaiting])

  const choose = (suggestion: Suggestion) => {
    if (suggestion.kind === 'command') {
      setText(`/${suggestion.value} `)
    } else if (mention) {
      const next = `${text.slice(0, mention.start)}@${suggestion.value} ${text.slice(caret)}`
      setText(next)
      setFiles((current) => (current.includes(suggestion.value) ? current : [...current, suggestion.value]))
      const at = mention.start + suggestion.value.length + 2
      setCaret(at)
      requestAnimationFrame(() => box.current?.setSelectionRange(at, at))
    }
    setPick(0)
    box.current?.focus()
  }

  const send = () => {
    const value = text.trim()
    if (!value || disabled) return
    // Only the files still mentioned in the text go along.
    const attachments: Attachment[] = files.filter((path) => value.includes(`@${path}`)).map((path) => ({ kind: 'file', path }))
    onSend(value, attachments)
    setText('')
    setFiles([])
    setCaret(0)
    setPick(0)
  }

  return (
    <div className="composer">
      {suggestions.length > 0 && (
        <ul className="suggestions" role="listbox" aria-label={mention ? 'Files' : 'Commands'}>
          {suggestions.map((suggestion, index) => (
            <li
              key={suggestion.value}
              role="option"
              aria-selected={index === pick}
              className={index === pick ? 'active' : ''}
              onMouseDown={(event) => {
                event.preventDefault()
                choose(suggestion)
              }}
            >
              {suggestion.kind === 'command' ? (
                <>
                  <span className="command">/{suggestion.value}</span> <span className="muted">{suggestion.help}</span>
                </>
              ) : (
                <span className="command">@{suggestion.value}</span>
              )}
            </li>
          ))}
        </ul>
      )}
      {questionWaiting && <div className="composer-hint">Answering Neuron's question</div>}
      <div className="composer-box">
        <textarea
          ref={box}
          aria-label="Message Neuron"
          placeholder={
            disabled
              ? 'Neuron is not connected'
              : questionWaiting
                ? 'Your answer…'
                : 'Ask Neuron to build, fix or explain…  @ adds a file, / runs a command'
          }
          value={text}
          disabled={disabled}
          rows={Math.min(10, Math.max(2, text.split('\n').length))}
          onChange={(event) => {
            setText(event.target.value)
            setCaret(event.target.selectionStart ?? event.target.value.length)
            setPick(0)
          }}
          onSelect={(event) => setCaret(event.currentTarget.selectionStart ?? 0)}
          onKeyDown={(event) => {
            if (suggestions.length > 0 && (event.key === 'ArrowDown' || event.key === 'ArrowUp')) {
              event.preventDefault()
              setPick((pick + (event.key === 'ArrowDown' ? 1 : suggestions.length - 1)) % suggestions.length)
              return
            }
            if (suggestions.length > 0 && (event.key === 'Tab' || (event.key === 'Enter' && !event.shiftKey))) {
              const chosen = suggestions[pick]
              // A command typed out in full runs on Enter; anything else completes.
              if (!(event.key === 'Enter' && chosen.kind === 'command' && text === `/${chosen.value}`)) {
                event.preventDefault()
                choose(chosen)
                return
              }
            }
            const sends = enterSends
              ? event.key === 'Enter' && !event.shiftKey
              : event.key === 'Enter' && (event.ctrlKey || event.metaKey)
            if (sends) {
              event.preventDefault()
              send()
            } else if (event.key === 'Escape' && busy) {
              onInterrupt()
            }
          }}
        />
        {files.length > 0 && (
          <div className="chips" aria-label="Attached files">
            {files.map((path) => (
              <span key={path} className="chip">
                @{path}
                <button
                  aria-label={`Remove ${path}`}
                  onClick={() => {
                    setFiles(files.filter((file) => file !== path))
                    setText(text.replace(`@${path} `, '').replace(`@${path}`, ''))
                  }}
                >
                  ✕
                </button>
              </span>
            ))}
          </div>
        )}
        <div className="composer-bar">
          {toolbar}
          <div className="spacer" />
          {busy ? (
            <button className="stop" onClick={onInterrupt} title="Interrupt (Esc)">
              ■ Stop
            </button>
          ) : (
            <button className="primary" onClick={send} disabled={disabled || !text.trim()}>
              Send
            </button>
          )}
        </div>
      </div>
    </div>
  )
}

/** The permission mode and plan mode, in one menu. */
export function ModeMenu({
  mode,
  planMode,
  disabled,
  onMode,
  onPlan,
}: {
  mode: string
  planMode: boolean
  disabled: boolean
  onMode: (mode: string) => void
  onPlan: (on: boolean) => void
}) {
  const [open, setOpen] = useState(false)
  const current = planMode ? 'Plan' : (MODES.find((entry) => entry.id === mode)?.label ?? mode)
  return (
    <div className="menu-anchor">
      <button
        className={`mode-button${mode === 'danger-full-access' ? ' risky' : ''}${planMode ? ' plan' : ''}`}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label="Permission mode"
        disabled={disabled}
        onClick={() => setOpen(!open)}
      >
        {current} ▾
      </button>
      {open && (
        <div className="menu up" role="listbox" aria-label="Permission modes" onMouseLeave={() => setOpen(false)}>
          {MODES.map((entry) => (
            <button
              key={entry.id}
              role="option"
              aria-selected={!planMode && entry.id === mode}
              className={`menu-item${!planMode && entry.id === mode ? ' current' : ''}`}
              onClick={() => {
                setOpen(false)
                if (planMode) onPlan(false)
                if (entry.id !== mode) onMode(entry.id)
              }}
            >
              <span>{entry.label}</span>
              <span className="muted">{entry.help}</span>
            </button>
          ))}
          <button
            role="option"
            aria-selected={planMode}
            className={`menu-item${planMode ? ' current' : ''}`}
            onClick={() => {
              setOpen(false)
              onPlan(!planMode)
            }}
          >
            <span>Plan {planMode ? '(on)' : ''}</span>
            <span className="muted">Read-only: Neuron plans first, then you ask it to build</span>
          </button>
        </div>
      )}
    </div>
  )
}
