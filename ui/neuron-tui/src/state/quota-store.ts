/**
 * The usage limit that stopped the last turn, and auto-continue's state: the
 * quota error card (components/quota-error-card.tsx) draws it and
 * engine/quota-scheduler.ts acts on it.
 */
import { create } from 'zustand'
import { immer } from 'zustand/middleware/immer'

import { AUTO_CONTINUE_GRACE_MS, AUTO_CONTINUE_MAX_ATTEMPTS } from '../utils/quota-schedule'
import { loadSettings, saveSettings } from '../utils/settings'

/** A turn a usage limit stopped. */
export interface QuotaBlock {
  /** The assistant message whose error card this is. */
  messageId: string
  /** The gateway's code: `insufficient_quota` or `rate_limited`. */
  code: string
  /** When the limit frees up (ms since the epoch), when it said. */
  resumeAt: number | null
  /** When the error arrived. */
  at: number
}

interface QuotaState {
  block: QuotaBlock | null
  /** The setting: continue by itself once the limit resets. */
  autoContinue: boolean
  /** When auto-continue sends "continue" (ms); `null` while not armed. */
  continueAt: number | null
  /** Auto-continues sent since a turn last ended well. */
  attempts: number
  /** Why auto-continue stopped, for the card ("cancelled", "gave up"). */
  stoppedReason: string | null

  /** A turn stopped on a resetting limit: remember it, and arm if the setting is on. */
  limitHit: (block: QuotaBlock) => void
  /** A turn ended well, or a new chat began: the card has nothing to wait for. */
  clear: () => void
  /** Turns the setting on or off (saved), arming or disarming the current block. */
  setAutoContinue: (on: boolean, persist?: boolean) => void
  /** Esc or a click: no auto-continue for this block. */
  cancel: () => void
  /** The scheduler sent "continue". */
  fired: () => void
}

function armFor(block: QuotaBlock | null): number | null {
  return block?.resumeAt != null ? block.resumeAt + AUTO_CONTINUE_GRACE_MS : null
}

/** Reads the saved auto-continue setting into the store (once, at start). */
export function loadAutoContinueSetting(): void {
  let on = false
  try {
    on = loadSettings().autoContinueAfterReset === true
  } catch {
    // An unreadable settings file leaves it off.
  }
  useQuotaStore.getState().setAutoContinue(on, false)
}

export const useQuotaStore = create<QuotaState>()(
  immer((set) => ({
    block: null,
    autoContinue: false,
    continueAt: null,
    attempts: 0,
    stoppedReason: null,

    limitHit: (block) =>
      set((state) => {
        state.block = block
        state.stoppedReason = null
        if (!state.autoContinue) {
          state.continueAt = null
        } else if (state.attempts >= AUTO_CONTINUE_MAX_ATTEMPTS) {
          state.continueAt = null
          state.stoppedReason = `stopped after ${state.attempts} tries`
        } else {
          state.continueAt = armFor(block)
        }
      }),
    clear: () =>
      set((state) => {
        state.block = null
        state.continueAt = null
        state.stoppedReason = null
        state.attempts = 0
      }),
    setAutoContinue: (on, persist = true) => {
      if (persist) saveSettings({ autoContinueAfterReset: on })
      set((state) => {
        state.autoContinue = on
        state.stoppedReason = null
        state.continueAt = on ? armFor(state.block) : null
      })
    },
    cancel: () =>
      set((state) => {
        if (state.continueAt === null) return
        state.continueAt = null
        state.stoppedReason = 'cancelled'
      }),
    fired: () =>
      set((state) => {
        state.continueAt = null
        state.attempts += 1
      }),
  })),
)
