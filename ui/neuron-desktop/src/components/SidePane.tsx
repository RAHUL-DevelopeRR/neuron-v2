import { lazy, Suspense, useEffect, useId, useMemo, useRef, useState, type ReactNode } from 'react'

import {
  FEATURE_CHECKPOINTS,
  FEATURE_GIT_VIEWS,
  additionDiff,
  hasFeature,
  parseUnifiedDiff,
  type CommitProposalEvent,
  type Diff,
  type SessionController,
  type SessionState,
  type TodoItem,
  type TranscriptItem,
} from '@neuron/client'

import { useFocusTrap } from '../a11y'
import { previewOf } from '../mediaFiles'
import type { BranchChanges, FileContent, GitChange, GitStatusView, Platform, WorktreeInfo } from '../platform'
import { CommitCard } from './CommitCard'
import { DiffView } from './DiffView'
import { FileTree } from './FileTree'
import { RewindPicker } from './RewindPicker'

// xterm only loads when the terminal first opens.
const TerminalPane = lazy(() => import('./TerminalPane').then((module) => ({ default: module.TerminalPane })))

type ToolItem = Extract<TranscriptItem, { kind: 'tool' }>
export type PaneTab = 'changes' | 'files' | 'tasks' | 'terminal'

const STATUS_LABEL: Record<string, string> = { M: 'modified', A: 'added', D: 'deleted', R: 'renamed', C: 'copied', U: 'conflict', '?': 'new' }

/** The engine's words for a change (`created`, ...), as the list shows them. */
const STATUS_NAMES: Record<string, string> = { created: 'new', modified: 'modified', deleted: 'deleted', renamed: 'renamed' }

function changeKind(index: string, worktree: string): string {
  const code = worktree !== ' ' ? worktree : index
  return STATUS_LABEL[code] ?? code
}

/** The controller calls the Changes pane makes (feature `git_views` and `checkpoints`). */
export type ReviewController = Pick<
  SessionController,
  'requestDiff' | 'proposeCommit' | 'commit' | 'dismissCommit' | 'requestCheckpoints' | 'rewind'
>

type Scope = 'chat' | 'folder' | 'branch'

const SCOPE_NAMES: Record<Scope, string> = { chat: 'This chat', folder: 'This folder', branch: 'Branch changes' }

/** v1.1: where a chat in its own copy reads what its branch changed. */
interface BranchSource {
  base: string
  load(): Promise<BranchChanges>
  diff(path: string): Promise<string>
}

const plural = (count: number, one: string, many = `${one}s`) => `${count} ${count === 1 ? one : many}`

/**
 * v1.1: everything the chat's branch changed since it left its base (its merge
 * base), committed or not, with the lines added and removed. Read only: a file
 * shows its diff; reverting stays in "This folder".
 */
