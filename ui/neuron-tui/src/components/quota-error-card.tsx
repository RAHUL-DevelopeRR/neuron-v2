/**
 * The error card of a turn a usage limit stopped: when the limit resets, a
 * toggle to continue by itself then, and the countdown while that is armed
 * (Esc or a click cancels). Once the limit has reset, the card says so
 * instead of standing as a live error. Its words are utils/quota-card.ts;
 * its state is state/quota-store.ts.
 */
import { useKeyboard } from '@opentui/react'
import { sanitizeTerminalText } from '@neuron-tui/common/util/terminal-safe-text'
import React, { useCallback, useEffect, useState } from 'react'

import { Button } from './button'
import { useTheme } from '../hooks/use-theme'
import { useQuotaStore } from '../state/quota-store'
import { quotaCardView } from '../utils/quota-card'
import { BORDER_CHARS } from '../utils/ui-constants'

import type { QuotaBlock } from '../state/quota-store'
import type { KeyEvent } from '@opentui/core'

/** The clock, re-read every `everyMs` while the card is up. */
export function useNow(everyMs: number): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), everyMs)
    return () => clearInterval(timer)
  }, [everyMs])
  return now
}

interface QuotaErrorCardProps {
  block: QuotaBlock
  error: string
}

export const QuotaErrorCard = React.memo(function QuotaErrorCard({
  block,
  error,
}: QuotaErrorCardProps) {
  const theme = useTheme()
  const autoContinue = useQuotaStore((state) => state.autoContinue)
  const continueAt = useQuotaStore((state) => state.continueAt)
  const stoppedReason = useQuotaStore((state) => state.stoppedReason)
  // Seconds matter only while the countdown runs.
  const now = useNow(continueAt !== null ? 1_000 : 30_000)
  const view = quotaCardView({ block, autoContinue, continueAt, stoppedReason, now })

  useKeyboard(
    useCallback(
      (key: KeyEvent) => {
        if (key.name !== 'escape') return
        if (useQuotaStore.getState().continueAt === null) return
        key.preventDefault?.()
        useQuotaStore.getState().cancel()
      },
      [],
    ),
  )

  const toggle = () => {
    const quota = useQuotaStore.getState()
    quota.setAutoContinue(!quota.autoContinue)
  }
  const accent = view.stale ? theme.muted : theme.error

  return (
    <box
      style={{
        width: '100%',
        borderStyle: 'single',
        borderColor: accent,
        customBorderChars: BORDER_CHARS,
        paddingLeft: 1,
        paddingRight: 1,
        flexDirection: 'column',
        gap: 0,
        marginTop: 1,
      }}
    >
      <text style={{ fg: accent, wrapMode: 'word' }}>{view.title}</text>
      <text style={{ fg: view.stale ? theme.muted : theme.foreground, wrapMode: 'word' }}>
        {sanitizeTerminalText(error).trim()}
      </text>
      {view.resetLine && (
        <text style={{ fg: view.stale ? theme.foreground : theme.warning, wrapMode: 'word' }}>
          {view.resetLine}
        </text>
      )}
      {view.toggle && (
        <Button onClick={toggle} style={{ alignSelf: 'flex-start' }}>
          <text style={{ fg: theme.primary }}>{view.toggle}</text>
        </Button>
      )}
      {view.countdown && (
        <Button
          onClick={() => useQuotaStore.getState().cancel()}
          style={{ alignSelf: 'flex-start' }}
        >
          <text style={{ fg: theme.muted }}>{view.countdown}</text>
        </Button>
      )}
    </box>
  )
})
