// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the status
// markers come from utils/glyphs.ts (console-safe in a classic Windows console
// window). See ui/neuron-tui/PORTING.md.
import { GLYPHS } from './glyphs'

import type { AgentContentBlock } from '../types/chat'

export interface StatusInfo {
  indicator: string
  label: string
  color: string
  text: string
}

/** Get status indicator, label, color, and formatted text based on agent status */
export function getAgentStatusInfo(
  status: AgentContentBlock['status'],
  theme: { primary: string; foreground: string; muted: string },
): StatusInfo {
  switch (status) {
    case 'running':
      return { indicator: '●', label: 'running', color: theme.primary, text: '● running' }
    case 'failed':
      return {
        indicator: GLYPHS.cross,
        label: 'failed',
        color: 'red',
        text: `${GLYPHS.cross} failed`,
      }
    case 'complete':
      return {
        indicator: GLYPHS.check,
        label: 'completed',
        color: theme.foreground,
        text: `completed ${GLYPHS.check}`,
      }
    case 'cancelled':
      return {
        indicator: GLYPHS.cancelled,
        label: 'cancelled',
        color: 'red',
        text: `${GLYPHS.cancelled} cancelled`,
      }
    default:
      return { indicator: '○', label: 'waiting', color: theme.muted, text: '○ waiting' }
  }
}
