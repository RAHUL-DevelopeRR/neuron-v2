/**
 * Onboarding's sign-in (T2, step 2): the first screen when the UI starts with
 * no usable Zero-X login, so a fresh install (or one after `neuron logout`)
 * signs in before the folder's trust (T1) and the chat. Enter starts the
 * engine's device sign-in, whose link and code the sign-in panel shows; Esc
 * leaves it for later (the chat's panel then says how to sign in); Ctrl-C
 * quits.
 */

import { TextAttributes, type KeyEvent } from '@opentui/core'
import { useKeyboard } from '@opentui/react'
import React, { useCallback } from 'react'

import { SignInPanel, signInView } from './sign-in-panel'
import { useTheme } from '../hooks/use-theme'
import { useEngineStore } from '../state/engine-store'
import { getGlyphMode, type GlyphMode } from '../utils/glyphs'

/** What a key does on the onboarding sign-in; `signingIn` while a sign-in runs. */
export function onboardingKeyAction(
  key: Pick<KeyEvent, 'name' | 'ctrl'>,
  signingIn: boolean,
): 'sign_in' | 'later' | 'quit' | 'none' {
  if (key.ctrl && key.name === 'c') return 'quit'
  // During a sign-in, Esc cancels it (the sign-in panel's own key).
  if (signingIn) return 'none'
  if (key.name === 'return' || key.name === 'enter') return 'sign_in'
  if (key.name === 'escape') return 'later'
  return 'none'
}

export interface OnboardingSignInProps {
  width: number
  onSignIn: () => void
  onLater: () => void
  onQuit: () => void
  /** Test seam: the console glyph set marks the choice with `>`. */
  glyphMode?: GlyphMode
}

export const OnboardingSignIn = ({
  width,
  onSignIn,
  onLater,
  onQuit,
  glyphMode = getGlyphMode(),
}: OnboardingSignInProps) => {
  const theme = useTheme()
  const status = useEngineStore((state) => state.status)
  const signIn = useEngineStore((state) => state.signIn)
  const problem = useEngineStore((state) => state.signInProblem)
  const view = signInView(status, signIn, Date.now(), problem)
  const signingIn = view?.kind === 'code' || view?.kind === 'starting'

  useKeyboard(
    useCallback(
      (key: KeyEvent) => {
        const action = onboardingKeyAction(key, signingIn)
        if (action === 'none') return
        key.preventDefault?.()
        if (action === 'quit') onQuit()
        else if (action === 'later') onLater()
        else onSignIn()
      },
      [onLater, onQuit, onSignIn, signingIn],
    ),
  )

  const marker = glyphMode === 'console' ? '>' : '›'
  const textWidth = Math.max(20, width - 4)

  return (
    <box style={{ flexDirection: 'column', paddingLeft: 2, paddingRight: 2, paddingTop: 1 }}>
      <text style={{ fg: theme.foreground, attributes: TextAttributes.BOLD }}>Welcome to Neuron</text>
      <text style={{ wrapMode: 'word', paddingTop: 1, fg: theme.foreground, width: textWidth }}>
        Sign in with your Zero-X account to use Neuron's models. Your browser opens the sign-in page; Neuron
        keeps the login on this computer.
      </text>
      {signingIn ? (
        <box style={{ paddingTop: 1 }}>
          <SignInPanel width={textWidth} />
        </box>
      ) : (
        <>
          {view?.kind === 'unreachable' ? (
            <box style={{ flexDirection: 'column', paddingTop: 1 }}>
              <text style={{ wrapMode: 'word', fg: theme.error, width: textWidth }}>
                Neuron could not start a sign-in with Zero-X.
              </text>
              <text style={{ wrapMode: 'word', fg: theme.muted, width: textWidth }}>{view.reason}</text>
              <text style={{ wrapMode: 'word', fg: theme.foreground, width: textWidth }}>
                Check your connection, then press Enter to try again.
              </text>
            </box>
          ) : null}
          <text style={{ paddingTop: 1, fg: theme.primary }}>
            <span>{`${marker} `}</span>
            <span attributes={TextAttributes.BOLD}>Sign in with Zero-X</span>
          </text>
          <text style={{ paddingTop: 1, fg: theme.muted, wrapMode: 'word', width: textWidth }}>
            Enter to sign in · Esc for later (/login signs in from the chat) · Ctrl+C to quit
          </text>
        </>
      )}
    </box>
  )
}
