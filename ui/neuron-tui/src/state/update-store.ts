/**
 * What the update check found (utils/update-check.ts): the prompt status bar
 * shows "Update available! Run: <command>" while `status` is `available`, and
 * `/update` reports the whole result.
 *
 * {@link startUpdateCheck} runs once, after the UI is drawn (index.tsx);
 * {@link refreshUpdateCheck} reads the manifest again for `/update`, after any
 * check still running.
 */

import { create } from 'zustand'

import { loadPackageVersion } from '../cli-args'
import {
  DEFAULT_MANIFEST_URL,
  processUpdateCheckOptions,
  runUpdateCheck,
  type UpdateCheckOptions,
  type UpdateCheckResult,
} from '../utils/update-check'

export type UpdateStatus = 'idle' | 'checking' | UpdateCheckResult['status']

export interface UpdateState {
  status: UpdateStatus
  /** The installed version. */
  current: string | null
  /** The newest release, when the manifest names one. */
  latest: string | null
  /** The manifest's minimum supported version. */
  minimum: string | null
  notesUrl: string | null
  /** The command that installs the latest release, while one is available. */
  command: string | null
  /** The installed version is older than the minimum. */
  required: boolean
  /** Why the check is off or failed. */
  reason: string | null
  manifestUrl: string | null
  /** The last result, for `/update`. */
  result: UpdateCheckResult | null
  setChecking: () => void
  setResult: (result: UpdateCheckResult) => void
  reset: () => void
}

const EMPTY = {
  status: 'idle' as UpdateStatus,
  current: null,
  latest: null,
  minimum: null,
  notesUrl: null,
  command: null,
  required: false,
  reason: null,
  manifestUrl: null,
  result: null,
}

export const useUpdateStore = create<UpdateState>()((set) => ({
  ...EMPTY,
  // A result already shown stays while the manifest is read again.
  setChecking: () => set((state) => (state.result ? {} : { status: 'checking' })),
  setResult: (result) =>
    set({
      status: result.status,
      current: result.current,
      latest: result.status === 'available' || result.status === 'up-to-date' ? result.latest : null,
      minimum: result.status === 'available' ? result.minimum : null,
      notesUrl: result.status === 'available' || result.status === 'up-to-date' ? result.notesUrl : null,
      command: result.status === 'available' ? result.command : null,
      required: result.status === 'available' && result.required,
      reason: result.status === 'disabled' || result.status === 'failed' ? result.reason : null,
      manifestUrl: result.manifestUrl,
      result,
    }),
  reset: () => set({ ...EMPTY }),
}))

/** The notice for the status bar, while an update is available. */
export function updateNotice(state: Pick<UpdateState, 'status' | 'command' | 'required'>): {
  command: string
  required: boolean
} | null {
  return state.status === 'available' && state.command
    ? { command: state.command, required: state.required }
    : null
}

let inFlight: Promise<UpdateCheckResult> | null = null
let started = false

function check(overrides: Partial<UpdateCheckOptions>): Promise<UpdateCheckResult> {
  const run = (async () => {
    useUpdateStore.getState().setChecking()
    let result: UpdateCheckResult
    try {
      const options = { ...processUpdateCheckOptions(loadPackageVersion()), ...overrides }
      result = await runUpdateCheck(options)
    } catch {
      result = {
        status: 'failed',
        current: overrides.version ?? 'unknown',
        reason: 'the check could not start',
        manifestUrl: DEFAULT_MANIFEST_URL,
      }
    }
    // A failed read again (offline, say) does not hide an update already found.
    const previous = useUpdateStore.getState().result
    if (!(result.status === 'failed' && previous?.status === 'available')) {
      useUpdateStore.getState().setResult(result)
    }
    return result
  })()
  inFlight = run
  void run.finally(() => {
    if (inFlight === run) inFlight = null
  })
  return run
}

/** The launch's check, once; its result lands in the store. Never throws. */
export function startUpdateCheck(overrides: Partial<UpdateCheckOptions> = {}): void {
  if (started) return
  started = true
  check(overrides).catch(() => {})
}

/** `/update`: the manifest read again, after a check still running. Never throws. */
export async function refreshUpdateCheck(
  overrides: Partial<UpdateCheckOptions> = {},
): Promise<UpdateCheckResult> {
  started = true
  if (inFlight) await inFlight.catch(() => {})
  return check({ force: true, ...overrides })
}

/** Tests: forget the launch's check. */
export function resetUpdateCheckForTesting(): void {
  inFlight = null
  started = false
  useUpdateStore.getState().reset()
}
