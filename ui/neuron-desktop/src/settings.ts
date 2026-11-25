import { useCallback, useEffect, useState } from 'react'

import type { GroupBy, SeenState } from './sessions'
import type { KeyValueStore } from './platform'

/** What the user sets in Settings. Kept in the app's store (the browser's storage in the browser build). */
export interface Settings {
  theme: 'dark' | 'light' | 'system'
  /** Enter sends (Shift+Enter breaks the line); otherwise Ctrl+Enter sends. */
  enterSends: boolean
  /** Show the model's reasoning when the provider streams it. */
  showReasoning: boolean
  /** Tool calls start folded, except edits and failures. */
  foldTools: boolean
  /** The model new sessions start on; empty for the engine's own default. */
  defaultModel: string
  /** The permission mode new sessions start in. */
  defaultPermissionMode: 'read-only' | 'workspace-write' | 'danger-full-access'
  /** Start each folder in the mode it was last left in. */
  rememberModePerFolder: boolean
  /** The last mode per folder path, for `rememberModePerFolder`. */
  modeByFolder: Record<string, string>
  /** The sidebar: full, an icon rail, or a rail only when the window is narrow. */
  sidebar: 'auto' | 'full' | 'rail'
  groupBy: GroupBy
  /** The welcome steps were done (or skipped). */
  firstRunDone: boolean
}

export const DEFAULT_SETTINGS: Settings = {
  theme: 'dark',
  enterSends: true,
  showReasoning: true,
  foldTools: true,
  defaultModel: '',
  defaultPermissionMode: 'workspace-write',
  rememberModePerFolder: true,
  modeByFolder: {},
  sidebar: 'auto',
  groupBy: 'project',
  firstRunDone: false,
}

const SETTINGS_KEY = 'settings'
const SEEN_KEY = 'seen'
/** A copy for the first paint, before the store answers (so the theme does not flash). */
const CACHE_KEY = 'neuron.desktop.settings'

function cached(): Settings {
  try {
    return { ...DEFAULT_SETTINGS, ...(JSON.parse(localStorage.getItem(CACHE_KEY) ?? '{}') as Partial<Settings>) }
  } catch {
    return DEFAULT_SETTINGS
  }
}

function cache(settings: Settings): void {
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify(settings))
  } catch {
    // Blocked storage: the store still has them.
  }
}

/** The settings, with whether the store has answered yet, and a setter that saves. */
export function useSettings(store: KeyValueStore): [Settings, (change: Partial<Settings>) => void, boolean] {
  const [settings, setSettings] = useState<Settings>(cached)
  const [loaded, setLoaded] = useState(false)
  useEffect(() => {
    let cancelled = false
    store.get<Partial<Settings>>(SETTINGS_KEY).then(
      (saved) => {
        if (cancelled) return
        if (saved) {
          const next = { ...DEFAULT_SETTINGS, ...saved }
          cache(next)
          setSettings(next)
        }
        setLoaded(true)
      },
      () => !cancelled && setLoaded(true),
    )
    return () => {
      cancelled = true
    }
  }, [store])
  const update = useCallback(
    (change: Partial<Settings>) => {
      setSettings((current) => {
        const next = { ...current, ...change }
        cache(next)
        void store.set(SETTINGS_KEY, next).catch(() => {})
        return next
      })
    },
    [store],
  )
  // The theme is a class on the page, so plain CSS follows it.
  useEffect(() => {
    const root = document.documentElement
    const media = window.matchMedia?.('(prefers-color-scheme: light)')
    const apply = () => {
      const light = settings.theme === 'light' || (settings.theme === 'system' && media?.matches === true)
      root.dataset.theme = light ? 'light' : 'dark'
    }
    apply()
    media?.addEventListener?.('change', apply)
    return () => media?.removeEventListener?.('change', apply)
  }, [settings.theme])
  return [settings, update, loaded]
}

/** When each session was last looked at, and which failed, kept between launches. */
export function useSeen(store: KeyValueStore): [SeenState, (next: (current: SeenState) => SeenState) => void] {
  const [seen, setSeen] = useState<SeenState>({ seen: {}, failed: {} })
  useEffect(() => {
    let cancelled = false
    store.get<SeenState>(SEEN_KEY).then(
      (saved) => {
        // What this launch already noted wins over what was saved.
        if (!cancelled && saved) setSeen((current) => ({ seen: { ...saved.seen, ...current.seen }, failed: { ...saved.failed, ...current.failed } }))
      },
      () => {},
    )
    return () => {
      cancelled = true
    }
  }, [store])
  const update = useCallback(
    (next: (current: SeenState) => SeenState) => {
      setSeen((current) => {
        const value = next(current)
        if (value !== current) void store.set(SEEN_KEY, value).catch(() => {})
        return value
      })
    },
    [store],
  )
  return [seen, update]
}
