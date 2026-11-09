import type { GatewayProblem, SignInCodeEvent } from '@neuron/client'

import { ProblemCard } from './ProblemCard'

/** The Zero-X sign-in: a device code the user approves in the browser. */
export function SignInPanel({
  code,
  starting,
  busy,
  problem,
  onSignIn,
  onCancel,
  onOpen,
}: {
  code: SignInCodeEvent | null
  starting: boolean
  busy: boolean
  /** Why the last sign-in failed, when the gateway could not be used. */
  problem: GatewayProblem | null
  onSignIn: () => void
  onCancel: () => void
  onOpen: (url: string) => void
}) {
  if (starting) {
    return (
      <div className="sign-in" role="status">
        <h2>Starting your session…</h2>
        <p className="muted">Neuron is starting its tools and MCP servers. This can take a little while.</p>
      </div>
    )
  }
  if (code) {
    const link = code.url_complete ?? code.url
    return (
      <div className="sign-in">
        <h2>Approve this sign-in</h2>
        <p>
          Open{' '}
          <a
            href={link}
            onClick={(event) => {
              event.preventDefault()
              onOpen(link)
            }}
          >
            {code.url}
          </a>{' '}
          and enter this code:
        </p>
        <div className="user-code" aria-label="sign-in code">
          {code.user_code}
        </div>
        <p className="muted">
          {code.browser_opened ? 'Your browser should have opened. ' : ''}The code expires in{' '}
          {Math.max(1, Math.round(code.expires_in_secs / 60))} minutes.
        </p>
        <div className="dialog-buttons">
          <button className="primary" onClick={() => onOpen(link)}>
            Open the sign-in page
          </button>
          <button onClick={onCancel}>Cancel</button>
        </div>
      </div>
    )
  }
  if (problem && !busy) {
    return (
      <div className="sign-in">
        <ProblemCard problem={problem} actions={{ onSignIn, onRetry: onSignIn, onOpenLink: onOpen }} />
      </div>
    )
  }
  return (
    <div className="sign-in">
      <h2>Sign in to Zero-X</h2>
      <p>Neuron runs your sessions through the Zero-X gateway. Sign in to start.</p>
      <div className="dialog-buttons">
        <button className="primary" onClick={onSignIn} disabled={busy} data-autofocus>
          Sign in
        </button>
      </div>
    </div>
  )
}
