import { formatTokens, type SessionState } from '@neuron/client'

/** Above the chat: its title, the project and branch, and the engine. */
export function ChatHeader({
  state,
  title,
  project,
  where,
  sidePaneOpen,
  onToggleSidePane,
}: {
  state: SessionState
  title: string
  /** The open project's name. */
  project: string | null
  where: string
  sidePaneOpen: boolean
  onToggleSidePane: () => void
}) {
  const status = state.status
  const git = status?.git
  return (
    <header className="chat-header">
      {project && <span className="chat-header-project">{project}</span>}
      <h1 className="chat-header-title" title={title}>
        {title}
      </h1>
      {git?.branch && (
        <span className="git" title={`${git.changed} changed`}>
          ⎇ {git.branch}
          {git.changed > 0 && <span className="git-changes"> ● {git.changed}</span>}
        </span>
      )}
      {status?.plan_mode && <span className="badge">plan mode</span>}
      <div className="spacer" />
      <span className={`connection ${state.connection}`} title={state.connectionError ?? `Engine on ${where}`}>
        {state.connection === 'ready' ? `engine ${state.version ?? ''}` : state.connection}
      </span>
      <button
        className="icon-button"
        aria-pressed={sidePaneOpen}
        aria-label="Side panel"
        title="Show or hide the side panel (Ctrl+B)"
        onClick={onToggleSidePane}
      >
        ◨
      </button>
    </header>
  )
}

/** After the folder was allowed: its own settings, hooks and MCP servers load once the engine restarts. */
export function RestartBanner({ state, canRestart, onRestart }: { state: SessionState; canRestart: boolean; onRestart: () => void }) {
  if (state.restartRequired) {
    return (
      <div className="banner" role="status">
        <span>Trusted. Restart the engine to load this folder's settings, hooks and MCP servers; the chat continues.</span>
        {canRestart && (
          <button className="primary" onClick={onRestart}>
            Restart engine
          </button>
        )}
      </div>
    )
  }
  return null
}

export function StatusBar({ state, onPane }: { state: SessionState; onPane: (pane: 'terminal' | 'tasks') => void }) {
  const usage = state.usage
  return (
    <footer className="status-bar">
      <button className="status-key" onClick={() => onPane('terminal')}>
        Terminal <kbd>Ctrl+`</kbd>
      </button>
      <span title="Share of the model's context the chat uses">Context {state.status?.context_percent ?? 0}%</span>
      {usage && (
        <span title={usage.estimated ? 'estimated' : 'tokens in this chat'}>
          ↑{formatTokens(usage.cumulative.input)} ↓{formatTokens(usage.cumulative.output)}
        </span>
      )}
      {state.todos.length > 0 && (
        <button className="status-link" onClick={() => onPane('tasks')}>
          Tasks {state.todos.filter((todo) => todo.status === 'completed').length}/{state.todos.length}
        </button>
      )}
      <div className="spacer" />
      {state.busy && <span className="busy-label">working…</span>}
      <span className="model-tag">{state.status?.model ?? ''}</span>
    </footer>
  )
}
