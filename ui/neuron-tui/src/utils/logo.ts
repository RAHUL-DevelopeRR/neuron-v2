// Derived from Codebuff/Freebuff cli/src/login/constants.ts and utils.ts
// (Apache-2.0); modified for Neuron: only the logo art and its sheen helpers
// are kept, without the login flow and its Codebuff/Freebuff URLs, and the
// logo is now Neuron's, each letter in its NeuronCLI brand colour. See
// ui/neuron-tui/PORTING.md.

import type { ThemeName } from '../types/theme-system'

/** NeuronCLI's brand colours, as in rust/crates/rusty-claude-cli/src/brand.rs. */
export const BRAND = {
  blue: '#4169C3',
  red: '#C83228',
  orange: '#F0A028',
  green: '#2D8C3C',
  cyan: '#5AC8FA',
} as const

/** The letters of "Neuron" in NeuronCLI's colours: N blue, e red, u and r orange, o and n green. */
export const LOGO_LETTER_COLORS = [
  BRAND.blue,
  BRAND.red,
  BRAND.orange,
  BRAND.orange,
  BRAND.green,
  BRAND.green,
] as const

// Neuron ASCII logo, in the block style of the logo it replaces; 54 columns
// wide, so it fits the full-logo slot (70+ columns).
export const LOGO = `
 ███╗   ██╗███████╗██╗   ██╗██████╗  ██████╗ ███╗   ██╗
 ████╗  ██║██╔════╝██║   ██║██╔══██╗██╔═══██╗████╗  ██║
 ██╔██╗ ██║█████╗  ██║   ██║██████╔╝██║   ██║██╔██╗ ██║
 ██║╚██╗██║██╔══╝  ██║   ██║██╔══██╗██║   ██║██║╚██╗██║
 ██║ ╚████║███████╗╚██████╔╝██║  ██║╚██████╔╝██║ ╚████║
 ╚═╝  ╚═══╝╚══════╝ ╚═════╝ ╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═══╝
`

// The N alone, for narrower terminals.
export const LOGO_SMALL = `
 ███╗   ██╗
 ████╗  ██║
 ██╔██╗ ██║
 ██║╚██╗██║
 ██║ ╚████║
 ╚═╝  ╚═══╝
`

/** What the logo collapses to when even the small one does not fit. */
export const TEXT_LOGO = 'NEURON'

// The column each letter of LOGO starts at (LOGO_SMALL is its first letter).
const LOGO_LETTER_STARTS = [0, 11, 19, 28, 36, 45]

/** The brand colour of the logo letter at column `charIndex`. */
export function logoLetterColor(charIndex: number): string {
  let letter = 0
  while (letter + 1 < LOGO_LETTER_STARTS.length && charIndex >= LOGO_LETTER_STARTS[letter + 1]) {
    letter++
  }
  return LOGO_LETTER_COLORS[letter]
}

/** The letter's colour dimmed toward the background, for its shadow while the sheen is away. */
export function logoShadowColor(charIndex: number, themeName: ThemeName): string {
  return mixHex(logoLetterColor(charIndex), themeName === 'dark' ? '#000000' : '#ffffff', 0.45)
}

function mixHex(from: string, to: string, amount: number): string {
  const channel = (hex: string, at: number) => parseInt(hex.slice(at, at + 2), 16)
  return `#${[1, 3, 5]
    .map((at) => Math.round(channel(from, at) + (channel(to, at) - channel(from, at)) * amount))
    .map((value) => value.toString(16).padStart(2, '0'))
    .join('')}`
}

// Shadow/border characters that receive the sheen animation effect
export const SHADOW_CHARS = new Set([
  '╚',
  '═',
  '╝',
  '║',
  '╔',
  '╗',
  '╠',
  '╣',
  '╦',
  '╩',
  '╬',
])

// Sheen animation constants
export const SHEEN_STEP = 2 // Advance 2 positions per frame for efficiency
export const SHEEN_INTERVAL_MS = 150

/**
 * Determines the color for a character based on its position relative to the sheen.
 * Blocks are always their letter's brand colour; shadow/border characters are
 * dimmed, and the sheen lights them up in the letter's colour.
 * @param isReversing - Whether the sheen is in the reverse (unfill) phase
 */
export function getSheenColor(
  char: string,
  charIndex: number,
  sheenPosition: number,
  shadowChars: Set<string>,
  themeName: ThemeName,
  isReversing: boolean = false,
): string {
  if (!shadowChars.has(char)) {
    return logoLetterColor(charIndex)
  }
  // Forward phase: lit at or behind the sheen. Reverse phase: lit only ahead of it.
  const lit = isReversing ? charIndex > sheenPosition : charIndex <= sheenPosition
  return lit ? logoLetterColor(charIndex) : logoShadowColor(charIndex, themeName)
}

/**
 * Parses the logo string into individual lines
 */
export function parseLogoLines(logo: string): string[] {
  return logo.split('\n').filter((line) => line.length > 0)
}
