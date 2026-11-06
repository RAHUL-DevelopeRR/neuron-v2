import { memo, useLayoutEffect, useRef } from 'react'

import type { LiveOutput as Live } from '@neuron/client'

// CSI sequences (colours, cursor moves) and OSC sequences (window titles, links).
// eslint-disable-next-line no-control-regex
const ANSI = /\u001b\[[0-?]*[ -/]*[@-~]|\u001b\][^\u0007\u001b]*(?:\u0007|\u001b\\)|\u001b[@-Z\\-_]/g

/** The text without terminal escape sequences. */
export const stripAnsi = (text: string): string => text.replace(ANSI, '')

/** How near the bottom still counts as "following" the output. */
const FOLLOW_SLACK = 24

/**
 * What a running shell command printed so far: chunks in order, stderr tinted,
 * in a bounded block that follows new output until the user scrolls up.
 */
export const LiveOutput = memo(function LiveOutput({ live }: { live: Live | null | undefined }) {
  const box = useRef<HTMLDivElement>(null)
  const following = useRef(true)
  useLayoutEffect(() => {
    const el = box.current
    if (el && following.current) el.scrollTop = el.scrollHeight
  }, [live])
  if (!live) return null
  return (
    <div
      ref={box}
      className="live-output"
      role="region"
      aria-label="Live output"
      aria-live="off"
      tabIndex={0}
      onScroll={(event) => {
        const el = event.currentTarget
        following.current = el.scrollHeight - el.scrollTop - el.clientHeight <= FOLLOW_SLACK
      }}
    >
      {live.dropped > 0 && <div className="live-dropped">… {live.dropped} earlier characters not shown</div>}
      <pre>
        {live.chunks.map((chunk, index) => (
          <span key={index} className={`live-chunk ${chunk.stream}`}>
            {stripAnsi(chunk.text)}
          </span>
        ))}
      </pre>
    </div>
  )
})
