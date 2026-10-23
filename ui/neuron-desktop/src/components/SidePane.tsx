import { lazy, Suspense, useEffect, useState } from 'react'

import { additionDiff, parseUnifiedDiff, type Diff, type Extensions, type TodoItem, type TranscriptItem } from '@neuron/client'

import type { FileContent, GitStatusView, Platform } from '../platform'
import { DiffView } from './DiffView'
import { ExtensionsPanel } from './ExtensionsPanel'
import { FileTree } from './FileTree'

// xterm only loads when the terminal first opens.
const TerminalPane = lazy(() => import('./TerminalPane').then((module) => ({ default: module.TerminalPane })))

type ToolItem = Extract<TranscriptItem, { kind: 'tool' }>
export type PaneTab = 'changes' | 'files' | 'tasks' | 'extensions' | 'terminal'

const STATUS_LABEL: Record<string, string> = { M: 'modified', A: 'added', D: 'deleted', R: 'renamed', C: 'copied', U: 'conflict', '?': 'new' }

function changeKind(index: string, worktree: string): string {
  const code = worktree !== ' ' ? worktree : index
  return STATUS_LABEL[code] ?? code
}

/** The working tree's changes, as git sees them, with each file's diff. */
function ChangesPanel({ platform, refreshKey, tool }: { platform: Platform; refreshKey: number; tool: ToolItem | null }) {
  const [status, setStatus] = useState<GitStatusView | null | undefined>(undefined)
  const [error, setError] = useState<string | null>(null)
  const [selected, setSelected] = useState<string | null>(null)
  const [diff, setDiff] = useState<Diff | null>(null)

  useEffect(() => {
    let cancelled = false
    platform.gitStatus().then(
      (result) => !cancelled && (setStatus(result), setError(null)),
      (failure: Error) => !cancelled && (setStatus(null), setError(failure.message)),
    )
    return () => {
      cancelled = true
    }
  }, [platform, refreshKey])

  useEffect(() => {
    if (!selected || !status) return
    let cancelled = false
    const change = status.changes.find((entry) => entry.path === selected)
    const load: Promise<Diff> =
      change?.index === '?'
        ? platform.read(selected).then((file: FileContent) => additionDiff(file.binary ? '(binary file)' : file.content))
        : platform.gitDiff(selected).then((text) => parseUnifiedDiff(text))
    load.then(
      (result) => !cancelled && setDiff(result),
      (failure: Error) => !cancelled && setError(failure.message),
    )
    return () => {
      cancelled = true
    }
  }, [platform, selected, status])

  if (tool) {
    return (
      <div className="pane-body">
        <div className="pane-subtitle">
          {tool.view.verb} <span className="muted">{tool.view.target}</span>
        </div>
        {tool.view.diff && <DiffView diff={tool.view.diff} />}
        {tool.view.output && <pre className="tool-output">{tool.view.output}</pre>}
      </div>
    )
  }
  return (
    <div className="pane-body">
      {error && <div className="error-line small">{error}</div>}
      {status === undefined && <div className="muted pad">Reading git…</div>}
      {status === null && !error && <div className="muted pad">This project is not a git repository.</div>}
      {status && (
        <>
          <div className="pane-subtitle">
            {status.branch ? `⎇ ${status.branch}` : 'git'} · {status.changes.length} changed
          </div>
          {status.changes.length === 0 && <div className="muted pad">No changes. The working tree is clean.</div>}
          <ul className="changes">
            {status.changes.map((change) => (
              <li key={change.path}>
                <button
                  className={`tree-row change-row${selected === change.path ? ' selected' : ''}`}
                  onClick={() => setSelected(change.path === selected ? null : change.path)}
                  title={change.path}
                >
                  <span className={`change-kind kind-${changeKind(change.index, change.worktree)}`}>{changeKind(change.index, change.worktree)}</span>
                  {change.path}
                </button>
                {selected === change.path && diff && <DiffView diff={diff} label={`diff of ${change.path}`} />}
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  )
}

function FilesPanel({ platform, refreshKey, changed }: { platform: Platform; refreshKey: number; changed: Set<string> }) {
  const [file, setFile] = useState<string | null>(null)
  const [content, setContent] = useState<FileContent | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    if (!file) return
    let cancelled = false
    platform.read(file).then(
      (result) => !cancelled && (setContent(result), setError(null)),
      (failure: Error) => !cancelled && (setContent(null), setError(failure.message)),
    )
    return () => {
      cancelled = true
    }
  }, [platform, file, refreshKey])
  return (
    <div className="pane-body files-pane">
      <FileTree platform={platform} refreshKey={refreshKey} changed={changed} selected={file} onOpen={setFile} />
      {file && (
        <section className="file-preview">
          <div className="pane-subtitle">
            {file}
            <button className="icon-button" aria-label="Close file" onClick={() => setFile(null)}>
              ✕
            </button>
          </div>
          {error && <div className="error-line small">{error}</div>}
          {content?.binary && <div className="muted pad">Binary file ({content.size} bytes)</div>}
          {content && !content.binary && (
            <pre className="file-view" data-testid="file-view">
              {content.content}
              {content.truncated && '\n… (cut)'}
            </pre>
          )}
        </section>
      )}
    </div>
  )
}

function TasksPanel({ todos, subagents }: { todos: TodoItem[]; subagents: ToolItem[] }) {
  const done = todos.filter((todo) => todo.status === 'completed').length
  return (
    <div className="pane-body">
      <div className="pane-subtitle">
        Tasks {todos.length > 0 && <span className="muted">{done}/{todos.length} done</span>}
      </div>
      {todos.length === 0 ? (
        <div className="muted pad">Neuron lists its steps here when it plans a larger job.</div>
      ) : (
        <ul className="todos big" aria-label="Tasks">
          {todos.map((todo, index) => (
            <li key={index} className={`todo-${todo.status}`}>
              <span className="todo-mark">{todo.status === 'completed' ? '✓' : todo.status === 'in_progress' ? '▸' : '○'}</span>
              {todo.status === 'in_progress' && todo.active_form ? todo.active_form : todo.content}
            </li>
          ))}
        </ul>
      )}
      <div className="pane-subtitle">Sub-agents</div>
      {subagents.length === 0 ? (
        <div className="muted pad">None running.</div>
      ) : (
        <ul className="subagents">
          {subagents.map((item) => (
            <li key={item.key}>
              <span className="tool-dot running-dot" aria-hidden /> {item.subagent?.description}
              {item.subagent?.type && <span className="muted"> · {item.subagent.type}</span>}
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

export function SidePane({
  platform,
  tab,
  onTab,
  refreshKey,
  changed,
  tool,
  todos,
  subagents,
  extensions,
  onRefreshExtensions,
  terminalOpened,
}: {
  platform: Platform
  tab: PaneTab
  onTab: (tab: PaneTab) => void
  refreshKey: number
  changed: Set<string>
  tool: ToolItem | null
  todos: TodoItem[]
  subagents: ToolItem[]
  /** `undefined` when the engine has no extensions inventory (feature `extensions`). */
  extensions: Extensions | null | undefined
  onRefreshExtensions: () => void
  terminalOpened: boolean
}) {
  const tabs: Array<[PaneTab, string]> = [
    ['changes', 'Changes'],
    ['files', 'Files'],
    ['tasks', todos.length ? `Tasks ${todos.filter((todo) => todo.status === 'completed').length}/${todos.length}` : 'Tasks'],
    ...(extensions !== undefined ? ([['extensions', 'Extensions']] as Array<[PaneTab, string]>) : []),
    ...(platform.can.terminal ? ([['terminal', 'Terminal']] as Array<[PaneTab, string]>) : []),
  ]
  return (
    <aside className="side-pane" aria-label="Details">
      <div className="tabs" role="tablist">
        {tabs.map(([id, label]) => (
          <button key={id} role="tab" aria-selected={tab === id} className={tab === id ? 'active' : ''} onClick={() => onTab(id)}>
            {label}
          </button>
        ))}
      </div>
      {tab === 'changes' && <ChangesPanel platform={platform} refreshKey={refreshKey} tool={tool} />}
      {tab === 'files' && <FilesPanel platform={platform} refreshKey={refreshKey} changed={changed} />}
      {tab === 'tasks' && <TasksPanel todos={todos} subagents={subagents} />}
      {tab === 'extensions' && extensions !== undefined && <ExtensionsPanel extensions={extensions} onRefresh={onRefreshExtensions} />}
      {/* The terminal stays mounted once opened, so its shell lives on across tabs. */}
      {terminalOpened && (
        <div className="terminal-tab" hidden={tab !== 'terminal'}>
          <Suspense fallback={<div className="muted pad">Loading the terminal…</div>}>
            <TerminalPane platform={platform} />
          </Suspense>
        </div>
      )}
    </aside>
  )
}
