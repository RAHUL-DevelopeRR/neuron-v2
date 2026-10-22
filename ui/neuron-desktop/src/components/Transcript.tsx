import { useEffect, useRef, useState } from 'react'

import type { ToolState, TranscriptItem } from '@neuron/client'

import { DiffView } from './DiffView'
import { Markdown } from './Markdown'

type ToolItem = Extract<TranscriptItem, { kind: 'tool' }>

const STATE_LABEL: Record<ToolState, string> = {
  running: 'running',
  awaiting_permission: 'waiting for you',
  done: 'done',
  error: 'failed',
  denied: 'denied',
  cut_off: 'cut off',
}

export function ToolCard({ item, onSelect, selected }: { item: ToolItem; onSelect?: (item: ToolItem) => void; selected?: boolean }) {
  const { view } = item
  const [open, setOpen] = useState(view.kind === 'edit' || view.kind === 'write' || item.state === 'error')
  return (
    <div className={`tool-card tool-${view.kind} state-${item.state}${selected ? ' selected' : ''}`} data-testid="tool-card">
      <button className="tool-head" onClick={() => setOpen(!open)} aria-expanded={open}>
        <span className="tool-dot" aria-hidden />
        <span className="tool-verb">{view.verb}</span>
        <span className="tool-target" title={view.target}>
          {view.target}
        </span>
        <span className="tool-state">{STATE_LABEL[item.state]}</span>
      </button>
      {open && (
        <div className="tool-body">
          {view.kind === 'terminal' && <pre className="tool-command">$ {view.target}</pre>}
          {view.diff && view.diff.lines.length > 0 && <DiffView diff={view.diff} label={`changes to ${view.target}`} />}
          {view.todos && (
            <ul className="todos">
              {view.todos.map((todo, index) => (
                <li key={index} className={`todo-${todo.status}`}>
                  {todo.status === 'completed' ? '✓' : todo.status === 'in_progress' ? '▸' : '○'} {todo.text}
                </li>
              ))}
            </ul>
          )}
          {view.output && view.kind !== 'write' && <pre className="tool-output">{view.output}</pre>}
          {view.output && view.kind === 'write' && <div className="muted">{view.output}</div>}
          {onSelect && (
            <button className="link-button" onClick={() => onSelect(item)}>
              Open in the side panel
            </button>
          )}
        </div>
      )}
    </div>
  )
}

export function Transcript({
  items,
  busy,
  onLink,
  onSelectTool,
  selectedTool,
}: {
  items: TranscriptItem[]
  busy: boolean
  onLink?: (url: string) => void
  onSelectTool?: (item: ToolItem) => void
  selectedTool?: string | null
}) {
  const end = useRef<HTMLDivElement>(null)
  const scroller = useRef<HTMLDivElement>(null)
  const [pinned, setPinned] = useState(true)
  useEffect(() => {
    if (pinned) end.current?.scrollIntoView?.({ block: 'end' })
  }, [items, pinned])
  return (
    <div
      className="transcript"
      ref={scroller}
      onScroll={(event) => {
        const el = event.currentTarget
        setPinned(el.scrollHeight - el.scrollTop - el.clientHeight < 40)
      }}
      role="log"
      aria-live="polite"
    >
      {items.length === 0 && (
        <div className="empty">
          <p>Ask Neuron to build, fix or explain something in this workspace.</p>
          <p className="muted">Enter sends. Shift+Enter adds a line. Type / for commands.</p>
        </div>
      )}
      {items.map((item) => {
        switch (item.kind) {
          case 'user':
            return (
              <div key={item.key} className="message user" data-testid="user-message">
                <div className="message-text">{item.text}</div>
                {item.attachments.length > 0 && (
                  <div className="attachments">{item.attachments.map((a) => a.path).join(', ')}</div>
                )}
              </div>
            )
          case 'assistant':
            return (
              <div key={item.key} className={`message assistant${item.streaming ? ' streaming' : ''}`} data-testid="assistant-message">
                <Markdown text={item.text} onLink={onLink} />
              </div>
            )
          case 'reasoning':
            return (
              <details key={item.key} className="reasoning">
                <summary>{item.redacted ? 'Reasoning (hidden by the provider)' : item.streaming ? 'Thinking…' : 'Reasoning'}</summary>
                {item.text && <pre>{item.text}</pre>}
              </details>
            )
          case 'tool':
            return <ToolCard key={item.key} item={item} onSelect={onSelectTool} selected={selectedTool === item.key} />
          case 'notice':
            return (
              <pre key={item.key} className="notice">
                {item.text}
              </pre>
            )
          case 'question':
            return (
              <div key={item.key} className={`question${item.answered ? ' answered' : ''}`} role="note">
                <div className="question-label">Neuron asks</div>
                <pre>{item.text}</pre>
              </div>
            )
          case 'error':
            return (
              <div key={item.key} className="error-line" role="alert">
                {item.text}
              </div>
            )
        }
      })}
      {busy && <div className="working" aria-label="working"><span /><span /><span /></div>}
      <div ref={end} />
    </div>
  )
}
