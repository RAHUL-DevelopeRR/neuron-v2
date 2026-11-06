/**
 * Text measured in terminal columns, not JavaScript string length: a CJK
 * character or an emoji takes two columns, a combining mark none, and a
 * surrogate pair one character. Lines fitted by `.length` overflow their row
 * when a branch name, an email or a model name holds such characters, and the
 * overflow wraps onto the next row (overlapping text in a narrow terminal).
 *
 * Every part of a one-line bar (the prompt status bar, the old footer, the
 * MCP panel's rows) is measured and cut with these.
 *
 * The bar's own glyphs (`│ ● ○ ◑ ▾`) are East Asian "ambiguous" characters:
 * one column in most terminals, two in one set up for Chinese, Japanese or
 * Korean. There they are measured as two ({@link ambiguousIsWide}), so the
 * bar still fits its row.
 */

import stringWidth from 'string-width'

const ELLIPSIS = '…'

/**
 * Whether the terminal draws ambiguous-width characters two columns wide:
 * `NEURON_AMBIGUOUS_WIDE=1` (or `0`) says so; otherwise a Chinese, Japanese
 * or Korean locale (`LC_ALL`, `LC_CTYPE` or `LANG`, the first one set) is
 * taken to.
 */
export function ambiguousIsWide(env: Record<string, string | undefined> = process.env): boolean {
  const explicit = env.NEURON_AMBIGUOUS_WIDE?.trim()
  if (explicit === '1') return true
  if (explicit === '0') return false
  const locale = [env.LC_ALL, env.LC_CTYPE, env.LANG].find((value) => value?.trim()) ?? ''
  return /^(zh|ja|ko)([_.@-]|$)/i.test(locale.trim())
}

let ambiguousWide = ambiguousIsWide()

/** Test seam: measure ambiguous-width characters as wide (or not). */
export function setAmbiguousWideForTesting(wide: boolean | null): void {
  ambiguousWide = wide ?? ambiguousIsWide()
}

const measure = (text: string): number =>
  stringWidth(text, { ambiguousIsNarrow: !ambiguousWide })

/** The columns `text` takes in a terminal. */
export function displayWidth(text: string): number {
  return measure(text)
}

/**
 * `text` in at most `columns` columns: unchanged when it fits, else cut at a
 * character boundary and ended with "…" (which takes one column, or two where
 * ambiguous characters are wide). A wide character that would straddle the
 * edge is left out whole.
 */
export function cutToDisplayWidth(text: string, columns: number): string {
  if (columns <= 0) return ''
  if (measure(text) <= columns) return text
  const ellipsis = measure(ELLIPSIS)
  if (columns <= ellipsis) return columns === ellipsis ? ELLIPSIS : ''
  let kept = ''
  let used = 0
  for (const char of text) {
    const width = measure(char)
    if (used + width > columns - ellipsis) break
    kept += char
    used += width
  }
  return kept + ELLIPSIS
}

/** `text` padded with spaces to `columns` columns (never cut). */
export function padToDisplayWidth(text: string, columns: number): string {
  const width = measure(text)
  return width >= columns ? text : text + ' '.repeat(columns - width)
}
