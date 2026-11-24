// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: it says
// what an untrusted folder means and how to trust it, the logo is in
// NeuronCLI's brand colours, and Neura, the mascot, stands beside it when
// there is room. See ui/neuron-tui/PORTING.md.
import { memo, useMemo, useState } from 'react'

import { UNTRUSTED_FOLDER_NOTE } from '../engine/status-report'
import { useLogo } from '../hooks/use-logo'
import { useSheenAnimation } from '../hooks/use-sheen-animation'
import { useTerminalDimensions } from '../hooks/use-terminal-dimensions'
import { useTheme } from '../hooks/use-theme'
import { useEngineStore } from '../state/engine-store'
import { getGlyphMode } from '../utils/glyphs'
import { LOGO, logoVariant, parseLogoLines } from '../utils/logo'
import { MASCOT_HEIGHT, MASCOT_WIDTH, mascotLinesFor } from '../utils/mascot'
import { openFileAtPath } from '../utils/open-file'
import { formatCwd } from '../utils/path-helpers'
import { TerminalLink } from './terminal-link'

import type { MascotLine } from '../utils/mascot'

/** Columns between the mascot and the logo (the logo's lines start with a space too). */
const MASCOT_GAP = 1

/**
 * Columns of the terminal the header's text does not get: the chat's side
 * padding (1 and 2), its scrollbar, and the header's own padding (1 and 1).
 */
const HEADER_SIDE_COLUMNS = 6

const FULL_LOGO_LINES = parseLogoLines(LOGO).length
const FULL_LOGO_WIDTH = Math.max(...parseLogoLines(LOGO).map((line) => line.length))

/**
 * Whether the mascot fits beside the logo: only beside the full logo, and
 * only when both fit in the header's width, so the logo never shrinks to
 * make room for it.
 */
export function showsMascot(terminalWidth: number, contentMaxWidth: number): boolean {
  if (logoVariant(contentMaxWidth) !== LOGO) return false
  const room = Math.min(contentMaxWidth, terminalWidth - HEADER_SIDE_COLUMNS)
  return MASCOT_WIDTH + MASCOT_GAP + FULL_LOGO_WIDTH <= room
}

/** Neura in half blocks, one text line per two grid rows. */
const Mascot = memo(function Mascot({ lines }: { lines: MascotLine[] }) {
  return (
    <box style={{ flexDirection: 'column', flexShrink: 0 }}>
      {lines.map((line, lineIndex) => (
        <text key={`mascot-line-${lineIndex}`} style={{ wrapMode: 'none' }}>
          {line.map((cell, cellIndex) => (
            <span key={cellIndex} fg={cell.fg} bg={cell.bg}>
              {cell.char}
            </span>
          ))}
        </text>
      ))}
    </box>
  )
})

export const ChatHeader = memo(function ChatHeader({
  projectRoot,
  animationEnabled,
}: {
  projectRoot: string
  animationEnabled: boolean
}) {
  const { contentMaxWidth, terminalWidth } = useTerminalDimensions()
  const theme = useTheme()
  // The footer's "untrusted folder", explained (the engine has no command
  // to trust it).
  const untrusted = useEngineStore(
    (state) => state.status !== null && !state.status.workspace_trusted,
  )
  const [sheenPosition, setSheenPosition] = useState(0)
  const { applySheenToChar } = useSheenAnimation({
    enabled: animationEnabled,
    themeName: theme.name,
    terminalWidth,
    sheenPosition,
    setSheenPosition,
  })
  const { component: logoComponent } = useLogo({
    availableWidth: contentMaxWidth,
    themeName: theme.name,
    applySheenToChar,
  })
  const mascotLines = useMemo(
    () => (showsMascot(terminalWidth, contentMaxWidth) ? mascotLinesFor(getGlyphMode()) : null),
    [terminalWidth, contentMaxWidth],
  )
  // The logo is a line shorter than the mascot: it starts lower by half the
  // difference, rounded down, which keeps the two centred within half a line.
  const logoOffset = Math.max(0, Math.floor((MASCOT_HEIGHT - FULL_LOGO_LINES) / 2))

  return (
    <box
      style={{
        flexDirection: 'column',
        gap: 0,
        paddingLeft: 1,
        paddingRight: 1,
      }}
    >
      {mascotLines ? (
        <box
          style={{
            flexDirection: 'row',
            alignItems: 'flex-start',
            gap: MASCOT_GAP,
            marginBottom: 1,
            marginTop: 2,
          }}
        >
          <Mascot lines={mascotLines} />
          <box style={{ flexDirection: 'column', flexShrink: 0, marginTop: logoOffset }}>
            {logoComponent}
          </box>
        </box>
      ) : (
        <box
          style={{
            flexDirection: 'column',
            marginBottom: 1,
            marginTop: 2,
          }}
        >
          {logoComponent}
        </box>
      )}
      <text style={{ wrapMode: 'word', marginBottom: 1, fg: theme.foreground }}>
        Neuron will run commands on your behalf to help you build.
      </text>
      <text style={{ wrapMode: 'word', marginBottom: 1, fg: theme.foreground }}>
        Directory{' '}
        <TerminalLink
          text={formatCwd(projectRoot)}
          color={theme.muted}
          inline={true}
          underlineOnHover={true}
          onActivate={() => openFileAtPath(projectRoot)}
        />
      </text>
      {untrusted && (
        <text style={{ wrapMode: 'word', marginBottom: 1, fg: theme.warning }}>
          {UNTRUSTED_FOLDER_NOTE}
        </text>
      )}
    </box>
  )
})
