import { useEffect, useState } from 'react'

import type { GatewayProblem } from '@neuron/client'

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
