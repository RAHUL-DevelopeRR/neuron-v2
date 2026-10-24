import { describe, expect, test } from 'bun:test'

import { segmentEdges } from '../../components/segmented-control'
import { bordersFor } from '../../utils/glyphs'

/** The control's top row: each segment's edges around a run of ─. */
function topRow(count: number, hovered: number, mode: 'unicode' | 'console'): string {
  const chars = bordersFor(mode).rounded
  return segmentEdges(count, hovered, chars, mode === 'console')
    .map(({ left, right }) => `${left?.[0] ?? ''}──${right?.[0] ?? ''}`)
    .join('')
}

describe('the mode selector edges', () => {
  test('in a classic console the edges between segments are joints, not boxes drawn over each other', () => {
    // Was ┌──┌──┌──┌──┌──┐ (t-customer-journey-verify/v1.txt) with the last
    // segment hovered, and corners on both sides of a hovered middle one.
    expect(topRow(5, 4, 'console')).toBe('┌──┬──┬──┬──┬──┐')
    expect(topRow(5, 2, 'console')).toBe('┌──┬──┬──┬──┬──┐')
    expect(topRow(5, 0, 'console')).toBe('┌──┬──┬──┬──┬──┐')
  })

  test('every boundary is drawn exactly once', () => {
    for (let hovered = 0; hovered < 4; hovered++) {
      const edges = segmentEdges(4, hovered, bordersFor('console').rounded, true)
      const drawn = edges.reduce((sum, { left, right }) => sum + (left ? 1 : 0) + (right ? 1 : 0), 0)
      expect(drawn).toBe(5)
    }
  })

  test('with rounded corners the segments keep their stacked-tab look', () => {
    expect(topRow(3, 2, 'unicode')).toBe('╭──╭──╭──╮')
    expect(topRow(3, 0, 'unicode')).toBe('╭──╮──╮──╮')
  })
})
