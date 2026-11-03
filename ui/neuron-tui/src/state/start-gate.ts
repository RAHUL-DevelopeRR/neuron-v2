/**
 * What the UI draws in place of the chat while it starts:
 *
 * - **starting**: the first engine has not said yet whether it needs a
 *   sign-in or whether the folder is trusted. Nothing runs before it has (a
 *   prompt given on the command line waits too).
 * - **sign_in**: onboarding (T2). The UI started with no usable Zero-X login,
 *   so the first screen signs in. After `neuron logout` it is back.
 * - **trust**: the folder is not trusted (T1). Its settings, hooks and MCP
 *   servers stay off until the user allows Neuron here; also shown while the
 *   engine restarts with them.
 * - **chat**: everything else, including an engine that failed to start (the
 *   chat says why).
 */

import { create } from 'zustand'

import { FEATURE_TRUST_WORKSPACE } from '../engine/protocol'
import { needsSignIn } from './engine-store'

import type { EngineConnection } from '../engine/neuron-engine-client'
import type { StatusEvent } from '../engine/protocol'

export type StartScreen = 'starting' | 'sign_in' | 'trust' | 'chat'

/** Where the trust screen is: asking, saving the answer, or restarting the engine with it. */
export type TrustStep =
  | { kind: 'asking' }
  | { kind: 'saving' }
  | { kind: 'restarting' }
  | { kind: 'failed'; error: string }

export interface StartGateInput {
  connection: EngineConnection
  status: StatusEvent | null
  features: string[]
  /** The first engine has sent its first status. */
  firstStatusSeen: boolean
  /** Onboarding's sign-in step is on (the UI started signed out) and not skipped. */
  onboardingSignIn: boolean
  trustStep: TrustStep
}

export function startScreen(input: StartGateInput): StartScreen {
  // The engine restarts with the folder's settings: the trust screen says so.
  if (input.trustStep.kind === 'restarting' && input.connection !== 'exited') return 'trust'
  if (!input.firstStatusSeen) return input.connection === 'exited' ? 'chat' : 'starting'
  if (input.connection !== 'ready' || input.status === null) return 'chat'
  if (input.onboardingSignIn && needsSignIn(input.status)) return 'sign_in'
  if (
    input.features.includes(FEATURE_TRUST_WORKSPACE) &&
    !input.status.workspace_trusted
  ) {
    return 'trust'
  }
  return 'chat'
}

interface StartGateState {
  firstStatusSeen: boolean
  onboardingSignIn: boolean
  trustStep: TrustStep
  /** An engine status: the first one decides whether onboarding signs in first. */
  noteStatus: (status: StatusEvent) => void
  /** "Not now" on the onboarding sign-in: the chat's sign-in panel takes over. */
  skipOnboardingSignIn: () => void
  setTrustStep: (step: TrustStep) => void
  reset: () => void
}

const initial = {
  firstStatusSeen: false,
  onboardingSignIn: false,
  trustStep: { kind: 'asking' } as TrustStep,
}

export const useStartGate = create<StartGateState>()((set, get) => ({
  ...initial,
  noteStatus: (status) => {
    if (!get().firstStatusSeen) {
      set({ firstStatusSeen: true, onboardingSignIn: needsSignIn(status) })
    }
  },
  skipOnboardingSignIn: () => set({ onboardingSignIn: false }),
  setTrustStep: (trustStep) => set({ trustStep }),
  reset: () => set({ ...initial }),
}))
