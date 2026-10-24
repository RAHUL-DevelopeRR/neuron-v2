import { createContext, useContext } from 'react'

import { brand, dark, light, logoLetters, type Palette } from '@neuron/client'

export { brand, logoLetters, type Palette }

export type ThemeName = 'light' | 'dark'

/** The NeuronCLI palettes (ui/neuron-client/src/theme.ts) in the shapes React Native styles take. */
export const palettes: Record<ThemeName, Palette> = { light, dark }

/** The dark palette, for the screens that have one look (the LAN dev screens). */
export const colors = dark

export const mono = 'monospace'

export interface Theme {
  name: ThemeName
  colors: Palette
}

export const ThemeContext = createContext<Theme>({ name: 'dark', colors: dark })

export const useTheme = (): Theme => useContext(ThemeContext)
