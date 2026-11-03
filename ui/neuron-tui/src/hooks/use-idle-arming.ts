/**
 * For a panel that takes the keyboard and acts on Enter (commit, rewind): it
 * is armed only after `ms` with no keypress at all, counted again from every
 * key and from each change of `since` (the panel, its preview). Keys typed
 * for something else, a held Enter or the Enter that opened it come in a
 * stream, so they never find it armed.
 */

import { useCallback, useEffect, useRef } from 'react'

export interface IdleArming {
  /** No key for `ms`: the panel may act on its keys. */
  isArmed: () => boolean
  /** Call on every key the panel sees, after deciding what it does. */
  noteKey: () => void
}

export function useIdleArming(ms: number, since: unknown): IdleArming {
  const armed = useRef(false)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)

  const restart = useCallback(() => {
    armed.current = false
    if (timer.current) clearTimeout(timer.current)
    timer.current = setTimeout(() => {
      armed.current = true
    }, ms)
  }, [ms])

  useEffect(() => {
    restart()
    return () => {
      if (timer.current) clearTimeout(timer.current)
    }
  }, [restart, since])

  return { isArmed: useCallback(() => armed.current, []), noteKey: restart }
}

/** The text a key types, when it is a plain printable character. */
export function typedText(key: {
  sequence?: string
  ctrl?: boolean
  meta?: boolean
  option?: boolean
}): string | null {
  if (key.ctrl || key.meta || key.option) return null
  const sequence = key.sequence ?? ''
  if ([...sequence].length !== 1) return null
  const code = sequence.codePointAt(0)!
  return code >= 0x20 && code !== 0x7f ? sequence : null
}
