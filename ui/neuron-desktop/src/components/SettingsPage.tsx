import { useState } from 'react'

import { type AccountView, type ModelInfo } from '@neuron/client'

import { LINKS } from '../links'
import type { UpdateInfo } from '../platform'
import type { Settings } from '../settings'
import { MODES } from './Composer'

export type SettingsSection = 'account' | 'usage' | 'appearance' | 'sessions' | 'about'

const SECTIONS: Array<[SettingsSection, string]> = [
  ['account', 'Account'],
  ['usage', 'Usage and plan'],
  ['appearance', 'Appearance'],
  ['sessions', 'Sessions'],
  ['about', 'About'],
]

export type UpdateCheck = { state: 'idle' } | { state: 'checking' } | { state: 'latest' } | { state: 'available'; update: UpdateInfo }

function External({ label, url, onOpen }: { label: string; url: string; onOpen: (url: string) => void }) {
  return (
    <button onClick={() => onOpen(url)}>
      {label} <span aria-hidden>↗</span>
      <span className="sr-only"> (opens in your browser)</span>
    </button>
  )
}

function localTime(iso: string | null): string | null {
  if (!iso) return null
  const date = new Date(iso)
  return Number.isNaN(date.getTime()) ? null : date.toLocaleString(undefined, { weekday: 'short', hour: 'numeric', minute: '2-digit' })
}

/** Today's usage as bars, with when it resets and the credit balance. */
export function UsagePanel({ account, onOpenLink }: { account: AccountView; onOpenLink: (url: string) => void }) {
  const resets = localTime(account.resetsAt)
  return (
    <section aria-labelledby="usage-title">
      <h2 id="usage-title">Usage and plan</h2>
      <p>
        Plan: <strong>{account.plan ?? (account.signedIn ? 'Zero-X' : 'Not signed in')}</strong>
      </p>
      {account.meters.length === 0 ? (
        <p className="muted">{account.signedIn ? "Today's usage shows here once Zero-X reports it." : 'Sign in to Zero-X to see your usage.'}</p>
      ) : (
        <ul className="meters">
          {account.meters.map((meter) => (
            <li key={meter.label}>
              <div className="meter-label">
                <span>{meter.label}</span>
                <span className="muted">
                  {meter.used.toLocaleString()} of {meter.limit.toLocaleString()} ({meter.percent}%)
                </span>
              </div>
              <div
                className={`meter${meter.percent >= 90 ? ' high' : ''}`}
                role="progressbar"
                aria-label={meter.label}
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={meter.percent}
              >
                <span style={{ width: `${meter.percent}%` }} />
              </div>
            </li>
          ))}
        </ul>
      )}
      {resets && <p className="muted small">Resets {resets}.</p>}
      {account.creditsUsd !== null && <p>Credit balance: ${account.creditsUsd.toFixed(2)}</p>}
      <div className="dialog-buttons start">
        {account.plan !== 'Max' && <External label="Plans and pricing" url={LINKS.pricing} onOpen={onOpenLink} />}
        <External label="Top up credits" url={LINKS.pricing} onOpen={onOpenLink} />
      </div>
    </section>
  )
}

