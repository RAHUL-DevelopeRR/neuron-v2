import { createContext, useContext, useEffect, useMemo, useSyncExternalStore, type ReactNode } from 'react'
import { AppState as NativeAppState, Platform as NativePlatform, useColorScheme } from 'react-native'
import Constants from 'expo-constants'

import { expoPushToken } from '../push/push'
import { ThemeContext, palettes, type Theme } from '../theme'
import { AppStore, type AppState } from './app'
import { platformStore } from './storage'

const StoreContext = createContext<AppStore | null>(null)

function deviceName(): string {
  if (NativePlatform.OS === 'web') return 'Web browser'
  try {
    const Device = require('expo-device') as typeof import('expo-device')
    return Device.deviceName || Device.modelName || 'Phone'
  } catch {
    return 'Phone'
  }
}

function makeStore(): AppStore {
  return new AppStore({
    storage: platformStore(NativePlatform.OS),
    os: NativePlatform.OS,
    deviceName: deviceName().slice(0, 64),
    appVersion: Constants.expoConfig?.version ?? '0.0.0',
    devBuild: __DEV__,
    pushToken: expoPushToken,
  })
}

/** Holds the one store, follows the app's foreground and the network, and themes the app. */
export function AppProvider({ children }: { children: ReactNode }) {
  const store = useMemo(makeStore, [])
  useEffect(() => {
    void store.load()
    const subscription = NativeAppState.addEventListener('change', (next) => {
      if (next === 'active') store.wake()
    })
    const scope = globalThis as { addEventListener?: (type: string, listener: () => void) => void; removeEventListener?: (type: string, listener: () => void) => void }
    const online = () => store.wake(true)
    const offline = () => store.wake(false)
    scope.addEventListener?.('online', online)
    scope.addEventListener?.('offline', offline)
    return () => {
      subscription.remove()
      scope.removeEventListener?.('online', online)
      scope.removeEventListener?.('offline', offline)
      store.stop()
    }
  }, [store])
  const setting = useSyncExternalStore(store.subscribe, () => store.getState().settings.theme)
  const system = useColorScheme()
  const name = setting === 'system' ? (system === 'light' ? 'light' : 'dark') : setting
  const theme: Theme = useMemo(() => ({ name, colors: palettes[name] }), [name])
  return (
    <StoreContext.Provider value={store}>
      <ThemeContext.Provider value={theme}>{children}</ThemeContext.Provider>
    </StoreContext.Provider>
  )
}

export function useStore(): AppStore {
  const store = useContext(StoreContext)
  if (!store) throw new Error('useStore outside AppProvider')
  return store
}

export function useApp(): AppState {
  const store = useStore()
  return useSyncExternalStore(store.subscribe, store.getState, store.getState)
}
