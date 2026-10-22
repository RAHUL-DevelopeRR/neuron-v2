import { useMemo, useState } from 'react'

/** The slash commands the engine runs for a UI today (engine.rs `run_slash`). */
export const ENGINE_COMMANDS: Array<[string, string]> = [
  ['help', 'What Neuron can do'],
  ['status', 'Session, model and context'],
  ['context', 'What fills the context window'],
  ['cost', 'Tokens and cost so far'],
  ['compact', 'Summarise the conversation to free context'],
  ['model', 'Show or switch the model'],
  ['permissions', 'Show or switch the permission mode'],
  ['plan', 'Plan mode: read-only, plan first'],
  ['clear', 'Start a fresh conversation'],
  ['resume', 'Continue a saved chat'],
  ['login', 'Sign in to Zero-X'],
  ['logout', 'Sign out of Zero-X'],
]

export function Composer({
  busy,
  disabled,
  question,
  onSend,
  onInterrupt,
}: {
  busy: boolean
  disabled: boolean
  /** The agent asked something: the next message answers it. */
  question: string | null
  onSend: (text: string) => void
  onInterrupt: () => void
}) {
  const [text, setText] = useState('')
  const [pick, setPick] = useState(0)
  const suggestions = useMemo(() => {
    const match = /^\/(\S*)$/.exec(text)
    if (!match || question) return []
    return ENGINE_COMMANDS.filter(([name]) => name.startsWith(match[1].toLowerCase()))
  }, [text, question])

  const send = () => {
    const value = text.trim()
    if (!value || disabled) return
    onSend(value)
    setText('')
    setPick(0)
  }

  return (
    <div className="composer">
      {suggestions.length > 0 && (
        <ul className="suggestions" role="listbox">
          {suggestions.map(([name, help], index) => (
            <li
              key={name}
              role="option"
              aria-selected={index === pick}
              className={index === pick ? 'active' : ''}
              onMouseDown={(event) => {
                event.preventDefault()
                setText(`/${name} `)
              }}
            >
              <span className="command">/{name}</span> <span className="muted">{help}</span>
            </li>
          ))}
        </ul>
      )}
      {question && <div className="composer-hint">Answering Neuron's question</div>}
      <div className="composer-row">
        <textarea
          aria-label="Message Neuron"
          placeholder={disabled ? 'Neuron is not connected' : question ? 'Your answer…' : 'Ask Neuron to do something…'}
          value={text}
          disabled={disabled}
          rows={Math.min(8, Math.max(2, text.split('\n').length))}
          onChange={(event) => {
            setText(event.target.value)
            setPick(0)
          }}
          onKeyDown={(event) => {
            if (suggestions.length > 0 && (event.key === 'ArrowDown' || event.key === 'ArrowUp')) {
              event.preventDefault()
              setPick((pick + (event.key === 'ArrowDown' ? 1 : suggestions.length - 1)) % suggestions.length)
            } else if (suggestions.length > 0 && event.key === 'Tab') {
              event.preventDefault()
              setText(`/${suggestions[pick][0]} `)
            } else if (event.key === 'Enter' && !event.shiftKey) {
              event.preventDefault()
              if (suggestions.length > 0 && text !== `/${suggestions[pick][0]}`) setText(`/${suggestions[pick][0]} `)
              else send()
            } else if (event.key === 'Escape' && busy) {
              onInterrupt()
            }
          }}
        />
        {busy ? (
          <button className="stop" onClick={onInterrupt} title="Interrupt (Esc)">
            Stop
          </button>
        ) : (
          <button className="primary" onClick={send} disabled={disabled || !text.trim()}>
            Send
          </button>
        )}
      </div>
    </div>
  )
}
