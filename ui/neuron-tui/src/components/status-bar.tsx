// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the
// Freebuff session countdown and fill, BYOK label, context usage from the SDK
// run state, "End session" button and capacity-wait line are gone. The engine
// footer (git, tokens, context %) is P3. The not-connected line now names the
// engine's state, without repeating a reason the chat already shows.
// See ui/neuron-tui/PORTING.md.
import { TextAttributes } from '@opentui/core'
import React, { useEffect, useState } from 'react'

import { Button } from './button'
import { ScrollToBottomButton } from './scroll-to-bottom-button'
import { ShimmerText } from './shimmer-text'

import { useTheme } from '../hooks/use-theme'
import { useEngineStore } from '../state/engine-store'
import { formatElapsedTime } from '../utils/format-elapsed-time'

import type { StatusIndicatorState } from '../utils/status-indicator-state'

/** A small status-bar action button with hover-bold styling. */
const StatusActionButton = ({
  children,
  onClick,
}: {
  children: React.ReactNode
  onClick: () => void
}) => {
  const theme = useTheme()
  const [hovered, setHovered] = useState(false)

  return (
    <Button
      style={{ paddingLeft: 1, paddingRight: 1 }}
      onClick={onClick}
      onMouseOver={() => setHovered(true)}
      onMouseOut={() => setHovered(false)}
    >
      <text>
        <span
          fg={theme.secondary}
          attributes={hovered ? TextAttributes.BOLD : TextAttributes.NONE}
        >
          {children}
        </span>
      </text>
    </Button>
  )
}

const SHIMMER_INTERVAL_MS = 160

interface StatusBarProps {
  timerStartTime: number | null
  isAtBottom: boolean
  scrollToLatest: () => void
  statusIndicatorState: StatusIndicatorState
  onStop?: () => void
}

export const StatusBar = ({
  timerStartTime,
  isAtBottom,
  scrollToLatest,
  statusIndicatorState,
  onStop,
}: StatusBarProps) => {
  const theme = useTheme()
  const engineConnection = useEngineStore((state) => state.connection)
  const engineError = useEngineStore((state) => state.error)
  // Why it stopped is already a chat line (or on the turn): said once.
  const engineErrorInChat = useEngineStore((state) => state.errorInChat)
  const [elapsedSeconds, setElapsedSeconds] = useState(0)

  // Show timer when actively working (streaming or waiting for response) or paused (ask_user)
  // This uses statusIndicatorState as the single source of truth for "is the LLM working?"
  const shouldShowTimer =
    statusIndicatorState?.kind === 'waiting' ||
    statusIndicatorState?.kind === 'streaming' ||
    statusIndicatorState?.kind === 'paused'

  useEffect(() => {
    if (!timerStartTime || !shouldShowTimer) {
      setElapsedSeconds(0)
      return
    }

    // When paused, don't update the timer - just keep the frozen value
    if (statusIndicatorState?.kind === 'paused') {
      // Calculate current elapsed time once and freeze it
      const now = Date.now()
      const elapsed = Math.floor((now - timerStartTime) / 1000)
      setElapsedSeconds(elapsed)
      return
    }

    const updateElapsed = () => {
      const now = Date.now()
      const elapsed = Math.floor((now - timerStartTime) / 1000)
      setElapsedSeconds(elapsed)
    }

    updateElapsed()
    const interval = setInterval(updateElapsed, 1000)

    return () => clearInterval(interval)
  }, [timerStartTime, shouldShowTimer, statusIndicatorState?.kind])

  const renderStatusIndicator = () => {
    switch (statusIndicatorState.kind) {
      case 'ctrlC':
        return <span fg={theme.secondary}>Press Ctrl-C again to exit</span>

      case 'clipboard':
        return (
          <span fg={theme.primary}>{statusIndicatorState.message}</span>
        )

      case 'reconnected':
        return <span fg={theme.success}>Reconnected</span>

      case 'retrying':
        return <ShimmerText text="retrying..." primaryColor={theme.warning} />

      case 'connecting':
        // The engine is starting, or is not running (and why).
        return engineConnection === 'starting' ? (
          <ShimmerText
            text="starting the engine..."
            interval={SHIMMER_INTERVAL_MS}
            primaryColor={theme.secondary}
          />
        ) : (
          <span fg={theme.warning}>
            {(!engineErrorInChat && engineError) ||
              'The Neuron engine is not running.'}
          </span>
        )

      case 'waiting':
        return (
          <ShimmerText
            text="thinking..."
            interval={SHIMMER_INTERVAL_MS}
            primaryColor={theme.secondary}
          />
        )

      case 'streaming':
        return (
          <ShimmerText
            text="working..."
            interval={SHIMMER_INTERVAL_MS}
            primaryColor={theme.secondary}
          />
        )

      case 'paused':
        return null

      case 'idle':
        return null
    }
  }

  const renderElapsedTime = () => {
    if (!shouldShowTimer || elapsedSeconds === 0) {
      return null
    }

    return <span fg={theme.secondary}>{formatElapsedTime(elapsedSeconds)}</span>
  }

  const statusIndicatorContent = renderStatusIndicator()
  const elapsedTimeContent = renderElapsedTime()

  // Show gray background when there's a status indicator or timer.
  const hasContent = statusIndicatorContent || elapsedTimeContent

  return (
    <box
      style={{
        // Same full-width parent as the composer's bordered box.
        width: '100%',
        flexShrink: 0,
        flexDirection: 'row',
        alignItems: 'center',
        paddingLeft: 1,
        paddingRight: 1,
        gap: 1,
        backgroundColor: hasContent ? theme.surface : 'transparent',
      }}
    >
      <box
        style={{
          // Keep text above the fill even when it mounts after these columns.
          zIndex: 1,
          flexGrow: 1,
          flexShrink: 1,
          flexBasis: 0,
        }}
      >
        <text
          style={{
            wrapMode: statusIndicatorState.kind === 'connecting' ? 'word' : 'none',
          }}
        >
          {statusIndicatorContent}
        </text>
      </box>

      <box style={{ flexShrink: 0, zIndex: 1 }}>
        {!isAtBottom && <ScrollToBottomButton onClick={scrollToLatest} />}
      </box>

      <box
        style={{
          zIndex: 1,
          flexGrow: statusIndicatorState.kind === 'connecting' ? 0 : 1,
          flexShrink: 1,
          flexBasis: statusIndicatorState.kind === 'connecting' ? 'auto' : 0,
          flexDirection: 'row',
          justifyContent: 'flex-end',
          alignItems: 'center',
          gap: 1,
        }}
      >
        <text style={{ wrapMode: 'none' }}>{elapsedTimeContent}</text>
        {onStop &&
          (statusIndicatorState.kind === 'waiting' ||
            statusIndicatorState.kind === 'streaming') && (
            <StatusActionButton onClick={onStop}>■ Esc</StatusActionButton>
          )}
      </box>
    </box>
  )
}