function BranchChangesView({ source, refreshKey }: { source: BranchSource; refreshKey: number }) {
  const [changes, setChanges] = useState<BranchChanges | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [selected, setSelected] = useState<string | null>(null)
  const [diff, setDiff] = useState<Diff | null>(null)
  useEffect(() => {
    let cancelled = false
    source.load().then(
      (loaded) => !cancelled && (setChanges(loaded), setError(null)),
      (failure: Error) => !cancelled && (setChanges(null), setError(failure.message)),
    )
    return () => {
      cancelled = true
    }
  }, [source, refreshKey])
  useEffect(() => {
    setDiff(null)
    if (!selected) return
    let cancelled = false
    source.diff(selected).then(
      (text) => !cancelled && setDiff(parseUnifiedDiff(text)),
      (failure: Error) => !cancelled && setError(failure.message),
    )
    return () => {
      cancelled = true
    }
  }, [source, selected, refreshKey])

  if (error) return <div className="error-line small">{error}</div>
  if (!changes) return <div className="muted pad">Reading the branch…</div>
  return (
    <>
      <div className="pane-subtitle">
        ⎇ {changes.branch ?? 'this branch'} since {changes.base} · {plural(changes.commits, 'commit')} · {plural(changes.totals.files, 'file')}{' '}
        <span className="change-counts">
          <span className="diff-added">+{changes.totals.adds}</span> <span className="diff-removed">−{changes.totals.dels}</span>
        </span>
      </div>
      {changes.files.length === 0 && <div className="muted pad">Nothing has changed on this branch since it left {changes.base}.</div>}
      <ul className="changes">
        {changes.files.map((file) => (
          <ChangeRow
            key={file.path}
            path={file.path}
            kind={file.untracked ? 'new' : file.status}
            counts={file.adds !== null && file.dels !== null ? { added: file.adds, removed: file.dels } : undefined}
            selected={selected === file.path}
            onSelect={() => setSelected(selected === file.path ? null : file.path)}
            onOpenEditor={null}
            onRevert={null}
          >
            {selected === file.path &&
              (file.binary ? <div className="muted pad">Binary file</div> : diff && <DiffView diff={diff} label={`diff of ${file.path}`} />)}
          </ChangeRow>
        ))}
      </ul>
      {changes.truncated && <div className="muted pad small">More files changed than are listed here.</div>}
    </>
  )
}

/** What reverting a file does, in words, for the confirmation. */
function revertWords(path: string, change: GitChange | undefined) {
  if (change?.index === '?') {
    return {
      title: `Delete ${path}?`,
      body: `${path} is a new file that git does not track yet. It will be deleted from this folder, and Neuron can't bring it back.`,
      confirm: 'Delete file',
    }
  }
  if (change?.index === 'A') {
    return {
      title: `Delete ${path}?`,
      body: `${path} was added to git but never committed. It will be taken out of git and deleted from this folder, and Neuron can't bring it back.`,
      confirm: 'Delete file',
    }
  }
  return {
    title: `Revert ${path}?`,
    body: `This puts ${path} back exactly as it was at the last commit. Everything changed in this file since then is lost, and Neuron can't bring it back.`,
    confirm: 'Revert file',
  }
}

function ConfirmDialog({
  title,
  body,
  confirm,
  onConfirm,
  onCancel,
}: {
  title: string
  body: string
  confirm: string
  onConfirm: () => void
  onCancel: () => void
}) {
  const ref = useRef<HTMLDivElement>(null)
  useFocusTrap(ref)
  return (
    <div className="dialog-backdrop">
      <div
        ref={ref}
        className="dialog small-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="confirm-title"
        aria-describedby="confirm-body"
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.stopPropagation()
            onCancel()
          }
        }}
      >
        <h2 id="confirm-title">{title}</h2>
        <p id="confirm-body">{body}</p>
        <div className="dialog-buttons">
          <button data-autofocus onClick={onCancel}>
            Cancel
          </button>
          <button className="danger" onClick={onConfirm}>
            {confirm}
          </button>
        </div>
      </div>
    </div>
  )
}

/** One changed file: its row, "open in editor", and (for files git can see) "revert". */
function ChangeRow({
  path,
  kind,
  counts,
  selected,
  onSelect,
  onOpenEditor,
  onRevert,
  children,
}: {
  path: string
  kind: string
  counts?: { added: number; removed: number }
  selected: boolean
  onSelect: () => void
  onOpenEditor: (() => void) | null
  onRevert: (() => void) | null
  children?: ReactNode
}) {
  const id = useId()
  return (
    <li>
      <div className="change-line">
        <button id={id} className={`tree-row change-row${selected ? ' selected' : ''}`} onClick={onSelect} title={path}>
          <span className={`change-kind kind-${kind}`}>{kind}</span>
          <span className="change-path">{path}</span>
          {counts && (
            <span className="change-counts">
              <span className="diff-added">+{counts.added}</span> <span className="diff-removed">−{counts.removed}</span>
            </span>
          )}
        </button>
        {onOpenEditor && (
          <button className="row-action" aria-describedby={id} onClick={onOpenEditor}>
            Open in editor
          </button>
        )}
        {onRevert && (
          <button className="row-action" aria-describedby={id} onClick={onRevert}>
            Revert
          </button>
        )}
      </div>
      {children}
    </li>
  )
}

