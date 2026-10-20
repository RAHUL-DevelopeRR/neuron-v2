// Derived from Codebuff/Freebuff cli/src/login/constants.ts and utils.ts
// (Apache-2.0); modified for Neuron: only the logo art and its sheen helpers
// are kept, without the login flow and its Codebuff/Freebuff URLs, and the
// logo is now Neuron's. See ui/neuron-tui/PORTING.md.

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
 * Determines the color for a character based on its position relative to the sheen
 * Block characters use blockColor, shadow/border characters animate to accent green
 * @param accentColor - The accent color to use for the sheen effect (typically theme.primary)
 * @param blockColor - The color for solid block characters (white for dark mode, black for light mode)
 * @param isReversing - Whether the sheen is in the reverse (unfill) phase
 */
export function getSheenColor(
  char: string,
  charIndex: number,
  sheenPosition: number,
  logoColor: string,
  shadowChars: Set<string>,
  accentColor: string = '#9EFC62',
  blockColor: string = '#ffffff',
  isReversing: boolean = false,
): string {
  // Block characters use the specified block color
  if (char === '█') {
    return blockColor
  }

  // Only apply sheen to shadow/border characters
  if (!shadowChars.has(char)) {
    return logoColor
  }

  if (isReversing) {
    // Reverse phase: characters behind the sheen return to logoColor
    if (charIndex <= sheenPosition) {
      return logoColor
    }
    // Characters ahead of the sheen stay accent color
    return accentColor
  } else {
    // Forward phase: characters at or behind the sheen get the accent color
    if (charIndex <= sheenPosition) {
      return accentColor
    }
    // Characters ahead of the sheen remain original color
    return logoColor
  }
}

/**
 * Parses the logo string into individual lines
 */
export function parseLogoLines(logo: string): string[] {
  return logo.split('\n').filter((line) => line.length > 0)
}
