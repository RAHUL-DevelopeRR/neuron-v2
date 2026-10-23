import { useEffect, useRef, useState } from 'react'

import { planLine, type AccountView } from '@neuron/client'

import { useFocusTrap, useMenuKeys } from '../a11y'
import { LINKS } from '../links'

export type AccountAction = 'settings' | 'usage' | 'whats-new' | 'sign-out'

function MenuLink({ label, href, onOpen }: { label: string; href: string; onOpen: (url: string) => void }) {
  return (
    <button role="menuitem" className="menu-item" onClick={() => onOpen(href)}>
      <span>{label}</span>
      <span className="menu-hint" aria-hidden>
        ↗
      </span>
      <span className="sr-only"> (opens in your browser)</span>
    </button>
  )
}

/** The account menu: who is signed in, today's usage, settings, Zero-X pages, and sign out. */
export function AccountMenu({
  account,
  onAction,
  onOpenLink,
  onClose,
}: {
  account: AccountView
  onAction: (action: AccountAction) => void
  onOpenLink: (url: string) => void
  onClose: () => void
}) {
  const menu = useRef<HTMLDivElement>(null)
  useMenuKeys(menu, true, onClose)
  useEffect(() => {
    const onDown = (event: MouseEvent) => {
      if (menu.current && !menu.current.contains(event.target as Node) && !(event.target as Element).closest?.('.account-row')) onClose()
    }
    window.addEventListener('mousedown', onDown)
    return () => window.removeEventListener('mousedown', onDown)
  }, [onClose])
  const line = planLine(account)
  const act = (action: AccountAction) => {
    onClose()
    onAction(action)
  }
  const open = (url: string) => {
    onClose()
    onOpenLink(url)
  }
  return (
    <div className="account-menu" role="menu" aria-label="Account" ref={menu}>
      <div className="menu-account" role="presentation">
        <div className="menu-email">{account.email ?? 'Zero-X account'}</div>
        {line && <div className="muted small">{line}</div>}
      </div>
      <div className="menu-separator" role="separator" />
      <button role="menuitem" className="menu-item" onClick={() => act('settings')}>
        <span>Settings</span>
        <kbd>Ctrl+,</kbd>
      </button>
      <button role="menuitem" className="menu-item" onClick={() => act('usage')}>
        <span>Usage and plan</span>
      </button>
      {account.plan !== 'Max' && <MenuLink label="Plans and pricing" href={LINKS.pricing} onOpen={open} />}
      <MenuLink label="API keys" href={LINKS.apiKeys} onOpen={open} />
      <div className="menu-separator" role="separator" />
      <MenuLink label="Download Neuron" href={LINKS.download} onOpen={open} />
      <button role="menuitem" className="menu-item" onClick={() => act('whats-new')}>
        <span>What's new</span>
      </button>
      <MenuLink label="Help and support" href={LINKS.help} onOpen={open} />
      <div className="menu-separator" role="separator" />
      <button role="menuitem" className="menu-item" onClick={() => act('sign-out')}>
        <span>Sign out</span>
      </button>
    </div>
  )
}

/** The bottom-left row: initials, name and plan, opening the account menu; or a sign-in button. */
export function AccountRow({
  account,
  rail,
  onSignIn,
  onAction,
  onOpenLink,
  onSearch,
}: {
  account: AccountView
  rail: boolean
  onSignIn: () => void
  onAction: (action: AccountAction) => void
  onOpenLink: (url: string) => void
  onSearch: () => void
}) {
  const [open, setOpen] = useState(false)
  const button = useRef<HTMLButtonElement>(null)
  const close = () => {
    setOpen(false)
    button.current?.focus()
  }
  // Signed out, or chatting through an own provider key: Zero-X is one click away.
  if (!account.signedIn) {
    return (
      <div className="account-area">
        <button className={`sign-in-row${rail ? ' rail' : ''}`} onClick={onSignIn} aria-label={rail ? 'Sign in to Zero-X' : undefined} title="Sign in to Zero-X">
          {rail ? '→' : 'Sign in to Zero-X'}
        </button>
      </div>
    )
  }
  return (
    <div className="account-area">
      {open && <AccountMenu account={account} onAction={onAction} onOpenLink={onOpenLink} onClose={close} />}
      <div className="account-line">
        <button
          ref={button}
          className="account-row"
          aria-haspopup="menu"
          aria-expanded={open}
          aria-label={`Account: ${account.name ?? account.email ?? 'Zero-X'}${account.plan ? `, ${account.plan} plan` : ''}`}
          onClick={() => setOpen(!open)}
        >
          <span className="avatar" aria-hidden>
            {account.initials}
          </span>
          {!rail && (
            <span className="account-text" aria-hidden>
              <span className="account-name">{account.name ?? account.email}</span>
              {account.plan && <span className="plan-badge">{account.plan}</span>}
            </span>
          )}
        </button>
        {!rail && (
          <button className="icon-button" aria-label="Find a session" title="Find a session (Ctrl+K)" onClick={onSearch}>
            ⌕
          </button>
        )}
      </div>
    </div>
  )
}

/** "Sign out of Zero-X?" before signing out. */
export function SignOutDialog({ email, onConfirm, onCancel }: { email: string | null; onConfirm: () => void; onCancel: () => void }) {
  const dialog = useRef<HTMLDivElement>(null)
  useFocusTrap(dialog)
  return (
    <div className="dialog-backdrop" onMouseDown={(event) => event.target === event.currentTarget && onCancel()}>
      <div
        className="dialog small-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="sign-out-title"
        aria-describedby="sign-out-text"
        ref={dialog}
        onKeyDown={(event) => event.key === 'Escape' && onCancel()}
      >
        <h2 id="sign-out-title">Sign out of Zero-X?</h2>
        <p id="sign-out-text" className="muted">
          {email ? `${email} will be signed out on this device.` : 'You will be signed out on this device.'} Your sessions stay here; sign in again to
          continue them.
        </p>
        <div className="dialog-buttons">
          <button onClick={onCancel} data-autofocus>
            Cancel
          </button>
          <button className="danger" onClick={onConfirm}>
            Sign out
          </button>
        </div>
      </div>
    </div>
  )
}