/**
 * Changes, in two scopes: what the agent changed in this chat (the engine's
 * list, feature `git_views`) and what git sees in the folder. Also the commit
 * card, the restore-point picker, reverting a file and "open in editor".
 */
function ChangesPanel({
  platform,
  refreshKey,
  tool,
  state,
  controller,
  branch = null,
}: {
  platform: Platform
  refreshKey: number
  tool: ToolItem | null
  state: SessionState | null
  controller: ReviewController | null
  /** v1.1: a chat in its own copy also shows what its branch changed. */
  branch?: BranchSource | null
}) {
  const [status, setStatus] = useState<GitStatusView | null | undefined>(undefined)
  const [error, setError] = useState<string | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)
  const [selectedFolder, setSelectedFolder] = useState<string | null>(null)
  const [selectedChat, setSelectedChat] = useState<string | null>(null)
  const [diff, setDiff] = useState<Diff | null>(null)
  const [choice, setChoice] = useState<Scope | null>(null)
  const [bump, setBump] = useState(0)
  const [reverting, setReverting] = useState<string | null>(null)
  const [rewinding, setRewinding] = useState(false)
  /**
   * The commit in progress, kept here: the session forgets the proposal once the commit is sent, and a failed
   * commit must not lose the message the user wrote. `mark` is how many transcript items there were when it
   * went out; what the engine says after that is its outcome.
   */
  const [commit, setCommit] = useState<{
    proposal: CommitProposalEvent
    message: string
    phase: 'editing' | 'sending' | 'failed'
    error: string | null
    mark: number
  } | null>(null)
  /** What the engine said about the last commit that went through; it belongs to that commit only. */
  const [commitOutcome, setCommitOutcome] = useState<string | null>(null)
  /** The `diff` answer there was when the open chat file's diff was last asked for. */
  const [diffBefore, setDiffBefore] = useState<SessionState['diff']>(null)

  const gitViews = Boolean(state && controller && hasFeature(state, FEATURE_GIT_VIEWS))
  const checkpoints = Boolean(state && controller && hasFeature(state, FEATURE_CHECKPOINTS))
  const chatFiles = state?.changes ?? []
  const scope: Scope = choice ?? (gitViews && chatFiles.length > 0 ? 'chat' : 'folder')
  const scopes: Scope[] = [...(gitViews ? (['chat'] as const) : []), 'folder', ...(branch ? (['branch'] as const) : [])]
  const busy = state?.busy ?? false

  useEffect(() => {
    let cancelled = false
    platform.gitStatus().then(
      (result) => !cancelled && (setStatus(result), setError(null)),
      (failure: Error) => !cancelled && (setStatus(null), setError(failure.message)),
    )
    return () => {
      cancelled = true
    }
  }, [platform, refreshKey, bump])

  // A new draft replaces whatever the last commit left, its outcome included.
  const proposal = state?.commitProposal ?? null
  useEffect(() => {
    if (!proposal) return
    setCommit({ proposal, message: proposal.message, phase: 'editing', error: null, mark: 0 })
    setCommitOutcome(null)
  }, [proposal])

  // After the commit went out, the engine's next error or notice is its answer.
  const items = state?.items
  useEffect(() => {
    if (!commit || commit.phase !== 'sending' || !items) return
    const answer = items.slice(commit.mark).reverse().find((item) => item.kind === 'error' || item.kind === 'notice')
    if (!answer) return
    if (answer.kind === 'error') setCommit({ ...commit, phase: 'failed', error: answer.text })
    else if (answer.kind === 'notice') {
      setCommit(null)
      setCommitOutcome(answer.text)
    }
  }, [commit, items])

  // This folder: the file's diff, read from git here.
  useEffect(() => {
    if (scope !== 'folder' || !selectedFolder || !status) return
    let cancelled = false
    const change = status.changes.find((entry) => entry.path === selectedFolder)
    const load: Promise<Diff> =
      change?.index === '?'
        ? platform.read(selectedFolder).then((file: FileContent) => additionDiff(file.binary ? '(binary file)' : file.content))
        : platform.gitDiff(selectedFolder).then((text) => parseUnifiedDiff(text))
    load.then(
      (result) => !cancelled && setDiff(result),
      (failure: Error) => !cancelled && setError(failure.message),
    )
    return () => {
      cancelled = true
    }
  }, [platform, scope, selectedFolder, status])

  // This chat: the engine's diff. `/diff` is a slash command, so it waits for an idle agent; a new
  // `changes` event (a new list) asks again.
  const wantDiff = scope === 'chat' && gitViews && selectedChat !== null && !busy
  const askedFor = state?.changes
  useEffect(() => {
    if (!wantDiff || !controller || !selectedChat) return
    setDiffBefore(state?.diff ?? null)
    controller.requestDiff(selectedChat)
    // `state.diff` is read for the baseline only: a new answer must not ask again.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wantDiff, controller, selectedChat, askedFor, bump])

  const refreshBoth = () => setBump((value) => value + 1)
  const guard = (action: Promise<void>) => action.then(() => setActionError(null), (failure: Error) => setActionError(failure.message))
  const openEditor = (path: string) => void guard(platform.openInEditor(path))
  const absolute = (path: string) => path.startsWith('/') || /^[A-Za-z]:/.test(path)

  const confirmRevert = () => {
    const path = reverting
    setReverting(null)
    if (!path) return
    platform.gitRevert(path).then(
      () => {
        setActionError(null)
        setSelectedFolder((current) => (current === path ? null : current))
        refreshBoth()
      },
      (failure: Error) => {
        setActionError(failure.message)
        refreshBoth()
      },
    )
  }

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

  const folderChange = (path: string) => status?.changes.find((entry) => entry.path === path)

  const chatDiff = (path: string) => {
    if (busy) return <div className="muted pad">The diff loads when the agent is idle</div>
    const file = state?.diff?.files.find((entry) => entry.path === path)
    if (file) {
      if (file.binary) return <div className="muted pad">Binary file</div>
      if (!file.patch) return <div className="muted pad">No diff to show for this file.</div>
      return <DiffView diff={parseUnifiedDiff(file.patch)} label={`diff of ${path}`} />
    }
    return <div className="muted pad">{state?.diff && state.diff !== diffBefore ? 'No diff to show for this file.' : 'Loading the diff…'}</div>
  }

  return (
    <div className="pane-body">
      <div className="changes-head">
        {scopes.length > 1 && (
          <div className="scope-toggle" role="group" aria-label="Show changes from">
            {scopes.map((id) => (
              <button key={id} aria-pressed={scope === id} className={scope === id ? 'active' : ''} onClick={() => setChoice(id)}>
                {SCOPE_NAMES[id]}
              </button>
            ))}
          </div>
        )}
        <div className="changes-actions">
          {gitViews && (
            <button
              disabled={busy}
              title={busy ? 'Wait until the agent is idle' : 'Draft a commit message for these changes'}
              onClick={() => {
                setCommitOutcome(null)
                controller?.proposeCommit()
              }}
            >
              Commit
            </button>
          )}
          {checkpoints && <button onClick={() => setRewinding(true)}>Restore point…</button>}
        </div>
      </div>
      {error && <div className="error-line small">{error}</div>}
      {actionError && (
        <div className="error-line small" role="alert">
          {actionError}
        </div>
      )}
      {commitOutcome && (
        <div className="commit-outcome" role="status">
          {commitOutcome}
        </div>
      )}
      {commit && (
        <CommitCard
          proposal={commit.proposal}
          message={commit.message}
          sending={commit.phase === 'sending'}
          error={commit.phase === 'failed' ? commit.error : null}
          busy={busy}
          onMessage={(message) => setCommit({ ...commit, message })}
          onCommit={(message) => {
            setCommit({ ...commit, message, phase: 'sending', error: null, mark: state?.items.length ?? 0 })
            controller?.commit(message)
          }}
          onCancel={() => {
            if (state?.commitProposal) controller?.dismissCommit()
            setCommit(null)
          }}
        />
      )}

      {scope === 'chat' && (
        <>
          <div className="pane-subtitle">{chatFiles.length} changed in this chat</div>
          {chatFiles.length === 0 && <div className="muted pad">Nothing has changed in this chat yet.</div>}
          <ul className="changes">
            {chatFiles.map((file) => (
              <ChangeRow
                key={file.path}
                path={file.path}
                kind={STATUS_NAMES[file.status] ?? file.status}
                counts={{ added: file.added, removed: file.removed }}
                selected={selectedChat === file.path}
                onSelect={() => setSelectedChat(selectedChat === file.path ? null : file.path)}
                onOpenEditor={absolute(file.path) ? null : () => openEditor(file.path)}
                onRevert={folderChange(file.path) ? () => setReverting(file.path) : null}
              >
                {selectedChat === file.path && chatDiff(file.path)}
              </ChangeRow>
            ))}
          </ul>
        </>
      )}

      {scope === 'folder' && (
        <>
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
                  <ChangeRow
                    key={change.path}
                    path={change.path}
                    kind={changeKind(change.index, change.worktree)}
                    selected={selectedFolder === change.path}
                    onSelect={() => setSelectedFolder(change.path === selectedFolder ? null : change.path)}
                    onOpenEditor={() => openEditor(change.path)}
                    onRevert={() => setReverting(change.path)}
                  >
                    {selectedFolder === change.path && diff && <DiffView diff={diff} label={`diff of ${change.path}`} />}
                  </ChangeRow>
                ))}
              </ul>
            </>
          )}
        </>
      )}

      {scope === 'branch' && branch && <BranchChangesView source={branch} refreshKey={refreshKey + bump} />}

      {reverting && <ConfirmDialog {...revertWords(reverting, folderChange(reverting))} onConfirm={confirmRevert} onCancel={() => setReverting(null)} />}
      {rewinding && state && controller && <RewindPicker state={state} controller={controller} onClose={() => setRewinding(false)} />}
    </div>
  )
}

