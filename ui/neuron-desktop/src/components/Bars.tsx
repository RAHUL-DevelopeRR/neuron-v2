import { formatTokens, sessionAccount, type SessionState } from '@neuron/client'

import { modelLabel } from '../modelNames'

/** The last part of a model or route id: `nvidia/nvidia/nemotron-3` is `nemotron-3`. */
function modelPart(id: string): string {
  return id.split('/').filter(Boolean).pop() ?? id
}

/** The same model, whichever route served it (`groq/openai/gpt-oss-120b` serves `openai/gpt-oss-120b`). */
function sameModel(a: string, b: string): boolean {
  return a === b || modelPart(a).toLowerCase() === modelPart(b).toLowerCase()
}

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

/** From this share of the context window or of today's allowance the bar tints or warns. */
const WARN_AT = 80

/** The quota line: nothing below 80% of today's requests or tokens, a quiet warning from there, the reset time at the limit. */
function quotaNote(state: SessionState): { text: string; limit: boolean } | null {
  const view = sessionAccount(state)
  const worst = view.meters
    .map((meter) => ({ meter, share: (meter.used / meter.limit) * 100 }))
    .sort((a, b) => b.share - a.share)[0]
  if (!worst || worst.share < WARN_AT) return null
  const what = worst.meter.label.startsWith('Requests') ? 'requests' : 'tokens'
  if (worst.share >= 100) {
    const at = view.resetsAt ? new Date(view.resetsAt) : null
    const time = at && !Number.isNaN(at.getTime()) ? at.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' }) : null
    return { text: time ? `Limit reached, resets at ${time}` : 'Limit reached', limit: true }
  }
  return { text: `${Math.floor(worst.share)}% of today's ${what} used`, limit: false }
}

export function StatusBar({ state, onPane }: { state: SessionState; onPane: (pane: 'terminal' | 'tasks') => void }) {
  const usage = state.usage
  const context = state.status?.context_percent ?? 0
  const status = state.status
  const model = status?.model ?? ''
  const served = status?.served_model
  const streaming = state.busy ? (state.turnProgress?.outputTokens ?? 0) : 0
  const quota = quotaNote(state)
  return (
    <footer className="status-bar">
      <button className="status-key status-low" onClick={() => onPane('terminal')}>
        Terminal <kbd>Ctrl+`</kbd>
      </button>
      <span className={`context-meter${context >= WARN_AT ? ' warn' : ''}`} title="Share of the model's context the chat uses">
        Context {context}%
      </span>
      {usage && (
        <span className="status-low" title={usage.estimated ? 'estimated' : 'tokens in this chat'}>
          ↑{formatTokens(usage.cumulative.input)} ↓{formatTokens(usage.cumulative.output)}
        </span>
      )}
      {streaming > 0 && <span className="turn-tokens">writing {formatTokens(streaming)} tokens</span>}
      {quota && (
        <span className={`quota${quota.limit ? ' limit' : ''}`} role="status">
          {quota.text}
        </span>
      )}
      {state.todos.length > 0 && (
        <button className="status-link status-low" onClick={() => onPane('tasks')}>
          Tasks {state.todos.filter((todo) => todo.status === 'completed').length}/{state.todos.length}
        </button>
      )}
      <div className="spacer" />
      {state.busy && <span className="busy-label">working…</span>}
      {served && !sameModel(served, model) && (
        <span className="served-model status-low" title="The model that answered, when it is not the one you picked">
          answered by {modelLabel(served, state.models, { name: status?.served_model_display_name })}
        </span>
      )}
      <span className="model-tag">{modelLabel(model, state.models, { name: status?.model_display_name, provider: status?.provider })}</span>
    </footer>
  )
}
