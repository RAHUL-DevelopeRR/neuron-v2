// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: every
// variant of the logo is drawn in NeuronCLI's brand colours, letter by letter.
// See ui/neuron-tui/PORTING.md.
import React, { useMemo } from 'react'

import {
  LOGO,
  LOGO_LETTER_COLORS,
  LOGO_SMALL,
  SHADOW_CHARS,
  TEXT_LOGO,
  logoLetterColor,
  logoShadowColor,
  parseLogoLines,
} from '../utils/logo'

import type { ThemeName } from '../types/theme-system'

interface UseLogoOptions {
  /**
   * Available width for rendering the logo
   */
  availableWidth: number
  /**
   * Optional function to apply styling to each character (e.g., for sheen animation)
   * If not provided, default coloring is applied (brand-coloured letters, dimmed shadows)
   */
  applySheenToChar?: (char: string, charIndex: number, lineIndex: number) => React.ReactNode
  /**
   * Color of the " CLI" after the text variant's brand-coloured "Neuron"
   */
  textColor?: string
  /**
   * The theme the dimmed shadows blend toward (dark by default)
   */
  themeName?: ThemeName
  /**
   * Optional vertical budget (in rows) for the logo. When fewer than the
   * ASCII art's 6 lines are available, the hook downgrades to the single-line
   * text variant so callers on short terminals don't have to special-case it.
   */
  maxHeight?: number
}

interface LogoResult {
  /**
   * The formatted logo as a React component ready to render in UI
   */
  component: React.ReactNode
  /**
   * The formatted logo string for plain text contexts (e.g., chat messages)
   * Empty string for narrow widths, formatted ASCII art otherwise
   */
  textBlock: string
}

/**
 * Hook to render a logo based on available width
 * Returns a fully formatted React component and text block that "just work"
 *
 * Returns:
 * - Full ASCII logo for width >= 70
 * - Small ASCII logo for width >= 40
 * - Text variant "NEURON" or "Neuron CLI" for narrow widths
 *
 * The hook handles ALL formatting internally including:
 * - Line parsing and width limiting
 * - Optional character-level styling (sheen animation) for React component
 * - Text wrapping and block formatting for plain text contexts
 * - No consumer needs to know about parseLogoLines, split, join, etc.
 */
export const useLogo = ({
  availableWidth,
  applySheenToChar,
  textColor,
  themeName = 'dark',
  maxHeight,
}: UseLogoOptions): LogoResult => {
  // The ASCII art (full and small) is 6 lines tall. If the caller can't spare
  // that many rows, collapse straight to the single-line text variant.
  const ASCII_LOGO_LINES = 6
  const rawLogoString = useMemo(() => {
    if (maxHeight != null && maxHeight < ASCII_LOGO_LINES) {
      return TEXT_LOGO
    }
    if (availableWidth >= 70) return LOGO
    if (availableWidth >= 20) return LOGO_SMALL
    return TEXT_LOGO
  }, [availableWidth, maxHeight])

  // Format text block for plain text contexts (chat messages, etc.)
  const textBlock = useMemo(() => {
    if (rawLogoString === TEXT_LOGO) {
      return '' // Don't show ASCII art for text-only variant in plain text contexts
    }
    // Parse and format for plain text display
    return parseLogoLines(rawLogoString)
      .map((line) => line.slice(0, availableWidth))
      .join('\n')
  }, [rawLogoString, availableWidth])

  // Format component for React contexts (login modal, etc.)
  const component = useMemo(() => {
    // Text-only variant for very narrow widths
    if (rawLogoString === TEXT_LOGO) {
      const brandName = 'Neuron'
      // When we collapsed to text purely to fit a short terminal (not because
      // the terminal is narrow), keep it to the bare brand name — "Neuron
      // CLI" reads as filler in that already-cramped space.
      const forcedByHeight = maxHeight != null && maxHeight < ASCII_LOGO_LINES
      const showCli = !(availableWidth < 30 || forcedByHeight)

      return (
        <text style={{ wrapMode: 'none' }}>
          <b>
            {brandName.split('').map((letter, index) => (
              <span key={index} fg={LOGO_LETTER_COLORS[index]}>
                {letter}
              </span>
            ))}
            {showCli &&
              (textColor ? <span fg={textColor}> CLI</span> : <> CLI</>)}
          </b>
        </text>
      )
    }

    // ASCII art variant
    const logoLines = parseLogoLines(rawLogoString)
    const displayLines = logoLines.map((line) => line.slice(0, availableWidth))

    // Default coloring: each letter in its brand colour, its shadow dimmed
    const defaultColorChar = (char: string, charIndex: number) => {
      if (char === ' ' || char === '\n') {
        return <span key={charIndex}>{char}</span>
      }
      const color = SHADOW_CHARS.has(char)
        ? logoShadowColor(charIndex, themeName)
        : logoLetterColor(charIndex)
      return <span key={charIndex} fg={color}>{char}</span>
    }

    return (
      <>
        {displayLines.map((line, lineIndex) => (
          <text key={`logo-line-${lineIndex}`} style={{ wrapMode: 'none' }}>
            {line
              .split('')
              .map((char, charIndex) =>
                applySheenToChar
                  ? applySheenToChar(char, charIndex, lineIndex)
                  : defaultColorChar(char, charIndex),
              )}
          </text>
        ))}
      </>
    )
  }, [rawLogoString, availableWidth, applySheenToChar, textColor, themeName, maxHeight])

  return { component, textBlock }
}
