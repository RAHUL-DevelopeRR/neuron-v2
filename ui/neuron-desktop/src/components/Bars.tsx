import { formatTokens, needsSignIn, type ModelInfo, type SessionState } from '@neuron/client'

import { ModelPicker } from './ModelPicker'
import { PixelLogo } from './PixelLogo'
import { accountLabel } from './SignIn'

const basename = (path: string) => path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path

export function TopBar({
  state,
  where,
  onListModels,
  onPickModel,
}: {
  state: SessionState
  where: string
  onListModels: () => void
  onPickModel: (model: string) => void
}) {
  const status = state.status
  const git = status?.git
  return (
    <header className="top-bar">
      <PixelLogo pixel={3} />
      <div className="workspace" title={status?.cwd}>
        <strong>{status ? basename(status.cwd) : 'Neuron'}</strong>
        {git?.branch && (
          <span className="git">
            ⎇ {git.branch}
            {git.changed > 0 && <span className="git-changes"> ● {git.changed}</span>}
          </span>
        )}
        {status && !status.workspace_trusted && (
          <span className="badge warn" title="Untrusted workspaces run without their own settings, hooks and MCP servers.">
            untrusted
          </span>
        )}
        {status?.plan_mode && <span className="badge">plan mode</span>}
      </div>
      <div className="spacer" />
      <span className={`connection ${state.connection}`} title={state.connectionError ?? `Engine on ${where}`}>
        {state.connection === 'ready' ? `engine ${state.version ?? ''}` : state.connection}
      </span>
      <span className="account">{accountLabel(status?.account, status?.signed_in, needsSignIn(state))}</span>
      <ModelPicker
        current={status?.model ?? ''}
        models={state.models as ModelInfo[]}
        disabled={state.connection !== 'ready'}
        onOpen={onListModels}
        onPick={onPickModel}
      />
    </header>
  )
}

export function StatusBar({
  state,
  terminalOpen,
  onToggleTerminal,
}: {
  state: SessionState
  terminalOpen: boolean
  onToggleTerminal: () => void
}) {
  const usage = state.usage
  return (
    <footer className="status-bar">
      <button className="status-key" onClick={onToggleTerminal} aria-pressed={terminalOpen}>
        {terminalOpen ? 'Hide terminal' : 'Terminal'} <kbd>Ctrl+`</kbd>
      </button>
      <span>Context {state.status?.context_percent ?? 0}%</span>
      {usage && (
        <span title={usage.estimated ? 'estimated' : undefined}>
          ↑{formatTokens(usage.cumulative.input)} ↓{formatTokens(usage.cumulative.output)}
        </span>
      )}
      <span>{state.status?.permission_mode ?? ''}</span>
      <div className="spacer" />
      {state.busy && <span className="busy-label">working…</span>}
      <span className="model-tag">{state.status?.model ?? ''}</span>
    </footer>
  )
}
