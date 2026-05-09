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
};

// Adapted from the local opencode Go TUI theme palette in
// opencode/internal/tui/theme/opencode.go. Kept as data instead of copied Go
// code so the React/Ink frontend can share the same visual language.
export const opencodeTheme: NeuronTheme = {
  primary: {dark: '#fab283', light: '#3b7dd8'},
  secondary: {dark: '#5c9cf5', light: '#7b5bb6'},
  accent: {dark: '#9d7cd8', light: '#d68c27'},
  error: {dark: '#e06c75', light: '#d1383d'},
  warning: {dark: '#f5a742', light: '#d68c27'},
  success: {dark: '#7fd88f', light: '#3d9a57'},
  info: {dark: '#56b6c2', light: '#318795'},
  text: {dark: '#e0e0e0', light: '#2a2a2a'},
  muted: {dark: '#6a6a6a', light: '#8a8a8a'},
  background: {dark: '#212121', light: '#f8f8f8'},
  panel: {dark: '#252525', light: '#f0f0f0'},
  border: {dark: '#4b4c5c', light: '#d3d3d3'}
};

export const icons = {
  mark: '⌬',
  check: '✓',
  error: '✖',
  warning: '⚠',
  loading: '⟳'
};

export function color(token: AdaptiveColor, mode: 'dark' | 'light' = 'dark'): string {
  return token[mode];
}
