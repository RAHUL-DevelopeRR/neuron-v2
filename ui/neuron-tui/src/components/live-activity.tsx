/**
 * The status line's account of a running turn:
 *
 *   ⠹ Writing grocery-shop/style.css · 412 lines · 12.3 KB       1m 52s · ↓ 1.2K tokens  esc to interrupt
 *   ⠼ Waiting for the model · no tokens for 32s
 *
 * The verb and target come from the live turn (engine/live-turn.ts); this
 * draws them, with a spinner, and the turn's output tokens so far.
 */

import React, { useEffect, useState } from 'react'

import { ShimmerText } from './shimmer-text'
import {
  describeLiveActivity,
  liveOutputTokens,
  liveTurnNow,
} from '../engine/live-turn'
import { useTheme } from '../hooks/use-theme'
import { useEngineStore } from '../state/engine-store'
import { useLiveTurnStore } from '../state/live-turn-store'
import { formatTokenCount } from '../utils/format-token-count'
import { GLYPHS } from '../utils/glyphs'

const SPINNER_INTERVAL_MS = 100
const SHIMMER_INTERVAL_MS = 160
/** The longest target the line shows whole. */
export const MAX_TARGET_LENGTH = 48

export const Spinner = ({ color }: { color: string }) => {
  const frames = [...GLYPHS.spinnerFrames]
  const [frame, setFrame] = useState(0)
  useEffect(() => {
    const interval = setInterval(
      () => setFrame((current) => (current + 1) % frames.length),
      SPINNER_INTERVAL_MS,
    )
    return () => clearInterval(interval)
  }, [frames.length])
  return <span fg={color}>{frames[frame % frames.length]}</span>
}

/**
 * A target that fits: a long path keeps its end (the file name), a long
 * command its start.
 */
export function shortTarget(target: string, max = MAX_TARGET_LENGTH): string {
  if (target.length <= max) return target
  const isPath = /[\\/]/.test(target) && !/\s/.test(target)
  return isPath ? `…${target.slice(-(max - 1))}` : `${target.slice(0, max - 1)}…`
}

/** The live clock, read again every second while mounted. */
export function useLiveClock(): number {
  const [now, setNow] = useState(liveTurnNow)
  useEffect(() => {
    const interval = setInterval(() => setNow(liveTurnNow()), 1000)
    return () => clearInterval(interval)
  }, [])
  return now
}

/** What the running turn is doing, for the status line's left side. */
export const LiveActivityLine = () => {
  const theme = useTheme()
  const live = useLiveTurnStore((state) => state)
  const awaitingApproval = useEngineStore((state) => state.permissionQueue.length > 0)
  useLiveClock()
  const activity = describeLiveActivity(live, { now: liveTurnNow(), awaitingApproval })
  const color = activity.stalled ? theme.warning : theme.secondary
  const head = activity.target
    ? `${activity.verb} ${shortTarget(activity.target)}`
    : activity.verb
  return (
    <>
      <Spinner color={color} />
      <span> </span>
      {activity.stalled ? (
        <span fg={color}>{head}</span>
      ) : (
        <ShimmerText text={head} interval={SHIMMER_INTERVAL_MS} primaryColor={color} />
      )}
      {activity.detail ? <span fg={color}>{` · ${activity.detail}`}</span> : null}
    </>
  )
}

/**
 * "↓ 1.2K tokens" for this turn's output so far, after `separator`; nothing
 * before any (older engines report tokens only when the turn ends).
 */
export const LiveTokenCount = ({ separator = '' }: { separator?: string }) => {
  const theme = useTheme()
  const tokens = useLiveTurnStore(liveOutputTokens)
  if (tokens <= 0) return null
  return (
    <span fg={theme.secondary}>{`${separator}↓ ${formatTokenCount(tokens)} tokens`}</span>
  )
}
