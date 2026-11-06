import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react'

import { quietReadKind, quietReadSummary, type ToolState, type TranscriptItem } from '@neuron/client'

import { DiffView } from './DiffView'
import { LiveOutput } from './LiveOutput'
import { CopyButton, Markdown } from './Markdown'
import { ProblemCard, type ProblemActions } from './ProblemCard'
import { ReportPanel } from './ReportPanel'

type ToolItem = Extract<TranscriptItem, { kind: 'tool' }>
type QuestionItem = Extract<TranscriptItem, { kind: 'question' }>

const STATE_LABEL: Record<ToolState, string> = {
  writing: 'writing',
  running: 'running',
  awaiting_permission: 'waiting for you',
  done: 'done',
  error: 'failed',
  denied: 'denied',
  cut_off: 'cut off',
}

const isShell = (item: ToolItem) => item.name === 'bash' || item.name === 'PowerShell'

/** "7s", "1m 05s", "1h 02m". */
export function formatDuration(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000))
  if (seconds < 60) return `${seconds}s`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes}m ${String(seconds % 60).padStart(2, '0')}s`
  return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, '0')}m`
}

/** The time a call has taken. Its timer is local to the card and runs only while the call does. */
function Duration({
  startedAt,
  endedAt,
  running,
}: {
  startedAt: number | null | undefined
  endedAt: number | null | undefined
  running: boolean
}) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!running) return
    setNow(Date.now())
    const timer = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(timer)
  }, [running])
  if (startedAt === null || startedAt === undefined) return null
  if (running) {
    return (
      <span className="tool-duration" aria-label="running for">
        {formatDuration(now - startedAt)}
      </span>
    )
  }
  if (endedAt === null || endedAt === undefined) return null
  return <span className="tool-duration">{formatDuration(endedAt - startedAt)}</span>
}

/** The model is still writing the call's input. */
function Writing({ item }: { item: ToolItem }) {
  const { progress, view } = item
  const target = progress?.target || view.target
  const last = (progress?.preview ?? '').replace(/\n+$/, '').split('\n').slice(-4)
  return (
    <div className="tool-writing">
      <div className="writing-head">
        <span className="writing-busy" aria-hidden>
          <span />
          <span />
          <span />
        </span>
        <span className="writing-label">{target ? `Writing ${target}…` : 'Writing…'}</span>
        {progress && progress.lines > 0 && <span className="muted small">{progress.lines} lines</span>}
      </div>
      {progress?.preview && <pre className="writing-preview">{last.join('\n')}</pre>}
    </div>
  )
}

/** Memoized on item identity: the reducer replaces only the item that changed. */
export const ToolCard = memo(function ToolCard({
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
  const liveShell = isShell(item) && item.state === 'running'
  const [toggled, setToggled] = useState<boolean | null>(null)
  // A running shell command shows its output however the cards are folded; the user's own choice wins.
  const open = toggled ?? (startsOpen || liveShell)
  const state = subagent?.running ? 'sub-agent working' : STATE_LABEL[item.state]
  return (
    <div className={`tool-card tool-${view.kind} state-${item.state}${selected ? ' selected' : ''}`} data-testid="tool-card">
      <button className="tool-head" onClick={() => setToggled(!open)} aria-expanded={open}>
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
        {isShell(item) && <Duration startedAt={item.startedAt} endedAt={item.endedAt} running={liveShell} />}
        {isShell(item) && view.exitCode !== undefined && (
          <span className={`tool-exit${view.exitCode === 0 ? '' : ' exit-fail'}`}>exit {view.exitCode}</span>
        )}
        <span className="tool-state">{state}</span>
      </button>
      {item.state === 'writing' && <Writing item={item} />}
      {open && item.state !== 'writing' && (
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
          {liveShell && item.live && <LiveOutput live={item.live} />}
          {!subagent && !liveShell && view.output && view.kind !== 'write' && <pre className="tool-output">{view.output}</pre>}
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
})

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

type Entry = { kind: 'item'; item: TranscriptItem } | { kind: 'quiet'; key: string; items: ToolItem[] }

/** A finished read-only call, not worth a card of its own. Failed, running, writing and waiting calls never qualify. */
const isQuiet = (item: TranscriptItem): item is ToolItem =>
  item.kind === 'tool' && item.state === 'done' && quietReadKind(item.name) !== null

/** The items, with runs of two or more finished read-only calls folded into one entry. */
function groupQuiet(items: TranscriptItem[]): Entry[] {
  const out: Entry[] = []
  let run: ToolItem[] = []
  const flush = () => {
    if (run.length >= 2) out.push({ kind: 'quiet', key: `quiet-${run[0].key}`, items: run })
    else for (const item of run) out.push({ kind: 'item', item })
    run = []
  }
  for (const item of items) {
    if (isQuiet(item)) run.push(item)
    else {
      flush()
      out.push({ kind: 'item', item })
    }
  }
  flush()
  return out
}

const sameItems = (a: ToolItem[], b: ToolItem[]) => a.length === b.length && a.every((item, index) => item === b[index])

/** Several finished reads and searches as one line; it expands to the cards. */
const QuietGroup = memo(
  function QuietGroup({
    items,
    onSelect,
    selectedTool,
  }: {
    items: ToolItem[]
    onSelect?: (item: ToolItem) => void
    selectedTool?: string | null
  }) {
    const [open, setOpen] = useState(false)
    return (
      <div className="quiet-group">
        <button className="quiet-head" aria-expanded={open} onClick={() => setOpen(!open)}>
          <span aria-hidden>{open ? '▾' : '▸'}</span> {quietReadSummary(items.map((item) => item.name))}
        </button>
        {open &&
          items.map((item) => (
            <ToolCard key={item.key} item={item} folded={false} onSelect={onSelect} selected={selectedTool === item.key} />
          ))}
      </div>
    )
  },
  (a, b) => sameItems(a.items, b.items) && a.onSelect === b.onSelect && a.selectedTool === b.selectedTool,
)

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
  // A stable callback, so the memoized cards do not re-render with every render of the parent.
  const latestSelect = useRef(onSelectTool)
  latestSelect.current = onSelectTool
  const onSelect = useCallback((item: ToolItem) => latestSelect.current?.(item), [])
  const select = onSelectTool ? onSelect : undefined
  const entries = useMemo(() => groupQuiet(items), [items])
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
      {entries.map((entry) => {
        if (entry.kind === 'quiet') {
          return <QuietGroup key={entry.key} items={entry.items} onSelect={select} selectedTool={selectedTool} />
        }
        const item = entry.item
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
            return <ToolCard key={item.key} item={item} folded={folded} onSelect={select} selected={selectedTool === item.key} />
          case 'notice':
            return item.noticeKind === 'refusal' ? (
              <div key={item.key} className="notice refusal" role="note">
                <span className="refusal-label">Declined</span>
                <span className="refusal-text">{item.text}</span>
              </div>
            ) : (
              <pre key={item.key} className="notice">
                {item.text}
              </pre>
            )
          case 'report':
            return <ReportPanel key={item.key} item={item} onLink={onLink} />
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
          default: {
            // A new item kind must be rendered above: this stops compiling until it is.
            const unhandled: never = item
            return unhandled
          }
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
