/**
 * Text measured in terminal columns, not JavaScript string length: a CJK
 * character or an emoji takes two columns, a combining mark none, and a
 * surrogate pair one character. Lines fitted by `.length` overflow their row
 * when a branch name, an email or a model name holds such characters, and the
 * overflow wraps onto the next row (overlapping text in a narrow terminal).
 *
 * Every part of a one-line bar (the prompt status bar, the old footer, the
 * MCP panel's rows) is measured and cut with these.
 */

import stringWidth from 'string-width'

const ELLIPSIS = '…'

/** The columns `text` takes in a terminal. */
export function displayWidth(text: string): number {
  return stringWidth(text)
}

/**
 * `text` in at most `columns` columns: unchanged when it fits, else cut at a
 * character boundary and ended with "…" (which takes one column). A wide
 * character that would straddle the edge is left out whole.
 */
export function cutToDisplayWidth(text: string, columns: number): string {
  if (columns <= 0) return ''
  if (stringWidth(text) <= columns) return text
  if (columns === 1) return ELLIPSIS
  let kept = ''
  let used = 0
  for (const char of text) {
    const width = stringWidth(char)
    if (used + width > columns - 1) break
    kept += char
    used += width
  }
  return kept + ELLIPSIS
}

/** `text` padded with spaces to `columns` columns (never cut). */
export function padToDisplayWidth(text: string, columns: number): string {
  const width = stringWidth(text)
  return width >= columns ? text : text + ' '.repeat(columns - width)
}
