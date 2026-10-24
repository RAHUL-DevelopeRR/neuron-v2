import { create } from 'zustand'
import { immer } from 'zustand/middleware/immer'

import type { EngineConnection } from '../engine/neuron-engine-client'
import type { QuestionTarget } from '../engine/questions'
import type {
  ModelsEvent,
  PermissionRequestedEvent,
  ReportEvent,
  SignInCodeEvent,
  StatusEvent,
  TrustPreviewEvent,
  UsageEvent,
} from '../engine/protocol'

/**
 * What the engine has told the UI outside a turn's transcript: whether it is
 * running, its footer status, the model list, token usage, the permission
 * requests waiting for an answer, an AskUserQuestion it is waiting on, what
 * trusting the folder would turn on, and the report a command showed.
 */
export interface EngineStoreState {
  connection: EngineConnection
  /** Neuron version from `ready`. */
  version: string | null
  /** What the engine adds to protocol 1 (`ready.features`, `FEATURE_*`). */
  features: string[]
  /** Why the engine is not running, after it failed to start or stopped. */
  error: string | null
  /**
   * That reason is already a line in the chat (or on the turn that was
   * running), so the status bar does not repeat it.
   */
  errorInChat: boolean
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
   * Where that question is drawn: its block in the turn's message, which the
   * answer is written into. `null` when it is a chat line instead.
   */
  pendingQuestionTarget: QuestionTarget | null
  /**
   * The device code of a `/login` in progress, until it ends. `approved` once
   * the engine said the sign-in worked and is opening the session.
   */
  signIn: (SignInCodeEvent & { receivedAt: number; approved: boolean }) | null
  /**
   * Why the last `/login` could not start: Zero-X was unreachable, does not
   * offer device sign-in, or answered with a server error (the engine's
   * message). Cleared when a sign-in code arrives or the session is signed in.
   */
  signInProblem: string | null
  /** What trusting the folder would turn on, for the trust screen (`trust_preview`). */
  trustPreview: TrustPreviewEvent | null
  /** The report panel's content (`/usage`, `/init`, `/memory`, `/doctor`), while it is open. */
  report: ReportEvent | null

  /** `errorInChat`: the chat already shows `error` (see {@link EngineStoreState.errorInChat}). */
  setConnection: (
    connection: EngineConnection,
    error?: string | null,
    errorInChat?: boolean,
  ) => void
  setReady: (version: string, features?: string[]) => void
  setStatus: (status: StatusEvent) => void
  setModels: (models: ModelsEvent) => void
  setUsage: (usage: UsageEvent) => void
  enqueuePermission: (request: PermissionRequestedEvent) => void
  /** Drop a request once answered (or when the turn ends without one). */
  resolvePermission: (id: number) => void
  clearPermissions: () => void
  /** Also forgets where the previous question was drawn. */
  setPendingQuestion: (question: string | null) => void
  setPendingQuestionTarget: (target: QuestionTarget | null) => void
  setSignIn: (code: SignInCodeEvent | null, receivedAt?: number) => void
  setSignInProblem: (problem: string | null) => void
  markSignInApproved: () => void
  setTrustPreview: (preview: TrustPreviewEvent | null) => void
  setReport: (report: ReportEvent | null) => void
  reset: () => void
}

/** The engine sent `feature` in `ready.features`. */
export function hasEngineFeature(state: Pick<EngineStoreState, 'features'>, feature: string): boolean {
  return state.features.includes(feature)
}

/**
 * Prompts need a Zero-X sign-in first. Engines that send `sign_in_required`
 * say so. Older ones are read from their signed-out stage: `signed_in: false`
 * with no session id (a provider with its own API key is also not signed in
 * to Zero-X, but it has a session).
 */
export function needsSignIn(status: StatusEvent | null): boolean {
  if (status === null) return false
  if (status.sign_in_required !== undefined) return status.sign_in_required
  return status.signed_in === false && status.session_id === ''
}

const initial = {
  connection: 'not_started' as EngineConnection,
  version: null,
  features: [] as string[],
  error: null,
  errorInChat: false,
  status: null,
  models: null,
  usage: null,
  permissionQueue: [] as PermissionRequestedEvent[],
  pendingQuestion: null,
  pendingQuestionTarget: null,
  signIn: null,
  signInProblem: null,
  trustPreview: null as TrustPreviewEvent | null,
  report: null as ReportEvent | null,
}

export const useEngineStore = create<EngineStoreState>()(
  immer((set) => ({
    ...initial,
    setConnection: (connection, error, errorInChat = false) =>
      set((state) => {
        state.connection = connection
        if (error !== undefined) {
          state.error = error
          state.errorInChat = error !== null && errorInChat
        }
        if (connection === 'exited') {
          state.permissionQueue = []
          state.pendingQuestion = null
          state.pendingQuestionTarget = null
          state.signIn = null
        }
      }),
    setReady: (version, features = []) =>
      set((state) => {
        state.connection = 'ready'
        state.version = version
        state.features = features
        state.error = null
        state.errorInChat = false
      }),
    setStatus: (status) =>
      set((state) => {
        state.status = status
        if (!needsSignIn(status)) state.signInProblem = null
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
        state.pendingQuestionTarget = null
      }),
    setPendingQuestionTarget: (target) =>
      set((state) => {
        state.pendingQuestionTarget = target
      }),
    setSignIn: (code, receivedAt = Date.now()) =>
      set((state) => {
        state.signIn = code ? { ...code, receivedAt, approved: false } : null
        if (code) state.signInProblem = null
      }),
    setSignInProblem: (problem) =>
      set((state) => {
        state.signInProblem = problem
      }),
    markSignInApproved: () =>
      set((state) => {
        if (state.signIn) state.signIn.approved = true
      }),
    setTrustPreview: (preview) =>
      set((state) => {
        state.trustPreview = preview
      }),
    setReport: (report) =>
      set((state) => {
        state.report = report
      }),
    reset: () => set(() => ({ ...initial })),
  })),
)
