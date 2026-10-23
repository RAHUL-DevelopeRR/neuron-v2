import { useEffect, useRef, useState } from 'react'

import type { ToolState, TranscriptItem } from '@neuron/client'

import { DiffView } from './DiffView'
import { CopyButton, Markdown } from './Markdown'
import { ProblemCard, type ProblemActions } from './ProblemCard'

type ToolItem = Extract<TranscriptItem, { kind: 'tool' }>
type QuestionItem = Extract<TranscriptItem, { kind: 'question' }>

const STATE_LABEL: Record<ToolState, string> = {
  running: 'running',
  awaiting_permission: 'waiting for you',
  done: 'done',
  error: 'failed',
  denied: 'denied',
  cut_off: 'cut off',
}

export function ToolCard({
  item,
  folded,
  onSelect,
  selected,
}: {
  item: ToolItem
  folded: boolean
  onSelect?: (item: ToolItem) => void
  selected?: boolean
}) {
  const { view, subagent } = item
  const startsOpen = !folded || view.kind === 'edit' || view.kind === 'write' || item.state === 'error'
  const [open, setOpen] = useState(startsOpen)
  const state = subagent?.running ? 'sub-agent working' : STATE_LABEL[item.state]
  return (
    <div className={`tool-card tool-${view.kind} state-${item.state}${selected ? ' selected' : ''}`} data-testid="tool-card">
      <button className="tool-head" onClick={() => setOpen(!open)} aria-expanded={open}>
        <span className="tool-dot" aria-hidden />
        <span className="tool-verb">{subagent ? `Sub-agent${subagent.type ? ` (${subagent.type})` : ''}` : view.verb}</span>
        <span className="tool-target" title={view.target}>
          {subagent?.description ?? view.target}
        </span>
        {view.diff && view.diff.lines.length > 0 && (
          <span className="tool-diffstat">
            <span className="diff-added">+{view.diff.added}</span> <span className="diff-removed">−{view.diff.removed}</span>
          </span>
        )}
        <span className="tool-state">{state}</span>
      </button>
      {open && (
        <div className="tool-body">
          {subagent && !subagent.running && (
            <div className="subagent">
              <div className="muted small">Sub-agent {subagent.status ?? 'finished'}</div>
              {subagent.result && <Markdown text={subagent.result} />}
              {subagent.note && <div className="muted small">{subagent.note}</div>}
            </div>
          )}
          {!subagent && view.kind === 'terminal' && <pre className="tool-command">$ {view.target}</pre>}
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
          {!subagent && view.output && view.kind !== 'write' && <pre className="tool-output">{view.output}</pre>}
          {view.output && view.kind === 'write' && <div className="muted">{view.output}</div>}
          {onSelect && (view.diff || view.output) && (
            <button className="link-button" onClick={() => onSelect(item)}>
              Open in the side panel
            </button>
          )}
        </div>
      )}
    </div>
  )
}

function QuestionCard({ item, onAnswer }: { item: QuestionItem; onAnswer?: (id: number, text: string) => void }) {
  return (
    <div className={`question${item.answered ? ' answered' : ''}`} role="note" aria-label="Neuron asks">
      <div className="question-label">{item.answered ? 'Neuron asked' : 'Neuron asks'}</div>
      <div className="question-text">{item.text}</div>
      {!item.answered && item.options && item.options.length > 0 && item.id !== undefined && (
        <div className="question-options">
          {item.options.map((option, index) => (
            <button key={option} onClick={() => onAnswer?.(item.id!, option)}>
              <kbd>{index + 1}</kbd> {option}
            </button>
          ))}
        </div>
      )}
      {!item.answered && <div className="muted small">Pick an answer or type your own below.</div>}
    </div>
  )
}

export function Transcript({
  items,
  busy,
  folded,
  showReasoning,
  onLink,
  onAnswer,
  onSelectTool,
  selectedTool,
  empty,
  problemActions,
}: {
  items: TranscriptItem[]
  busy: boolean
  folded: boolean
  showReasoning: boolean
  onLink?: (url: string) => void
  onAnswer?: (id: number, text: string) => void
  onSelectTool?: (item: ToolItem) => void
  selectedTool?: string | null
  empty?: React.ReactNode
  /** What a gateway failure's card offers; without them it is a plain error line. */
  problemActions?: ProblemActions
}) {
  const end = useRef<HTMLDivElement>(null)
  const [pinned, setPinned] = useState(true)
  useEffect(() => {
    if (pinned) end.current?.scrollIntoView?.({ block: 'end' })
  }, [items, pinned])
  return (
    <div
      className="transcript"
      onScroll={(event) => {
        const el = event.currentTarget
        setPinned(el.scrollHeight - el.scrollTop - el.clientHeight < 40)
      }}
      role="log"
      aria-live="polite"
    >
      {items.length === 0 && empty}
      {items.map((item) => {
        switch (item.kind) {
          case 'user':
            return (
              <div key={item.key} className="message user" data-testid="user-message">
                <div className="message-text">{item.text}</div>
              </div>
            )
          case 'assistant':
            return (
              <div key={item.key} className={`message assistant${item.streaming ? ' streaming' : ''}`} data-testid="assistant-message">
                <Markdown text={item.text} onLink={onLink} />
                {!item.streaming && <CopyButton text={item.text} label="Copy reply" />}
              </div>
            )
          case 'reasoning':
            if (!showReasoning) return null
            return (
              <details key={item.key} className="reasoning">
                <summary>{item.redacted ? 'Reasoning (hidden by the provider)' : item.streaming ? 'Thinking…' : 'Reasoning'}</summary>
                {item.text && <pre>{item.text}</pre>}
              </details>
            )
          case 'tool':
            return <ToolCard key={item.key} item={item} folded={folded} onSelect={onSelectTool} selected={selectedTool === item.key} />
          case 'notice':
            return (
              <pre key={item.key} className="notice">
                {item.text}
              </pre>
            )
          case 'question':
            return <QuestionCard key={item.key} item={item} onAnswer={onAnswer} />
          case 'error':
            return item.problem && problemActions ? (
              <ProblemCard key={item.key} problem={item.problem} actions={problemActions} />
            ) : (
              <div key={item.key} className="error-line" role="alert">
                {item.text}
              </div>
            )
        }
      })}
      {busy && (
        <div className="working" aria-label="working">
          <span />
          <span />
          <span />
        </div>
      )}
      <div ref={end} />
    </div>
  )
}
