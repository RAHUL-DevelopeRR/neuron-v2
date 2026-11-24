/**
 * Double-click detection for the mouse: terminals report each press on its
 * own, with no click count, so two clicks on the same thing within
 * {@link DOUBLE_CLICK_MS} make a double click.
 */

/** The longest gap between the two clicks of a double click. */
export const DOUBLE_CLICK_MS = 400

export interface DoubleClickDetector {
  /** Records a click on `id`; true when it completes a double click. */
  click: (id: string) => boolean
  /** Forgets the last click, so the next one starts over. */
  reset: () => void
}

export function createDoubleClickDetector(
  windowMs: number = DOUBLE_CLICK_MS,
  now: () => number = Date.now,
): DoubleClickDetector {
  let last: { id: string; at: number } | null = null
  return {
    click(id) {
      const at = now()
      const isDouble = last !== null && last.id === id && at - last.at <= windowMs
      // A third click starts a new pair rather than opening twice.
      last = isDouble ? null : { id, at }
      return isDouble
    },
    reset() {
      last = null
    },
  }
}
