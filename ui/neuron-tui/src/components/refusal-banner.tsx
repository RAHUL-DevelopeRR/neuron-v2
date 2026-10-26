import { sanitizeTerminalText } from '@neuron-tui/common/util/terminal-safe-text'
import React from 'react'

import { useTheme } from '../hooks/use-theme'
import { BORDER_CHARS } from '../utils/ui-constants'

interface RefusalBannerProps {
  /** The engine's refusal line: why the model stopped, with the provider's note. */
  message: string
}

/**
 * The end of a turn the model declined to continue (the engine's `refusal`
 * notice). Drawn apart from the answer, which keeps whatever the model wrote
 * before, so a refusal never reads as model text, nor a refusal before any
 * text as an empty reply.
 */
export const RefusalBanner = React.memo(function RefusalBanner({
  message,
}: RefusalBannerProps) {
  const theme = useTheme()

  // The note is the provider's text, passed through: no escape sequences.
  const safeMessage = sanitizeTerminalText(message).trim()
  if (!safeMessage) {
    return null
  }

  return (
    <box
      style={{
        width: '100%',
        borderStyle: 'single',
        borderColor: theme.warning,
        customBorderChars: BORDER_CHARS,
        paddingLeft: 1,
        paddingRight: 1,
        paddingTop: 0,
        paddingBottom: 0,
        flexDirection: 'column',
        gap: 0,
        marginTop: 1,
      }}
    >
      <text style={{ fg: theme.warning, wrapMode: 'word' }}>Declined</text>
      <text style={{ fg: theme.foreground, wrapMode: 'word' }}>{safeMessage}</text>
    </box>
  )
})
