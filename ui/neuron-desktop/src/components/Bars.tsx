import { formatTokens, needsSignIn, type SessionState } from '@neuron/client'

import { accountLabel } from './SignIn'

/** Above the chat: its title, the branch, trust, and who is signed in. */
export function ChatHeader({
  state,
  title,
  where,
  sidePaneOpen,
  onToggleSidePane,
}: {
  state: SessionState
  title: string
  where: string
  sidePaneOpen: boolean
  onToggleSidePane: () => void
}) {
  const status = state.status
  const git = status?.git
  return (
    <header className="chat-header">
      <div className="chat-header-title" title={title}>
        {title}
      </div>
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
      <span className="account">{accountLabel(status?.account, status?.signed_in, needsSignIn(state))}</span>
      <button className="icon-button" aria-pressed={sidePaneOpen} title="Show or hide the side panel (Ctrl+B)" onClick={onToggleSidePane}>
        ◨
      </button>
    </header>
  )
}

/** An untrusted folder runs without its own settings; trusting it needs a restart to load them. */
export function TrustBanner({
  state,
  canTrust,
  canRestart,
  onTrust,
  onRestart,
}: {
  state: SessionState
  canTrust: boolean
  canRestart: boolean
  onTrust: () => void
  onRestart: () => void
}) {
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
  if (state.status && !state.status.workspace_trusted && canTrust) {
    return (
      <div className="banner warn" role="status">
        <span>
          This folder isn't trusted, so its own settings, hooks and MCP servers are off. Trust it only if you trust its contents.
        </span>
        <button onClick={onTrust}>Trust folder</button>
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
