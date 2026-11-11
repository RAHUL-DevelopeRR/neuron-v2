/**
 * The status line's account of a running turn:
 *
 *   ● writing grocery-shop/style.css +412 ⠹                       1m 52s · ↓ 1.2K tokens  esc to interrupt
 *   ● editing src/app.ts +12 -3 ⠼ · no new tokens for 14s
 *   ⠼ Waiting for the model · no tokens for 32s
 *
 * The verb and target come from the live turn (engine/live-turn.ts); this
 * draws them, with a spinner, and the turn's output tokens so far.
 */

import React, { useEffect, useRef, useState } from 'react'

import { ShimmerText } from './shimmer-text'
import {
  describeLiveActivity,
  liveOutputTokens,
  liveTurnNow,
} from '../engine/live-turn'
import { useTheme } from '../hooks/use-theme'
import { useEngineStore } from '../state/engine-store'
import { useLiveTurnStore, type LiveTurnState } from '../state/live-turn-store'
import { formatTokenCount } from '../utils/format-token-count'
import { GLYPHS } from '../utils/glyphs'
import { renderProbe } from '../utils/render-probe'

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

/**
 * The live turn as the status line decides when to draw it. Every token
 * moves `lastTokenAt`, which only matters once the stream has been silent for
 * STALL_AFTER_MS (engine/live-turn.ts): here it is kept to the whole second after it, so
 * the line is drawn again when what it says can change (a new call, the
 * stream turning from thinking to responding), not for every token. The live
 * clock's tick draws it again each second anyway.
 */
export function liveActivityInput(state: LiveTurnState): LiveTurnState {
  const lastTokenAt =
    state.lastTokenAt === null ? null : Math.ceil(state.lastTokenAt / 1000) * 1000
  return lastTokenAt === state.lastTokenAt ? state : { ...state, lastTokenAt }
}

const ACTIVITY_FIELDS = ['active', 'calls', 'model', 'stream', 'requestSentAt', 'lastTokenAt'] as const

/** The fields of the live turn the status line reads, unchanged until one of them changes. */
function sameActivityInput(a: LiveTurnState, b: LiveTurnState): boolean {
  return ACTIVITY_FIELDS.every((field) => Object.is(a[field], b[field]))
}

/** {@link liveActivityInput}, the same object for as long as the line would read the same. */
function useLiveActivityInput(): LiveTurnState {
  const previous = useRef<LiveTurnState | null>(null)
  return useLiveTurnStore((state) => {
    const next = liveActivityInput(state)
    if (previous.current && sameActivityInput(previous.current, next)) return previous.current
    previous.current = next
    return next
  })
}

/** What the running turn is doing, for the status line's left side. */
export const LiveActivityLine = () => {
  renderProbe.onRender?.('LiveActivityLine')
  const theme = useTheme()
  const live = useLiveActivityInput()
  const awaitingApproval = useEngineStore((state) => state.permissionQueue.length > 0)
  const cwd = useEngineStore((state) => state.status?.cwd ?? null)
  useLiveClock()
  // Drawn with the exact time of the last token: the rounded one only says when to draw.
  const { lastTokenAt } = useLiveTurnStore.getState()
  const activity = describeLiveActivity(
    { ...live, lastTokenAt },
    { now: liveTurnNow(), awaitingApproval, cwd },
  )
  const color = activity.stalled ? theme.warning : theme.secondary
  const head = activity.target
    ? `${activity.verb} ${shortTarget(activity.target)}`
    : activity.verb
  if (activity.writing) {
    // `● writing index.js +91 ⠹`: the file and its count lead, the spinner says
    // it is still going, and a quiet stream is named after it.
    return (
      <>
        <span fg={theme.success}>{GLYPHS.activity}</span>
        <span> </span>
        <ShimmerText text={head} interval={SHIMMER_INTERVAL_MS} primaryColor={theme.secondary} />
        {activity.detail ? <span fg={theme.success}>{` ${activity.detail}`}</span> : null}
        <span> </span>
        <Spinner color={color} />
        {activity.writing.silence ? (
          <span fg={theme.warning}>{` · ${activity.writing.silence}`}</span>
        ) : null}
      </>
    )
  }
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
