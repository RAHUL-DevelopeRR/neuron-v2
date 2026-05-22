/**
 * Neuron TUI Theme — Design system for the React/Ink terminal interface.
 *
 * Adapted from the Rust brand.rs color system and the opencode Go TUI palette.
 * Uses AdaptiveColor for dark/light terminal backgrounds.
 */

export type AdaptiveColor = {
  dark: string;
  light: string;
};

export type NeuronTheme = {
  primary: AdaptiveColor;
  secondary: AdaptiveColor;
  accent: AdaptiveColor;
  error: AdaptiveColor;
  warning: AdaptiveColor;
  success: AdaptiveColor;
  info: AdaptiveColor;
  text: AdaptiveColor;
  muted: AdaptiveColor;
  background: AdaptiveColor;
  panel: AdaptiveColor;
  border: AdaptiveColor;
  // Brand-specific colors matching brand.rs
  brandBlue: AdaptiveColor;
  brandRed: AdaptiveColor;
  brandOrange: AdaptiveColor;
  brandGreen: AdaptiveColor;
  brandCyan: AdaptiveColor;
};

// Direct mapping from brand.rs ANSI true-color values
export const neuronTheme: NeuronTheme = {
  primary: { dark: '#4169C3', light: '#3b7dd8' },     // BLUE
  secondary: { dark: '#5c9cf5', light: '#7b5bb6' },
  accent: { dark: '#F0A028', light: '#d68c27' },       // ORANGE
  error: { dark: '#C83228', light: '#d1383d' },         // RED
  warning: { dark: '#F0A028', light: '#d68c27' },
  success: { dark: '#2D8C3C', light: '#3d9a57' },      // GREEN
  info: { dark: '#5AC8FA', light: '#318795' },          // CYAN
  text: { dark: '#DCDCE6', light: '#2a2a2a' },          // WHITE
  muted: { dark: '#888888', light: '#8a8a8a' },         // DIM
  background: { dark: '#121218', light: '#f8f8f8' },
  panel: { dark: '#1a1a24', light: '#f0f0f0' },
  border: { dark: '#4169C3', light: '#d3d3d3' },        // Blue borders
  // Brand colors (exact match to brand.rs)
  brandBlue: { dark: '#4169C3', light: '#4169C3' },
  brandRed: { dark: '#C83228', light: '#C83228' },
  brandOrange: { dark: '#F0A028', light: '#F0A028' },
  brandGreen: { dark: '#2D8C3C', light: '#2D8C3C' },
  brandCyan: { dark: '#5AC8FA', light: '#5AC8FA' },
};

// Legacy alias for compatibility
export const opencodeTheme = neuronTheme;

export const icons = {
  mark: '⌬',
  check: '✓',
  error: '✗',
  warning: '⚠',
  loading: '⟳',
  arrow: '▸',
  diamond: '◆',
  dot: '●',
  ring: '○',
  block: '█',
} as const;

/**
 * Resolve an AdaptiveColor to a hex string for the current terminal mode.
 * Defaults to dark mode since most developer terminals are dark.
 */
export function color(token: AdaptiveColor, mode: 'dark' | 'light' = 'dark'): string {
  return token[mode];
}