export function SettingsPage({
  section,
  onSection,
  settings,
  onChange,
  account,
  models,
  appVersion,
  engineVersion,
  where,
  canUpdate,
  update,
  onCheckUpdates,
  onOpenLink,
  onSignIn,
  onSignOut,
  onClose,
}: {
  section: SettingsSection
  onSection: (section: SettingsSection) => void
  settings: Settings
  onChange: (change: Partial<Settings>) => void
  account: AccountView
  models: ModelInfo[]
  appVersion: string | null
  engineVersion: string | null
  where: string
  canUpdate: boolean
  update: UpdateCheck
  onCheckUpdates: () => void
  onOpenLink: (url: string) => void
  onSignIn: () => void
  onSignOut: () => void
  onClose: () => void
}) {
  const [copied, setCopied] = useState(false)
  const versionText = `Neuron Desktop ${appVersion ?? 'dev'} · engine ${engineVersion ?? 'not connected'}`
  const copyVersion = () => {
    void navigator.clipboard?.writeText(versionText).then(
      () => {
        setCopied(true)
        setTimeout(() => setCopied(false), 1500)
      },
      () => {},
    )
  }
  return (
    <div className="page settings-page">
      <header className="page-header">
        <h1>Settings</h1>
        <button className="icon-button" aria-label="Close settings" title="Close (Esc)" onClick={onClose}>
          ✕
        </button>
      </header>
      <div className="settings-body">
        <nav className="settings-nav" aria-label="Settings sections">
          {SECTIONS.map(([id, label]) => (
            <button key={id} className={section === id ? 'active' : ''} aria-current={section === id ? 'page' : undefined} onClick={() => onSection(id)}>
              {label}
            </button>
          ))}
        </nav>
        <div className="settings-content">
          {section === 'account' && (
            <section aria-labelledby="account-title">
              <h2 id="account-title">Account</h2>
              {account.signedIn ? (
                <>
                  <div className="account-card">
                    <span className="avatar big" aria-hidden>
                      {account.initials}
                    </span>
                    <div>
                      <div className="account-name">{account.name}</div>
                      <div className="muted">{account.email}</div>
                      {account.plan && <span className="plan-badge">{account.plan}</span>}
                    </div>
                  </div>
                  <div className="dialog-buttons start">
                    <External label="Manage your account" url={LINKS.account} onOpen={onOpenLink} />
                    <button className="danger" onClick={onSignOut}>
                      Sign out
                    </button>
                  </div>
                </>
              ) : (
                <>
                  <p className="muted">Not signed in to Zero-X. Sessions run through your own provider key, if one is set up.</p>
                  <button className="primary" onClick={onSignIn}>
                    Sign in to Zero-X
                  </button>
                </>
              )}
            </section>
          )}
          {section === 'usage' && <UsagePanel account={account} onOpenLink={onOpenLink} />}
          {section === 'appearance' && (
            <section aria-labelledby="appearance-title" className="settings">
              <h2 id="appearance-title">Appearance</h2>
              <label>
                Theme
                <select value={settings.theme} onChange={(event) => onChange({ theme: event.target.value as Settings['theme'] })}>
                  <option value="dark">Dark</option>
                  <option value="light">Light</option>
                  <option value="system">Same as the system</option>
                </select>
              </label>
              <label>
                Sidebar
                <select value={settings.sidebar} onChange={(event) => onChange({ sidebar: event.target.value as Settings['sidebar'] })}>
                  <option value="auto">Icons only when the window is narrow</option>
                  <option value="full">Always full</option>
                  <option value="rail">Always icons only</option>
                </select>
              </label>
              <label className="check">
                <input type="checkbox" checked={settings.enterSends} onChange={(event) => onChange({ enterSends: event.target.checked })} />
                Enter sends (Shift+Enter for a new line). Off: Ctrl+Enter sends.
              </label>
              <label className="check">
                <input type="checkbox" checked={settings.showReasoning} onChange={(event) => onChange({ showReasoning: event.target.checked })} />
                Show the model's reasoning when the provider streams it
              </label>
              <label className="check">
                <input type="checkbox" checked={settings.foldTools} onChange={(event) => onChange({ foldTools: event.target.checked })} />
                Fold tool calls (edits and failures stay open)
              </label>
            </section>
          )}
          {section === 'sessions' && (
            <section aria-labelledby="sessions-title" className="settings">
              <h2 id="sessions-title">Sessions</h2>
              <p className="muted small">These apply to sessions started from now on.</p>
              <label>
                Default model
                <input
                  list="model-ids"
                  value={settings.defaultModel}
                  placeholder="The engine's default"
                  onChange={(event) => onChange({ defaultModel: event.target.value })}
                />
                <datalist id="model-ids">
                  {models.map((model) => (
                    <option key={model.id} value={model.id}>
                      {model.display_name ?? model.id}
                    </option>
                  ))}
                </datalist>
              </label>
              <label>
                Default permission mode
                <select
                  value={settings.defaultPermissionMode}
                  onChange={(event) => onChange({ defaultPermissionMode: event.target.value as Settings['defaultPermissionMode'] })}
                >
                  {MODES.map((mode) => (
                    <option key={mode.id} value={mode.id}>
                      {mode.label}: {mode.help}
                    </option>
                  ))}
                </select>
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  checked={settings.rememberModePerFolder}
                  onChange={(event) => onChange({ rememberModePerFolder: event.target.checked })}
                />
                Start each folder in the mode it was last left in
              </label>
            </section>
          )}
          {section === 'about' && (
            <section aria-labelledby="about-title">
              <h2 id="about-title">About</h2>
              <p>
                <button className="version-button" onClick={copyVersion} aria-label={`${versionText}. Copy`}>
                  {versionText}
                </button>{' '}
                <span className="muted small" role="status">
                  {copied ? 'Copied' : 'Click to copy'}
                </span>
              </p>
              <p className="muted small">The engine runs on {where}.</p>
              {canUpdate && (
                <div className="dialog-buttons start">
                  <button onClick={onCheckUpdates} disabled={update.state === 'checking'}>
                    {update.state === 'checking' ? 'Checking…' : 'Check for updates'}
                  </button>
                  <span className="muted small" role="status">
                    {update.state === 'latest' && 'Neuron is up to date.'}
                    {update.state === 'available' && `Neuron ${update.update.version} is available.`}
                  </span>
                  {update.state === 'available' && <External label="Download" url={LINKS.download} onOpen={onOpenLink} />}
                </div>
              )}
              <div className="dialog-buttons start">
                <External label="What's new" url={LINKS.changelog} onOpen={onOpenLink} />
                <External label="Help and support" url={LINKS.help} onOpen={onOpenLink} />
              </div>
              <p className="muted small">Neuron Desktop includes open-source software, listed with its licences in the installed app's licence files.</p>
            </section>
          )}
        </div>
      </div>
    </div>
  )
}
