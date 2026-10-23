/**
 * The Markdown an agent writes, as blocks a renderer draws: the same subset
 * and rules as the desktop's renderer (ui/neuron-desktop/src/components/Markdown.tsx)
 * so both show a message the same way. Fenced code, headings, lists, quotes
 * and paragraphs; inline code, bold, italics and links. Links are kept only
 * when they are http(s). A fence still streaming (no closing line yet) is code
 * to the end.
 *
 * It is pure and platform-free: the desktop could import it from
 * @neuron/client once it moves there (a WP3 follow-up).
 */

export type Inline =
  | { kind: 'text'; text: string }
  | { kind: 'code'; text: string }
  | { kind: 'strong'; text: string }
  | { kind: 'em'; text: string }
  | { kind: 'link'; text: string; url: string }

export type Block =
  | { kind: 'code'; lang: string | null; text: string }
  | { kind: 'heading'; level: number; inlines: Inline[] }
  | { kind: 'list'; ordered: boolean; items: Inline[][] }
  | { kind: 'quote'; inlines: Inline[] }
  | { kind: 'paragraph'; lines: Inline[][] }

const INLINE = /(`[^`]+`)|(\*\*[^*]+\*\*)|(\*[^*\s][^*]*\*)|(\[[^\]]+\]\((https?:\/\/[^)\s]+)\))/g
const LIST_ITEM = /^\s*([-*+]|\d+[.)])\s+/
const BLOCK_START = /^\s*(```|~~~|#{1,6}\s|[-*+]\s|\d+[.)]\s|>)/

export function parseInline(text: string): Inline[] {
  const out: Inline[] = []
  let last = 0
  for (const match of text.matchAll(INLINE)) {
    const index = match.index ?? 0
    if (index > last) out.push({ kind: 'text', text: text.slice(last, index) })
    const [token] = match
    if (match[1]) out.push({ kind: 'code', text: token.slice(1, -1) })
    else if (match[2]) out.push({ kind: 'strong', text: token.slice(2, -2) })
    else if (match[3]) out.push({ kind: 'em', text: token.slice(1, -1) })
    else if (match[4]) out.push({ kind: 'link', text: token.slice(1, token.indexOf(']')), url: match[5] })
    last = index + token.length
  }
  if (last < text.length) out.push({ kind: 'text', text: text.slice(last) })
  return out
}

export function parseMarkdown(text: string): Block[] {
  const lines = text.replace(/\r\n/g, '\n').split('\n')
  const out: Block[] = []
  let index = 0
  while (index < lines.length) {
    const line = lines[index]
    const fence = /^\s*(```|~~~)\s*([\w+-]*)/.exec(line)
    if (fence) {
      const body: string[] = []
      index++
      while (index < lines.length && !lines[index].trimStart().startsWith(fence[1])) body.push(lines[index++])
      index++
      out.push({ kind: 'code', lang: fence[2] || null, text: body.join('\n') })
      continue
    }
    const heading = /^(#{1,6})\s+(.*)$/.exec(line)
    if (heading) {
      out.push({ kind: 'heading', level: heading[1].length, inlines: parseInline(heading[2]) })
      index++
      continue
    }
    if (LIST_ITEM.test(line)) {
      const ordered = /^\s*\d/.test(line)
      const items: Inline[][] = []
      while (index < lines.length && LIST_ITEM.test(lines[index])) items.push(parseInline(lines[index++].replace(LIST_ITEM, '')))
      out.push({ kind: 'list', ordered, items })
      continue
    }
    if (/^\s*>/.test(line)) {
      const quote: string[] = []
      while (index < lines.length && /^\s*>/.test(lines[index])) quote.push(lines[index++].replace(/^\s*>\s?/, ''))
      out.push({ kind: 'quote', inlines: parseInline(quote.join(' ')) })
      continue
    }
    if (!line.trim()) {
      index++
      continue
    }
    // The first line always belongs here (an indented `  # x` looks like a block start but is not one).
    const paragraph: string[] = [lines[index++]]
    while (index < lines.length && lines[index].trim() && !BLOCK_START.test(lines[index])) paragraph.push(lines[index++])
    out.push({ kind: 'paragraph', lines: paragraph.map(parseInline) })
  }
  return out
}
