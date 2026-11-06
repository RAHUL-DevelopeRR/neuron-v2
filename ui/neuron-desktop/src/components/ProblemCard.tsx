import { useEffect, useState } from 'react'

import type { GatewayProblem } from '@neuron/client'

import type { EngineProblem, EngineProblemCode } from '../engineProblems'
import { LINKS } from '../links'

export interface ProblemActions {
  onSignIn: () => void
  /** Sends the prompt the failure stopped again; absent when there is none. */
  onRetry?: () => void
  onOpenLink: (url: string) => void
}

/** "5:30 AM", in the user's own time. */
function localTime(iso: string): string | null {
  const date = new Date(iso)
  return Number.isNaN(date.getTime()) ? null : date.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })
}

/** Seconds left of `total`, counting down from when it mounted. */
function useCountdown(total: number | null): number {
  const [left, setLeft] = useState(total ?? 0)
  useEffect(() => {
    if (!total) return
    setLeft(total)
    const started = Date.now()
    const timer = setInterval(() => {
      const remaining = Math.max(0, total - Math.floor((Date.now() - started) / 1000))
      setLeft(remaining)
      if (remaining === 0) clearInterval(timer)
    }, 1000)
    return () => clearInterval(timer)
  }, [total])
  return left
}

function External({ label, url, onOpen, primary }: { label: string; url: string; onOpen: (url: string) => void; primary?: boolean }) {
  return (
    <button className={primary ? 'primary' : undefined} onClick={() => onOpen(url)}>
      {label} <span aria-hidden>↗</span>
      <span className="sr-only"> (opens in your browser)</span>
    </button>
  )
}

/** A gateway failure as a card with what to do next, instead of the raw error. */
export function ProblemCard({ problem, actions }: { problem: GatewayProblem; actions: ProblemActions }) {
  const left = useCountdown(problem.kind === 'busy' ? problem.retryAfterSeconds : null)
  let title: string
  let body: string
  let buttons: React.ReactNode
  switch (problem.kind) {
    case 'sign_in':
      title = 'Sign in again'
      body = 'Your Zero-X sign-in has expired or was revoked. Sign in again and your message is still here to send.'
      buttons = (
        <button className="primary" onClick={actions.onSignIn}>
          Sign in to Zero-X
        </button>
      )
      break
    case 'upgrade':
      title = 'This needs a paid plan'
      body = 'The model or feature you asked for is not included in your plan.'
      buttons = <External primary label="Plans and pricing" url={problem.upgradeUrl ?? LINKS.pricing} onOpen={actions.onOpenLink} />
      break
    case 'quota': {
      const at = problem.resetsAt ? localTime(problem.resetsAt) : null
      title = "You've used today's quota"
      body = at ? `It resets at ${at}. Your message is kept until then.` : 'It resets tomorrow. Your message is kept until then.'
      buttons = <External label="Plans and pricing" url={problem.upgradeUrl ?? LINKS.pricing} onOpen={actions.onOpenLink} />
      break
    }
    case 'busy':
      title = 'Zero-X is busy'
      body = left > 0 ? `Try again in ${left} s.` : 'Try again now.'
      buttons = actions.onRetry && (
        <button className="primary" onClick={actions.onRetry} disabled={left > 0}>
          Try again
        </button>
      )
      break
    case 'unreachable':
      title = "Can't reach Zero-X right now"
      body = 'Check your connection, then retry. Your message is kept.'
      buttons = (
        <>
          {actions.onRetry && (
            <button className="primary" onClick={actions.onRetry}>
              Retry
            </button>
          )}
          <External label="Use my own provider key" url={LINKS.providerKeys} onOpen={actions.onOpenLink} />
        </>
      )
      break
  }
  return (
    <section className={`problem-card problem-${problem.kind}`} role="alert" aria-label={title}>
      <h3>{title}</h3>
      <p>{body}</p>
      <details className="problem-detail">
        <summary>Details</summary>
        <code>{problem.message}</code>
      </details>
      {buttons && <div className="dialog-buttons">{buttons}</div>}
    </section>
  )
}

export interface EngineProblemActions {
  /** Starts the engine again; absent where the app cannot. */
  onRestart?: () => void
  /** Puts the support report on the clipboard. */
  onCopyDiagnostics: () => Promise<void>
  onOpenLink: (url: string) => void
}

const ENGINE_TITLE: Record<EngineProblemCode, string> = {
  missing: "Neuron's engine is missing",
  quarantined: 'Security software blocked the engine',
  denied: 'The engine is not allowed to run',
  crashed: 'The engine stopped unexpectedly',
  failed: 'The engine could not start',
  restart_failed: 'The engine did not restart',
}

/** What to do, for the cases whose message does not already say. */
const ENGINE_HINT: Partial<Record<EngineProblemCode, string>> = {
  crashed: 'Try again. If it keeps happening, copy the diagnostics and send them with your report.',
  failed: 'Try again, or reinstall Neuron if it keeps happening.',
  restart_failed: 'Try again, or reinstall Neuron if it keeps happening.',
}

/** L5: an engine problem as a card: what happened, and what to do about it. */
export function EngineProblemCard({ problem, actions }: { problem: EngineProblem; actions: EngineProblemActions }) {
  const [copied, setCopied] = useState(false)
  const [copyFailed, setCopyFailed] = useState(false)
  const title = ENGINE_TITLE[problem.code]
  const copy = () => {
    setCopyFailed(false)
    void actions.onCopyDiagnostics().then(
      () => {
        setCopied(true)
        setTimeout(() => setCopied(false), 1500)
      },
      () => setCopyFailed(true),
    )
  }
  return (
    <section className={`problem-card problem-${problem.code}`} role="alert" aria-label={title}>
      <h3>{title}</h3>
      {problem.code === 'crashed' ? (
        <>
          <p>The Neuron engine stopped on its own{problem.exitCode ? ` (exit code ${problem.exitCode})` : ''}.</p>
          {problem.lastMessage && (
            <p className="muted small">
              Its last message: <code>{problem.lastMessage}</code>
            </p>
          )}
        </>
      ) : (
        <p>{problem.message}</p>
      )}
      {ENGINE_HINT[problem.code] && <p>{ENGINE_HINT[problem.code]}</p>}
      <div className="dialog-buttons">
        {actions.onRestart && (
          <button className="primary" onClick={actions.onRestart}>
            Try again
          </button>
        )}
        <button onClick={copy}>Copy diagnostics</button>
        <External label="Open help" url={LINKS.help} onOpen={actions.onOpenLink} />
        <span className="muted small" role="status">
          {copied ? 'Copied' : copyFailed ? 'Could not copy' : ''}
        </span>
      </div>
    </section>
  )
}
