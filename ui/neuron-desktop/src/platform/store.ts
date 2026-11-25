import type { KeyValueStore } from './types'

/** `localStorage` as a key-value store, for the browser build and tests. Blocked storage keeps nothing. */
export function browserStore(prefix: string): KeyValueStore {
  return {
    async get<T>(key: string) {
      try {
        const raw = localStorage.getItem(prefix + key)
        return raw === null ? undefined : (JSON.parse(raw) as T)
      } catch {
        return undefined
      }
    },
    async set(key, value) {
      try {
        localStorage.setItem(prefix + key, JSON.stringify(value))
      } catch {
        // Private windows and blocked storage keep the value for this session only.
      }
    },
  }
}
