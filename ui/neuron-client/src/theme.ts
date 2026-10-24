/**
 * Neuron's brand palette, taken from the NeuronCLI 6.2.5 terminal UI: the
 * four-colour pixel logo (blue N, red E, orange U, green O/N, with a cyan
 * variant on the splash), on a deep navy. The desktop and mobile apps share it.
 */

export const brand = {
  blue: '#4169c3',
  red: '#c83228',
  orange: '#f0a028',
  green: '#2d8c3c',
  cyan: '#59c8fa',
} as const

/** The logo's letters in order, and their colours (as the 6.2.5 sidebar draws them). */
export const logoLetters: ReadonlyArray<readonly [string, string]> = [
  ['N', brand.blue],
  ['E', brand.red],
  ['U', brand.orange],
  ['R', brand.orange],
  ['O', brand.green],
  ['N', brand.green],
]

export const dark = {
  /** The chat background. */
  bg: '#1a1a2e',
  /** Panels: the sidebar, the status bar. */
  panel: '#1e1e32',
  /** Wells: inputs, code, the terminal. */
  sunken: '#121220',
  /** The splash screen. */
  void: '#0c0c0c',
  border: '#2c2c44',
  text: '#dcdce6',
  textMuted: '#bdbdc8',
  textDim: '#888888',
  textFaint: '#575768',
  /** The user's messages carry an orange bar, the assistant's a blue one. */
  user: brand.orange,
  assistant: brand.blue,
  accent: brand.blue,
  link: brand.cyan,
  /** The model chip in the status bar. */
  chip: brand.orange,
  chipText: '#121220',
  success: '#3fae52',
  warning: brand.orange,
  danger: '#e0483d',
  diffAddBg: 'rgba(45, 140, 60, 0.22)',
  diffAddText: '#8fd99b',
  diffRemoveBg: 'rgba(200, 50, 40, 0.22)',
  diffRemoveText: '#f19a92',
} as const

export const light = {
  bg: '#f7f7fb',
  panel: '#ececf4',
  sunken: '#ffffff',
  void: '#ffffff',
  border: '#d6d6e2',
  text: '#1a1a2e',
  textMuted: '#3c3c52',
  textDim: '#6a6a7c',
  textFaint: '#9a9aac',
  user: '#c77d0a',
  assistant: brand.blue,
  accent: brand.blue,
  link: '#1f7fb5',
  chip: brand.orange,
  chipText: '#121220',
  success: '#23843a',
  warning: '#b06d00',
  danger: '#c0352b',
  diffAddBg: 'rgba(45, 140, 60, 0.14)',
  diffAddText: '#1d6b2b',
  diffRemoveBg: 'rgba(200, 50, 40, 0.12)',
  diffRemoveText: '#9e2a21',
} as const

export type Palette = { [K in keyof typeof dark]: string }

export const fonts = {
  mono: '"JetBrains Mono", "Cascadia Mono", "SF Mono", Consolas, Menlo, monospace',
  ui: '"Inter", "Segoe UI", system-ui, -apple-system, sans-serif',
} as const
