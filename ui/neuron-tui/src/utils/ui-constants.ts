// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the border
// characters come from utils/glyphs.ts, so a classic Windows console window
// gets square corners. See ui/neuron-tui/PORTING.md.
import { BORDERS } from './glyphs'

import type { BorderCharacters } from '@opentui/core'

/**
 * Dark text color for focused/inverted controls (dark label on the green
 * `theme.primary` fill). The themes set `background: 'transparent'`, so it
 * can't be used as an inverted-text color — a transparent foreground on the
 * green fill renders the label invisible. This near-black reads cleanly on the
 * bright green fill in both the dark and light themes.
 */
export const INVERTED_CTA_FG = '#10131a'

/** Rounded corners (square in a classic Windows console). */
export const BORDER_CHARS: BorderCharacters = BORDERS.rounded

/** Dashed border characters with rounded corners for ghost/ephemeral UI */
export const DASHED_BORDER_CHARS: BorderCharacters = BORDERS.dashed

/** Square corner border for image cards (separate from the rounded default) */
export const IMAGE_CARD_BORDER_CHARS: BorderCharacters = BORDERS.square

/** Dashed border with rounded corners for proposal cards */
export const PROPOSAL_BORDER_CHARS: BorderCharacters = BORDERS.dotted
