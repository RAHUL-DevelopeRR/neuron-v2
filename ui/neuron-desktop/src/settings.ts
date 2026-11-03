import { useCallback, useEffect, useState } from 'react'

/** What the user sets in Settings. Kept in this browser profile (the app's own in Tauri). */
export interface Settings {
  theme: 'dark' | 'light' | 'system'
  /** Enter sends (Shift+Enter breaks the line); otherwise Ctrl+Enter sends. */
  enterSends: boolean
  /** Show the model's reasoning when the provider streams it. */
  showReasoning: boolean
  /** Tool calls start folded, except edits and failures. */
  foldTools: boolean
}

export const DEFAULT_SETTINGS: Settings = {
  theme: 'dark',
  enterSends: true,
  showReasoning: true,
  foldTools: true,
}

const KEY = 'neuron.desktop.settings'

function load(): Settings {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? '{}') as Partial<Settings>
    return { ...DEFAULT_SETTINGS, ...saved }
  } catch {
    return DEFAULT_SETTINGS
  }
}

export function useSettings(): [Settings, (change: Partial<Settings>) => void] {
  const [settings, setSettings] = useState<Settings>(load)
  const update = useCallback((change: Partial<Settings>) => {
    setSettings((current) => {
      const next = { ...current, ...change }
      try {
        localStorage.setItem(KEY, JSON.stringify(next))
      } catch {
        // Private windows and blocked storage keep the settings for this session only.
      }
      return next
    })
  }, [])
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
  return [settings, update]
}
