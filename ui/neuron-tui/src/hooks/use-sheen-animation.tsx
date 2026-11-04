// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the sheen
// lights the logo in its letters' brand colours. See ui/neuron-tui/PORTING.md.
import React, { useCallback, useEffect, useState } from 'react'

import {
  SHADOW_CHARS,
  SHEEN_STEP,
  SHEEN_INTERVAL_MS,
  SHEEN_MAX_PHASES,
  getSheenColor,
} from '../utils/logo'

import type { ThemeName } from '../types/theme-system'

interface UseSheenAnimationParams {
  enabled?: boolean
  themeName: ThemeName
  terminalWidth: number | undefined
  sheenPosition: number
  setSheenPosition: (value: number | ((prev: number) => number)) => void
}

/**
 * Custom hook that handles the sheen animation effect on the logo
 * Animates a fill effect: fill with accent color, then unfill back to original,
 * SHEEN_MAX_PHASES times, then rests until it is enabled again. Each tick redraws
 * the logo span by span, so a sheen that never stopped kept an idle chat drawing
 * about 7 frames a second.
 */
export function useSheenAnimation({
  enabled = true,
  themeName,
  terminalWidth,
  sheenPosition,
  setSheenPosition,
}: UseSheenAnimationParams) {
  // Track whether we're in the reverse (unfill) phase
  const [isReversing, setIsReversing] = useState(false)

  // Run the sheen, a fixed number of phases each time it is enabled
  useEffect(() => {
    if (!enabled) return

    const maxPosition = Math.max(10, Math.min((terminalWidth || 80) - 4, 100))
    let position = 0
    let phases = 0
    setSheenPosition(0)
    setIsReversing(false)

    const interval = setInterval(() => {
      position += SHEEN_STEP
      if (position >= maxPosition) {
        // Reached the end: switch direction, or rest after the last phase
        position = 0
        phases += 1
        setIsReversing(phases % 2 === 1)
        if (phases >= SHEEN_MAX_PHASES) clearInterval(interval)
      }
      setSheenPosition(position)
    }, SHEEN_INTERVAL_MS)

    return () => {
      clearInterval(interval)
    }
  }, [enabled, terminalWidth, setSheenPosition])

  // Apply sheen effect to a character based on its position
  const applySheenToChar = useCallback(
    (char: string, charIndex: number) => {
      if (char === ' ' || char === '\n') {
        return <span key={charIndex}>{char}</span>
      }

      const color = getSheenColor(
        char,
        charIndex,
        sheenPosition,
        SHADOW_CHARS,
        themeName,
        isReversing,
      )

      return (
        <span key={charIndex} fg={color}>
          {char}
        </span>
      )
    },
    [sheenPosition, themeName, isReversing],
  )

  return {
    applySheenToChar,
  }
}