function FilesPanel({ platform, refreshKey, changed }: { platform: Platform; refreshKey: number; changed: Set<string> }) {
  const [file, setFile] = useState<string | null>(null)
  const [content, setContent] = useState<FileContent | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [openError, setOpenError] = useState<string | null>(null)
  // v1.1: images, videos and sounds are shown from their bytes (a blob: URL), not read as text.
  const preview = file && platform.readMedia ? previewOf(file) : null
  const [mediaUrl, setMediaUrl] = useState<string | null>(null)
  const [mediaProblem, setMediaProblem] = useState<string | null>(null)
  useEffect(() => {
    setMediaUrl(null)
    setMediaProblem(null)
    if (!file || !preview || !platform.readMedia) return
    let cancelled = false
    let url: string | null = null
    platform.readMedia(file).then(
      (bytes) => {
        if (cancelled) return
        url = URL.createObjectURL(new Blob([bytes as BlobPart], { type: preview.mimeType }))
        setMediaUrl(url)
        setError(null)
      },
      (failure: Error) => !cancelled && setMediaProblem(failure.message),
    )
    return () => {
      cancelled = true
      if (url) URL.revokeObjectURL(url)
    }
    // `preview` follows `file`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [platform, file, refreshKey])
  useEffect(() => {
    if (!file || preview) return
    let cancelled = false
    platform.read(file).then(
      (result) => !cancelled && (setContent(result), setError(null)),
      (failure: Error) => !cancelled && (setContent(null), setError(failure.message)),
    )
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [platform, file, refreshKey])
  return (
    <div className="pane-body files-pane">
      <FileTree platform={platform} refreshKey={refreshKey} changed={changed} selected={file} onOpen={setFile} />
      {file && (
        <section className="file-preview">
          <div className="pane-subtitle">
            {file}
            <button className="row-action" onClick={() => platform.openInEditor(file).then(() => setOpenError(null), (failure: Error) => setOpenError(failure.message))}>
              Open in editor
            </button>
            <button className="icon-button" aria-label="Close file" onClick={() => setFile(null)}>
              ✕
            </button>
          </div>
          {error && <div className="error-line small">{error}</div>}
          {openError && <div className="error-line small">{openError}</div>}
          {preview && mediaProblem && <div className="error-line small">{mediaProblem}</div>}
          {preview && !mediaUrl && !mediaProblem && <div className="muted pad">Loading…</div>}
          {/* An SVG is shown as an image only: its scripts and links never run in the page. */}
          {preview?.kind === 'image' && mediaUrl && <img className="file-image" src={mediaUrl} alt={file} />}
          {preview?.kind === 'video' && mediaUrl && (
            <video className="file-media" src={mediaUrl} controls aria-label={file} onError={() => setMediaProblem("This video can't play here.")} />
          )}
          {preview?.kind === 'audio' && mediaUrl && (
            <audio className="file-media" src={mediaUrl} controls aria-label={file} onError={() => setMediaProblem("This sound can't play here.")} />
          )}
          {!preview && content?.binary && <div className="muted pad">Binary file ({content.size} bytes)</div>}
          {!preview && content && !content.binary && (
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
  terminalOpened,
  state = null,
  controller = null,
  chatKey,
  worktree = null,
}: {
  platform: Platform
  tab: PaneTab
  onTab: (tab: PaneTab) => void
  refreshKey: number
  changed: Set<string>
  tool: ToolItem | null
  todos: TodoItem[]
  subagents: ToolItem[]
  terminalOpened: boolean
  /** The session, for the engine's changes, commit card and restore points (L2). */
  state?: SessionState | null
  controller?: ReviewController | null
  /** L4: the chat in view; the Changes and Files panels and the terminal follow its folder. */
  chatKey?: string | null
  /** v1.1: the chat in view's own copy of the project, for "Branch changes". */
  worktree?: WorktreeInfo | null
}) {
  const chats = platform.chats
  const base = worktree?.base ?? null
  const branch = useMemo<BranchSource | null>(() => {
    if (!chatKey || base === null || !chats?.branchChanges || !chats.branchDiff) return null
    return { base, load: () => chats.branchChanges!(chatKey), diff: (path) => chats.branchDiff!(chatKey, path) }
  }, [chats, chatKey, base])
  const tabs: Array<[PaneTab, string]> = [
    ['changes', 'Changes'],
    ['files', 'Files'],
    ['tasks', todos.length ? `Tasks ${todos.filter((todo) => todo.status === 'completed').length}/${todos.length}` : 'Tasks'],
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
      {/* L4: each chat has its own folder, so a chat switch starts these panels afresh. */}
      {tab === 'changes' && (
        <ChangesPanel key={chatKey ?? undefined} platform={platform} refreshKey={refreshKey} tool={tool} state={state} controller={controller} branch={branch} />
      )}
      {tab === 'files' && <FilesPanel key={chatKey ?? undefined} platform={platform} refreshKey={refreshKey} changed={changed} />}
      {tab === 'tasks' && <TasksPanel todos={todos} subagents={subagents} />}
      {/* The terminal stays mounted once opened, so its shell lives on across tabs. */}
      {terminalOpened && (
        <div className="terminal-tab" hidden={tab !== 'terminal'}>
          <Suspense fallback={<div className="muted pad">Loading the terminal…</div>}>
            <TerminalPane platform={platform} folder={chatKey ?? undefined} />
          </Suspense>
        </div>
      )}
    </aside>
  )
}
