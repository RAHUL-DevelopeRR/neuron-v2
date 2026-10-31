/**
 * One line above the input while the commit card or the rewind picker waits
 * behind the composer (it came while the user was typing, or they typed into
 * it): what is waiting, and how to bring it up.
 */

import React from 'react'

import { useTheme } from '../hooks/use-theme'
import { useGitViewsStore } from '../state/git-views-store'

/** The line's text, or `null` when nothing waits. */
export function gitPanelWaitingText(state: {
  deferred: boolean
  proposal: unknown
  rewind: unknown
}): string | null {
  if (!state.deferred) return null
  if (state.proposal) return 'Commit proposal ready: press Enter on an empty input to review it.'
  if (state.rewind) return 'Rewind ready: press Enter on an empty input to pick a checkpoint.'
  return null
}

export const GitPanelWaiting = () => {
  const theme = useTheme()
  const text = useGitViewsStore(gitPanelWaitingText)
  if (!text) return null
  return (
    <box style={{ paddingLeft: 1, paddingRight: 1, flexShrink: 0 }}>
      <text style={{ wrapMode: 'word', fg: theme.secondary }}>{text}</text>
    </box>
  )
}
