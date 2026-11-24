/**
 * Onboarding's sign-in (T2, step 2): the first screen when the UI starts with
 * no usable Zero-X login, so a fresh install (or one after `neuron logout`)
 * signs in before the folder's trust (T1) and the chat. Enter starts the
 * engine's device sign-in, whose link and code the sign-in panel shows; Esc
 * leaves it for later (the chat's panel then says how to sign in); Ctrl-C
 * quits. With the mouse, a click on "Sign in with Zero-X" or on a key in the
 * hint does the same.
 */

import { TextAttributes, type KeyEvent } from '@opentui/core'
import { useKeyboard } from '@opentui/react'
import React, { useCallback, useState } from 'react'

import { Button } from './button'
import { SignInPanel, signInView } from './sign-in-panel'
import { useTheme } from '../hooks/use-theme'
import { useEngineStore } from '../state/engine-store'
import { getGlyphMode, type GlyphMode } from '../utils/glyphs'

interface OnboardingHint {
  action: 'sign_in' | 'later' | 'quit'
  label: string
}

/** The hint under the choice: each part is also what a click on it does. */
export const ONBOARDING_HINTS: readonly OnboardingHint[] = [
  { action: 'sign_in', label: 'Enter to sign in' },
  { action: 'later', label: 'Esc for later (/login signs in from the chat)' },
  { action: 'quit', label: 'Ctrl+C to quit' },
]

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

  // The mouse: the sign-in line and each key in the hint are clickable, and
  // the one under the pointer lights up.
  const [hovered, setHovered] = useState<string | null>(null)
  const hoverProps = (id: string) => ({
    onMouseOver: () => setHovered(id),
    onMouseOut: () => setHovered((now) => (now === id ? null : now)),
  })
  const actions: Record<OnboardingHint['action'], () => void> = {
    sign_in: onSignIn,
    later: onLater,
    quit: onQuit,
  }

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
          <Button
            onClick={onSignIn}
            {...hoverProps('sign_in')}
            style={{
              paddingTop: 1,
              flexDirection: 'row',
              alignSelf: 'flex-start',
            }}
          >
            <text
              style={{
                fg: theme.primary,
                bg: hovered === 'sign_in' ? theme.surfaceHover : undefined,
              }}
            >
              <span>{`${marker} `}</span>
              <span attributes={TextAttributes.BOLD}>Sign in with Zero-X</span>
            </text>
          </Button>
          {/* The keys, each one a click target too. */}
          <box style={{ paddingTop: 1, flexDirection: 'row', flexWrap: 'wrap', width: textWidth }}>
            {ONBOARDING_HINTS.map((hint, index) => (
              <box key={hint.action} style={{ flexDirection: 'row', flexShrink: 0 }}>
                <Button onClick={actions[hint.action]} {...hoverProps(hint.action)}>
                  <text
                    style={{
                      fg: hovered === hint.action ? theme.foreground : theme.muted,
                      attributes: hovered === hint.action ? TextAttributes.UNDERLINE : undefined,
                    }}
                  >
                    {hint.label}
                  </text>
                </Button>
                {/* The dot ends its line, so a wrapped line starts with a key. */}
                {index < ONBOARDING_HINTS.length - 1 ? (
                  <text style={{ fg: theme.muted }}>{' · '}</text>
                ) : null}
              </box>
            ))}
          </box>
        </>
      )}
    </box>
  )
}
