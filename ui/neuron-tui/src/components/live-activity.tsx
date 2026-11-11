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
import { useTerminalDimensions } from '../hooks/use-terminal-dimensions'
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

/**
 * The columns the status line's left side has in a terminal `terminalWidth`
 * wide: the status bar (status-bar.tsx) has one column of padding each side
 * and splits the rest, less two gaps, evenly with the elapsed time and tokens
 * (one column more is kept free, for rounding).
 */
export function statusLineWidth(terminalWidth: number): number {
  return Math.max(0, Math.floor((terminalWidth - 4) / 2) - 1)
}

/** The fewest columns worth giving a path: fewer and it is left out. */
const MIN_PATH_COLUMNS = 6

/** A path cut to `max` columns from the left, so its end stays. */
function fitPath(path: string, max: number): string {
  return path.length <= max ? path : `…${path.slice(-(max - 1))}`
}

/** The last part of a path: the file's own name. */
function fileNameOf(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path
}

/**
 * The writing line fitted to `width` columns. The parts that show progress
 * (the count, the spinner and how long the stream has been quiet) always
 * fit; the path gets what is left. It is cut from the left so the file name
 * stays whole, shown as the file name alone when only that fits, and left
 * out when too little room is left even for part of the name. A narrow line
 * says the silence in fewer words before the file name is cut.
 */
export function fitWritingLine(
  width: number,
  parts: {
    verb: string
    target: string | null
    detail: string | null
    silence: string | null
    shortSilence?: string | null
  },
): { target: string | null; silence: string | null } {
  const { verb, target, detail } = parts
  // `● ` + verb + ` +91` + ` ⠹` + ` · no new tokens for 37s`
  const fixed = (silence: string | null) =>
    2 + verb.length + (detail ? 1 + detail.length : 0) + 2 + (silence ? 3 + silence.length : 0)
  const silences = [parts.silence, parts.shortSilence ?? parts.silence]
  // Columns left for ` <path>` with each wording of the silence.
  const rooms = silences.map((silence) => ({ silence, room: width - fixed(silence) - 1 }))
  if (!target) {
    const fits = rooms.find(({ room }) => room >= -1)
    return { target: null, silence: (fits ?? rooms[1]!).silence }
  }
  const name = fileNameOf(target)
  // Best first: the whole path; the path cut short with its file name whole;
  // the file name alone; part of the file name.
  const choices: Array<(room: number) => string | null> = [
    (room) => (room >= target.length ? target : null),
    (room) => (room >= name.length + 1 ? fitPath(target, room) : null),
    (room) => (room >= name.length ? name : null),
  ]
  for (const choose of choices) {
    for (const { silence, room } of rooms) {
      const shown = choose(room)
      if (shown) return { target: shown, silence }
    }
  }
  const { silence, room } = rooms[1]!
  return { target: room >= MIN_PATH_COLUMNS ? fitPath(name, room) : null, silence }
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
  const { terminalWidth } = useTerminalDimensions()
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
    // it is still going, and a quiet stream is named after it. The path takes
    // the columns the rest leaves, so the count and the silence never fall off
    // the end; a quiet stream also turns the whole line the warning color, so
    // it never reads as a file still being written.
    const fitted = fitWritingLine(statusLineWidth(terminalWidth), {
      verb: activity.verb,
      target: activity.target,
      detail: activity.detail,
      silence: activity.writing.silence,
      shortSilence: activity.writing.shortSilence,
    })
    const writingHead = fitted.target ? `${activity.verb} ${fitted.target}` : activity.verb
    return (
      <>
        <span fg={activity.stalled ? theme.warning : theme.success}>{GLYPHS.activity}</span>
        <span> </span>
        {activity.stalled ? (
          <span fg={theme.warning}>{writingHead}</span>
        ) : (
          <ShimmerText
            text={writingHead}
            interval={SHIMMER_INTERVAL_MS}
            primaryColor={theme.secondary}
          />
        )}
        {activity.detail ? (
          <span fg={activity.stalled ? theme.warning : theme.success}>{` ${activity.detail}`}</span>
        ) : null}
        <span> </span>
        <Spinner color={color} />
        {fitted.silence ? <span fg={theme.warning}>{` · ${fitted.silence}`}</span> : null}
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
