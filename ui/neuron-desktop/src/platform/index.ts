import { hostPlatform } from './host'
import type { Platform } from './types'

export type { DirEntry, DirListing, FileContent, Platform, TerminalSession } from './types'

/** Inside Tauri, its bridge; in a browser, the host named by the page's settings. */
export async function detectPlatform(): Promise<Platform> {
  if ('__TAURI_INTERNALS__' in window) {
    const { tauriPlatform } = await import('./tauri')
    return tauriPlatform()
  }
  const url = import.meta.env.VITE_NEURON_HOST_URL as string | undefined
  const token = import.meta.env.VITE_NEURON_HOST_TOKEN as string | undefined
  if (!url || !token) {
    throw new Error(
      'No Neuron host configured. Start one with `bun run host --mock --token-file <file>` in ui/neuron-client, ' +
        'then `NEURON_HOST_URL=<url> NEURON_HOST_TOKEN_FILE=<file> bun run dev` here.',
    )
  }
  return hostPlatform({ url, token })
}
