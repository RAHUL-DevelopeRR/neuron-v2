// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the corners
// come from utils/ui-constants.ts (square in a classic Windows console
// window), where the edges between segments are joints (┬ │ ┴) instead of
// corners drawn over each other. See ui/neuron-tui/PORTING.md.
import React, { useState } from 'react'
import stringWidth from 'string-width'

import { Button } from './button'
import { useTheme } from '../hooks/use-theme'
import { getGlyphMode } from '../utils/glyphs'
import { BORDER_CHARS } from '../utils/ui-constants'

import type { ChatTheme } from '../types/theme-system'
import type { BorderCharacters } from '@opentui/core'

/** One edge column of the control: its top, middle and bottom characters. */
export type SegmentEdge = readonly [top: string, middle: string, bottom: string]

/**
 * The edge drawn left and right of each segment (`null`: none). Segments left
 * of the hovered one draw their left edge, those right of it their right
 * edge, and the hovered one both, so every boundary is drawn once, by the
 * segment on the hovered side. With rounded corners (╭ ╮) Freebuff draws
 * every edge as a corner, and the segments read as tabs stacked on each
 * other. With square ones (a classic Windows console: ┌ ┐) those corners read
 * as boxes drawn over each other (┌─────┌────┌───┐), so `joinInnerEdges`
 * draws the edges between segments as joints (┌─────┬────┬───┐).
 */
export function segmentEdges(
  count: number,
  hoveredIndex: number,
  chars: BorderCharacters,
  joinInnerEdges: boolean,
): Array<{ left: SegmentEdge | null; right: SegmentEdge | null }> {
  const outerLeft: SegmentEdge = [chars.topLeft, chars.vertical, chars.bottomLeft]
  const outerRight: SegmentEdge = [chars.topRight, chars.vertical, chars.bottomRight]
  const joint: SegmentEdge = [chars.topT, chars.vertical, chars.bottomT]
  const last = count - 1
  return Array.from({ length: count }, (_, index) => ({
    left:
      index <= hoveredIndex
        ? joinInnerEdges && index > 0
          ? joint
          : outerLeft
        : null,
    right:
      index >= hoveredIndex
        ? joinInnerEdges && index < last
          ? joint
          : outerRight
        : null,
  }))
}

const EdgeColumn = ({ edge, color }: { edge: SegmentEdge; color: string }) => (
  <box style={{ flexDirection: 'column', gap: 0 }}>
    <text fg={color} selectable={false}>{edge[0]}</text>
    <text fg={color} selectable={false}>{edge[1]}</text>
    <text fg={color} selectable={false}>{edge[2]}</text>
  </box>
)

export interface Segment {
  id: string
  label: string
  isBold?: boolean
  isSelected?: boolean
  defaultHighlighted?: boolean // Highlighted when nothing else is hovered
  disabled?: boolean // Gray out and de-emphasize disabled items
}

/**
 * SegmentedControlProps
 *
 * Renders a bordered segmented toggle. Pure UI; all behavior is driven by
 * the parent via callbacks.
 */
interface SegmentedControlProps {
  segments: Segment[]
  onSegmentClick?: (id: string) => void
  onMouseOver?: () => void
  onMouseOut?: () => void
}

export const SegmentedControl = ({
  segments,
  onSegmentClick,
  onMouseOver,
  onMouseOut,
}: SegmentedControlProps) => {
  const theme = useTheme()
  const [hoveredId, setHoveredId] = useState<string | null>(null)
  const [hasHoveredSinceOpen, setHasHoveredSinceOpen] = useState(false)

  const processedSegments = processSegments(
    segments,
    hoveredId,
    hasHoveredSinceOpen,
    theme,
  )
  const hoveredIndex = hoveredId
    ? processedSegments.findIndex((s) => s.id === hoveredId)
    : processedSegments.length - 1
  const edges = segmentEdges(
    processedSegments.length,
    hoveredIndex,
    BORDER_CHARS,
    getGlyphMode() === 'console',
  )

  return (
    <box
      style={{
        flexDirection: 'row',
        gap: 0,
        backgroundColor: 'transparent',
      }}
      onMouseOver={onMouseOver}
      onMouseOut={() => {
        setHoveredId(null)
        onMouseOut && onMouseOut()
      }}
    >
      {/* Segments rendered with dynamic left/right edges based on hover */}
      {processedSegments.map((seg, idx) => {
        const { left, right } = edges[idx]!

        return (
          <React.Fragment key={seg.id}>
            {left ? <EdgeColumn edge={left} color={seg.frameColor} /> : null}

            <Button
              onClick={() => onSegmentClick && onSegmentClick(seg.id)}
              onMouseOver={() => {
                setHoveredId(seg.id)
                setHasHoveredSinceOpen(true)
              }}
              style={{
                flexDirection: 'column',
                gap: 0,
                width: seg.width,
                minWidth: seg.width,
              }}
            >
              <text fg={seg.frameColor}>{seg.topBorder}</text>
              <text fg={seg.textColor}>
                {seg.isItalic ? (
                  <i>{seg.content}</i>
                ) : seg.isBold ? (
                  <b>{seg.content}</b>
                ) : (
                  seg.content
                )}
              </text>
              <text fg={seg.frameColor}>{seg.bottomBorder}</text>
            </Button>

            {right ? <EdgeColumn edge={right} color={seg.frameColor} /> : null}
          </React.Fragment>
        )
      })}
    </box>
  )
}

export type ProcessedSegment = {
  id: string
  topBorder: string
  content: string
  bottomBorder: string
  frameColor: string
  leftBorderColor: string
  textColor: string
  isHovered: boolean
  isBold: boolean
  isItalic: boolean
  label: string
  width: number
}

/**
 * Pure function that maps input segments + UI state to render-ready
 * segment descriptors. This is exported for unit testing.
 */
export const processSegments = (
  segments: Segment[],
  hoveredId: string | null,
  hasHoveredSinceOpen: boolean,
  theme: ChatTheme,
): ProcessedSegment[] => {
  return segments.map((seg) => {
    // Normalized flags
    const isDisabled = !!seg.disabled
    const isSelected = !!seg.isSelected
    const defaultHL = !!seg.defaultHighlighted

    // Hover and highlight state
    const canHover = !isSelected || defaultHL
    const isHovered = hoveredId === seg.id && canHover
    const isDefaultHighlighted = defaultHL && !hasHoveredSinceOpen
    const isHighlighted = isHovered || isDefaultHighlighted

    // Emphasis
    const isBold = !!(seg.isBold || isHovered || (isSelected && isHighlighted))

    // Colors
    const frameColor = isHighlighted ? theme.foreground : theme.border
    const textMuted = isDisabled || (isSelected && !isHighlighted)
    const textColor = textMuted ? theme.muted : theme.foreground

    // Content + metrics
    const content = ` ${seg.label} `
    const width = stringWidth(content)
    const horizontal = '─'.repeat(width)

    // Return render-ready descriptor
    // - Computed (complex conditions): frameColor, textColor, isBold
    // - Inlined (simple): isItalic (disabled), leftBorderColor (= frameColor)
    return {
      id: seg.id,
      topBorder: horizontal,
      content,
      bottomBorder: horizontal,
      frameColor,
      leftBorderColor: frameColor,
      textColor,
      isHovered,
      isBold,
      isItalic: isDisabled,
      label: seg.label,
      width,
    }
  })
}
