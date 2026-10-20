import { create } from 'zustand'
import { immer } from 'zustand/middleware/immer'

import type { EngineConnection } from '../engine/neuron-engine-client'
import type {
  ModelsEvent,
  PermissionRequestedEvent,
  SignInCodeEvent,
  StatusEvent,
  UsageEvent,
} from '../engine/protocol'

/**
 * What the engine has told the UI outside a turn's transcript: whether it is
 * running, its footer status, the model list, token usage, the permission
 * requests waiting for an answer, and an AskUserQuestion it is waiting on.
 */
export interface EngineStoreState {
  connection: EngineConnection
  /** Neuron version from `ready`. */
  version: string | null
  /** Why the engine is not running, after it failed to start or stopped. */
  error: string | null
  status: StatusEvent | null
  models: ModelsEvent | null
  usage: Omit<UsageEvent, 'type'> | null
  /** Oldest first; the dialog shows the head. */
  permissionQueue: PermissionRequestedEvent[]
  /**
   * The engine asked a question (a notice starting with "Question:") and
   * takes the next submit as its answer, even while the turn runs.
   */
  pendingQuestion: string | null
  /**
   * The device code of a `/login` in progress, until it ends. `approved` once
   * the engine said the sign-in worked and is opening the session.
   */
  signIn: (SignInCodeEvent & { receivedAt: number; approved: boolean }) | null

  setConnection: (connection: EngineConnection, error?: string | null) => void
  setReady: (version: string) => void
  setStatus: (status: StatusEvent) => void
  setModels: (models: ModelsEvent) => void
  setUsage: (usage: UsageEvent) => void
  enqueuePermission: (request: PermissionRequestedEvent) => void
  /** Drop a request once answered (or when the turn ends without one). */
  resolvePermission: (id: number) => void
  clearPermissions: () => void
  setPendingQuestion: (question: string | null) => void
  setSignIn: (code: SignInCodeEvent | null, receivedAt?: number) => void
  markSignInApproved: () => void
  reset: () => void
}

/**
 * The engine runs but has no session until a Zero-X sign-in: its signed-out
 * stage reports `signed_in: false` with no session id. (A provider with its
 * own API key is also not signed in to Zero-X, but it has a session.)
 */
export function needsSignIn(status: StatusEvent | null): boolean {
  return status !== null && status.signed_in === false && status.session_id === ''
}

const initial = {
  connection: 'not_started' as EngineConnection,
  version: null,
  error: null,
  status: null,
  models: null,
  usage: null,
  permissionQueue: [] as PermissionRequestedEvent[],
  pendingQuestion: null,
  signIn: null,
}

export const useEngineStore = create<EngineStoreState>()(
  immer((set) => ({
    ...initial,
    setConnection: (connection, error) =>
      set((state) => {
        state.connection = connection
        if (error !== undefined) state.error = error
        if (connection === 'exited') {
          state.permissionQueue = []
          state.pendingQuestion = null
          state.signIn = null
        }
      }),
    setReady: (version) =>
      set((state) => {
        state.connection = 'ready'
        state.version = version
        state.error = null
      }),
    setStatus: (status) =>
      set((state) => {
        state.status = status
      }),
    setModels: (models) =>
      set((state) => {
        state.models = models
      }),
    setUsage: ({ turn, cumulative, estimated }) =>
      set((state) => {
        state.usage = { turn, cumulative, estimated }
      }),
    enqueuePermission: (request) =>
      set((state) => {
        if (!state.permissionQueue.some((queued) => queued.id === request.id)) {
          state.permissionQueue.push(request)
        }
      }),
    resolvePermission: (id) =>
      set((state) => {
        state.permissionQueue = state.permissionQueue.filter(
          (queued) => queued.id !== id,
        )
      }),
    clearPermissions: () =>
      set((state) => {
        state.permissionQueue = []
      }),
    setPendingQuestion: (question) =>
      set((state) => {
        state.pendingQuestion = question
      }),
    setSignIn: (code, receivedAt = Date.now()) =>
      set((state) => {
        state.signIn = code ? { ...code, receivedAt, approved: false } : null
      }),
    markSignInApproved: () =>
      set((state) => {
        if (state.signIn) state.signIn.approved = true
      }),
    reset: () => set(() => ({ ...initial })),
  })),
)
