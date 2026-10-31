/**
 * Draws what comes before the chat (state/start-gate.ts decides): "Starting
 * Neuron…" until the first engine says how things stand, onboarding's
 * sign-in, the folder trust screen, then the chat itself (`children`).
 */

import React, { useRef } from 'react'
import { useShallow } from 'zustand/react/shallow'

import { OnboardingSignIn } from './onboarding-sign-in'
import { TrustScreen } from './trust-screen'
import { runEngineLogin, trustWorkspace } from '../engine/engine-session'
import { FEATURE_TRUST_PREVIEW } from '../engine/protocol'
import { useTerminalDimensions } from '../hooks/use-terminal-dimensions'
import { useTheme } from '../hooks/use-theme'
import { useEngineStore } from '../state/engine-store'
import { startScreen, useStartGate } from '../state/start-gate'
import { exitCliCleanly } from '../utils/exit-cleanly'
import { logger } from '../utils/logger'

import type { TrustPreviewEvent } from '../engine/protocol'

const quit = () => void exitCliCleanly(0)

const signIn = () => {
  runEngineLogin().catch((error) => logger.warn({ error }, '[onboarding] sign-in did not start'))
}

export const StartGate = ({ children }: { children: React.ReactNode }) => {
  const theme = useTheme()
  const { terminalWidth } = useTerminalDimensions()
  const engine = useEngineStore(
    useShallow((state) => ({
      connection: state.connection,
      status: state.status,
      features: state.features,
      trustPreview: state.trustPreview,
    })),
  )
  const gate = useStartGate(
    useShallow((state) => ({
      firstStatusSeen: state.firstStatusSeen,
      onboardingSignIn: state.onboardingSignIn,
      trustStep: state.trustStep,
      skipOnboardingSignIn: state.skipOnboardingSignIn,
    })),
  )
  // While the engine restarts with the folder's settings, its store starts
  // over: the screen keeps showing the folder it was about.
  const lastCwd = useRef('')
  const lastPreview = useRef<TrustPreviewEvent | null>(null)
  if (engine.status?.cwd) lastCwd.current = engine.status.cwd
  if (engine.trustPreview) lastPreview.current = engine.trustPreview

  const screen = startScreen({ ...engine, ...gate })
  if (screen === 'chat') return <>{children}</>
  if (screen === 'starting') {
    return (
      <box style={{ paddingLeft: 2, paddingTop: 1 }}>
        <text style={{ fg: theme.muted }}>Starting Neuron…</text>
      </box>
    )
  }
  if (screen === 'sign_in') {
    return (
      <OnboardingSignIn
        width={terminalWidth}
        onSignIn={signIn}
        onLater={gate.skipOnboardingSignIn}
        onQuit={quit}
      />
    )
  }
  // A preview of another folder (before a folder change) is not this one's.
  const preview = lastPreview.current?.cwd === lastCwd.current ? lastPreview.current : null
  return (
    <TrustScreen
      cwd={lastCwd.current}
      preview={preview}
      previewPending={preview === null && engine.features.includes(FEATURE_TRUST_PREVIEW)}
      step={gate.trustStep}
      width={terminalWidth}
      onAllow={trustWorkspace}
      onQuit={quit}
    />
  )
}
