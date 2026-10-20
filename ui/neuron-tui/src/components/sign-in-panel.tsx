/**
 * Zero-X sign-in, above the input. While the engine waits for a sign-in it
 * says how to start one (`/login`). During a `/login` it shows the link and
 * the device code, counts down to the code's expiry, and cancels on Esc (an
 * engine interrupt). The engine does the sign-in and keeps the token; the UI
 * only ever sees the code.
 */

import { TextAttributes, type KeyEvent } from '@opentui/core'
import { useKeyboard } from '@opentui/react'
import React, { useCallback, useEffect, useState } from 'react'

import { TerminalLink } from './terminal-link'
import { getNeuronEngineClient } from '../engine/neuron-engine-client'
import { useTheme } from '../hooks/use-theme'
import { needsSignIn, useEngineStore, type EngineStoreState } from '../state/engine-store'
import { logger } from '../utils/logger'
import { safeOpen } from '../utils/open-url'
import { BORDER_CHARS } from '../utils/ui-constants'

import type { StatusEvent } from '../engine/protocol'

export type SignInView =
  | { kind: 'signed_out' }
  | { kind: 'starting' }
  | {
      kind: 'code'
      url: string
      userCode: string
      remainingSecs: number
      browserOpened: boolean
    }

/** What the panel shows, or `null` when there is nothing to sign in to. */
export function signInView(
  status: StatusEvent | null,
  signIn: EngineStoreState['signIn'],
  now: number,
): SignInView | null {
  if (signIn?.approved) return { kind: 'starting' }
  if (signIn) {
    const elapsed = Math.floor((now - signIn.receivedAt) / 1000)
    return {
      kind: 'code',
      url: signIn.url_complete ?? signIn.url,
      userCode: signIn.user_code,
      remainingSecs: Math.max(0, signIn.expires_in_secs - elapsed),
      browserOpened: signIn.browser_opened,
    }
  }
  return needsSignIn(status) ? { kind: 'signed_out' } : null
}

/** `m:ss`. */
export function formatRemaining(secs: number): string {
  const minutes = Math.floor(secs / 60)
  const seconds = secs % 60
  return `${minutes}:${seconds.toString().padStart(2, '0')}`
}

export const SignInPanel = ({ width }: { width: number }) => {
  const theme = useTheme()
  const status = useEngineStore((state) => state.status)
  const signIn = useEngineStore((state) => state.signIn)
  const [now, setNow] = useState(() => Date.now())

  useEffect(() => {
    if (!signIn) return
    setNow(Date.now())
    const timer = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(timer)
  }, [signIn])

  useKeyboard(
    useCallback(
      (key: KeyEvent) => {
        if (!signIn || signIn.approved || key.name !== 'escape') return
        key.preventDefault?.()
        try {
          getNeuronEngineClient().interrupt()
        } catch (error) {
          logger.warn({ error }, '[sign-in] cancel failed')
        }
      },
      [signIn],
    ),
  )

  const view = signInView(status, signIn, now)
  if (!view) return null

  return (
    <box
      title=" Sign in to Zero-X "
      titleAlignment="center"
      style={{
        width: Math.max(20, width),
        borderStyle: 'single',
        borderColor: theme.primary,
        customBorderChars: BORDER_CHARS,
        paddingLeft: 1,
        paddingRight: 1,
        flexDirection: 'column',
      }}
    >
      {view.kind === 'starting' ? (
        <text style={{ wrapMode: 'word', fg: theme.foreground }}>
          Signed in. Starting your session…
        </text>
      ) : view.kind === 'signed_out' ? (
        <text style={{ wrapMode: 'word', fg: theme.foreground }}>
          <span>Not signed in. Type </span>
          <span fg={theme.primary} attributes={TextAttributes.BOLD}>
            /login
          </span>
          <span> to sign in with your browser.</span>
        </text>
      ) : (
        <>
          <text style={{ wrapMode: 'word', fg: theme.foreground }}>
            {view.browserOpened
              ? 'Your browser opened the sign-in page. If it did not, open:'
              : 'Open this page to sign in:'}
          </text>
          <TerminalLink
            text={view.url}
            maxWidth={Math.max(10, width - 4)}
            lineWrap={true}
            onActivate={() => safeOpen(view.url)}
          />
          <text style={{ wrapMode: 'word', paddingTop: 1 }}>
            <span fg={theme.foreground}>Code </span>
            <span fg={theme.primary} attributes={TextAttributes.BOLD}>
              {view.userCode}
            </span>
            <span fg={theme.muted}>
              {view.remainingSecs > 0
                ? `   expires in ${formatRemaining(view.remainingSecs)}`
                : '   expired: press Esc and run /login again'}
            </span>
          </text>
          <text style={{ fg: theme.muted }}>Waiting for approval · esc cancel</text>
        </>
      )}
    </box>
  )
}
