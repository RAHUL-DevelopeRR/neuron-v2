// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the check
// marks and boxes come from utils/glyphs.ts. See ui/neuron-tui/PORTING.md.
import { GLYPHS } from '../../utils/glyphs'

/**
 * Visual symbols used in the UI
 */
export const SYMBOLS = {
  /** Selected radio button */
  SELECTED: '●',

  /** Unselected radio button */
  UNSELECTED: '○',

  /** Completed question indicator */
  COMPLETED: GLYPHS.check,

  /** Current question indicator */
  CURRENT: '●',

  /** Checked checkbox (multi-select) */
  CHECKBOX_CHECKED: GLYPHS.checkboxOn,

  /** Unchecked checkbox (multi-select) */
  CHECKBOX_UNCHECKED: GLYPHS.checkboxOff,
} as const

/** Option type - can be string or object with label/description */
export type AskUserOption = string | { label: string; description?: string }

/** Helper to extract label from an option (handles both string and object formats) */
export const getOptionLabel = (option: AskUserOption): string => {
  return typeof option === 'string' ? option : option?.label ?? ''
}

/** Constant for the "Custom" option index */
export const CUSTOM_OPTION_INDEX: number = -1

export const KEYBOARD_HINTS = [
  '↑↓ navigate •',
  'Enter select',
] as const
