/**
 * The update check, over an injected updater so it can be tested.
 *
 * A build without an updater public key cannot verify an update, so it never
 * asks the updater: its users get the releases page instead (`manual`).
 */

import type { UpdateInfo } from './types'

export interface UpdateChecker {
  /** True when updates are fetched by hand from the releases page. */
  manual: boolean
  /** A newer release, if there is one the app can install; `null` otherwise or on failure. */
  check(): Promise<UpdateInfo | null>
}

export function createUpdateChecker(options: {
  signed: boolean
  check: () => Promise<{ version: string; body?: string | null } | null>
}): UpdateChecker {
  if (!options.signed) return { manual: true, check: async () => null }
  return {
    manual: false,
    async check() {
      try {
        const update = await options.check()
        return update ? { version: update.version, notes: update.body ?? null } : null
      } catch {
        // No manifest yet or offline: no notice.
        return null
      }
    },
  }
}
