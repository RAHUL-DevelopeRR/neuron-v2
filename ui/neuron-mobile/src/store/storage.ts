/**
 * Where the app keeps what it must remember. Secrets (the Zero-X session, the
 * pair keys) go to the platform's secure store: the iOS Keychain, Android's
 * Keystore-backed storage. The web build has no secure store; it uses
 * localStorage, which is fine for development and is why the web build is
 * not a product.
 */

export interface KeyValueStore {
  get(key: string): Promise<string | null>
  set(key: string, value: string): Promise<void>
  delete(key: string): Promise<void>
  /** Whether values are protected at rest (false on the web). */
  readonly secure: boolean
}

export function memoryStore(): KeyValueStore {
  const values = new Map<string, string>()
  return {
    secure: false,
    get: async (key) => values.get(key) ?? null,
    set: async (key, value) => void values.set(key, value),
    delete: async (key) => void values.delete(key),
  }
}

function webStore(): KeyValueStore {
  const storage = (globalThis as { localStorage?: Storage }).localStorage
  if (!storage) return memoryStore()
  return {
    secure: false,
    get: async (key) => storage.getItem(key),
    set: async (key, value) => storage.setItem(key, value),
    delete: async (key) => storage.removeItem(key),
  }
}

/** The platform's store: expo-secure-store on iOS and Android, localStorage on the web. */
export function platformStore(os: string): KeyValueStore {
  if (os === 'web') return webStore()
  // Required lazily so tests and the web bundle never load the native module.
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const SecureStore = require('expo-secure-store') as typeof import('expo-secure-store')
  const options = { keychainAccessible: SecureStore.AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY }
  return {
    secure: true,
    get: (key) => SecureStore.getItemAsync(key, options),
    set: (key, value) => SecureStore.setItemAsync(key, value, options),
    delete: (key) => SecureStore.deleteItemAsync(key, options),
  }
}

/** Keys the app uses. Secure-store keys allow `[A-Za-z0-9._-]` only. */
export const KEYS = {
  device: 'neuron.device',
  session: 'neuron.session',
  settings: 'neuron.settings',
  computers: 'neuron.computers',
  computer: (hostId: string) => `neuron.computer.${hostId}`,
  viewed: 'neuron.viewed',
} as const
