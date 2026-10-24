/**
 * Runs before anything else (app/_layout.tsx imports it first). Hermes has no
 * `crypto.getRandomValues`; the relay's encryption needs it, so it comes from
 * expo-crypto, which reads the platform's secure random source.
 */
import { getRandomValues } from 'expo-crypto'

const scope = globalThis as { crypto?: { getRandomValues?: unknown } }
if (typeof scope.crypto?.getRandomValues !== 'function') {
  scope.crypto = { ...(scope.crypto ?? {}), getRandomValues: getRandomValues as never }
}
