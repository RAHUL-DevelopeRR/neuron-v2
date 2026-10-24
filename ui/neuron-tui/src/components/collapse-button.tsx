// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the arrow
// comes from utils/glyphs.ts. See ui/neuron-tui/PORTING.md.
import { memo } from 'react'

import { Button } from './button'
import { useTheme } from '../hooks/use-theme'
import { GLYPHS } from '../utils/glyphs'

import type { CSSProperties } from 'react'

interface CollapseButtonProps {
  onClick: () => void
  style?: CSSProperties
}

/**
 * Reusable collapse button with '▴ collapse' text
 * Styled with theme.secondary color, aligned to the right by default
 */
export const CollapseButton = memo(({ onClick, style }: CollapseButtonProps) => {
  const theme = useTheme()

  return (
    <Button
      style={{
        alignSelf: 'flex-end',
        marginTop: 0,
        ...style,
      }}
      onClick={onClick}
    >
      <text fg={theme.secondary} style={{ wrapMode: 'none' }}>
        {`${GLYPHS.collapseUp} collapse`}
      </text>
    </Button>
  )
})
