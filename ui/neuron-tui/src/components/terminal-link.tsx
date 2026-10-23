// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: a link can
// keep its https:// and be an OSC 8 hyperlink (`href`), which terminals that
// support it open on click. See ui/neuron-tui/PORTING.md.
import React, { useCallback, useMemo, useState } from 'react'

import { Button } from './button'
import { useTheme } from '../hooks/use-theme'

type FormatLinesFn = (text: string, maxWidth?: number) => string[]

export interface TerminalLinkProps {
  text: string
  maxWidth?: number
  formatLines?: FormatLinesFn
  color?: string
  activeColor?: string
  underlineOnHover?: boolean
  isActive?: boolean
  onActivate?: () => void | Promise<any>
  containerStyle?: Record<string, unknown>
  lineWrap?: boolean
  inline?: boolean
  /** Show the scheme (`https://`); it is dropped by default. */
  keepScheme?: boolean
  /**
   * Make the text an OSC 8 hyperlink to this URL. OpenTUI writes it only to
   * terminals that support hyperlinks.
   */
  href?: string
}

const defaultFormatLines: FormatLinesFn = (text) => [text]
const PREFIXES_TO_STRIP = ['https://', 'http://']

export const TerminalLink: React.FC<TerminalLinkProps> = ({
  text,
  maxWidth,
  formatLines = defaultFormatLines,
  color,
  activeColor,
  underlineOnHover = true,
  isActive = false,
  onActivate,
  containerStyle,
  lineWrap = false,
  inline = false,
  keepScheme = false,
  href,
}) => {
  const theme = useTheme()

  // Use theme colors as defaults if not provided
  const linkColor = color ?? theme.link
  const linkActiveColor = activeColor ?? theme.success
  const [isHovered, setIsHovered] = useState(false)

  const displayLines = useMemo(() => {
    let displayText = text.trim()
    for (const prefix of keepScheme ? [] : PREFIXES_TO_STRIP) {
      if (displayText.startsWith(prefix)) {
        displayText = displayText.slice(prefix.length)
      }
    }

    const formatted = formatLines(displayText, maxWidth)
    if (formatted.length <= 1) {
      return formatted
    }
    return formatted.filter((line) => line.trim().length > 0)
  }, [formatLines, keepScheme, maxWidth, text])

  const displayColor = isActive ? linkActiveColor : linkColor
  const shouldUnderline = underlineOnHover && isHovered

  const handleActivate = useCallback(() => {
    if (onActivate) {
      onActivate()
    }
  }, [onActivate])

  // For inline mode, render without hover/click support (spans don't support mouse events)
  if (inline) {
    return <span fg={displayColor}>{text}</span>
  }

  return (
    <Button
      style={{
        flexDirection: 'column',
        alignItems: 'flex-start',
        width: '100%',
        gap: 0,
        ...containerStyle,
      }}
      onMouseOver={() => setIsHovered(true)}
      onMouseOut={() => setIsHovered(false)}
      onClick={handleActivate}
    >
      {displayLines.map((line: string, index: number) => {
        const coloredText = <span fg={displayColor}>{line}</span>
        const styled = shouldUnderline ? <u>{coloredText}</u> : coloredText
        return (
          <text key={index} style={{ wrapMode: lineWrap ? 'word' : 'none' }}>
            {href ? <a href={href}>{styled}</a> : styled}
          </text>
        )
      })}
    </Button>
  )
}
