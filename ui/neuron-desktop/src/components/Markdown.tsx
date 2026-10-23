import { Fragment, useState, type ReactNode } from 'react'

/** Copies `text`, and says so for a moment. */
export function CopyButton({ text, label = 'Copy' }: { text: string; label?: string }) {
  const [copied, setCopied] = useState(false)
  return (
    <button
      className="copy-button"
      aria-label={label}
      title={label}
      onClick={() => {
        void navigator.clipboard?.writeText(text).then(
          () => {
            setCopied(true)
            setTimeout(() => setCopied(false), 1200)
          },
          () => {},
        )
      }}
    >
      {copied ? 'Copied' : 'Copy'}
    </button>
  )
}

/**
 * The Markdown an agent writes: fenced code, headings, lists, quotes,
 * paragraphs, and inline code, bold, italics and links. Everything is text
 * nodes (React escapes it); links only open http(s).
 */
export function Markdown({ text, onLink }: { text: string; onLink?: (url: string) => void }) {
  return <div className="markdown">{blocks(text, onLink)}</div>
}

function blocks(text: string, onLink?: (url: string) => void): ReactNode[] {
  const lines = text.replace(/\r\n/g, '\n').split('\n')
  const out: ReactNode[] = []
  let index = 0
  let key = 0
  while (index < lines.length) {
    const line = lines[index]
    const fence = /^\s*(```|~~~)\s*([\w+-]*)/.exec(line)
    if (fence) {
      const body: string[] = []
      index++
      while (index < lines.length && !lines[index].trimStart().startsWith(fence[1])) body.push(lines[index++])
      index++
      out.push(
        <div key={key++} className="code-wrap">
          <div className="code-head">
            <span>{fence[2] || 'code'}</span>
            <CopyButton text={body.join('\n')} label="Copy code" />
          </div>
          <pre className="code-block" data-lang={fence[2] || undefined}>
            <code>{body.join('\n')}</code>
          </pre>
        </div>,
      )
      continue
    }
    const heading = /^(#{1,6})\s+(.*)$/.exec(line)
    if (heading) {
      const level = Math.min(heading[1].length + 2, 6)
      const Tag = `h${level}` as 'h3'
      out.push(<Tag key={key++}>{inline(heading[2], onLink)}</Tag>)
      index++
      continue
    }
    if (/^\s*([-*+]|\d+[.)])\s+/.test(line)) {
      const ordered = /^\s*\d/.test(line)
      const items: string[] = []
      while (index < lines.length && /^\s*([-*+]|\d+[.)])\s+/.test(lines[index])) {
        items.push(lines[index].replace(/^\s*([-*+]|\d+[.)])\s+/, ''))
        index++
      }
      const List = ordered ? 'ol' : 'ul'
      out.push(
        <List key={key++}>
          {items.map((item, i) => (
            <li key={i}>{inline(item, onLink)}</li>
          ))}
        </List>,
      )
      continue
    }
    if (/^\s*>/.test(line)) {
      const quote: string[] = []
      while (index < lines.length && /^\s*>/.test(lines[index])) quote.push(lines[index++].replace(/^\s*>\s?/, ''))
      out.push(<blockquote key={key++}>{inline(quote.join(' '), onLink)}</blockquote>)
      continue
    }
    if (!line.trim()) {
      index++
      continue
    }
    const paragraph: string[] = []
    while (index < lines.length && lines[index].trim() && !/^\s*(```|~~~|#{1,6}\s|[-*+]\s|\d+[.)]\s|>)/.test(lines[index])) {
      paragraph.push(lines[index++])
    }
    out.push(
      <p key={key++}>
        {paragraph.map((part, i) => (
          <Fragment key={i}>
            {i > 0 && <br />}
            {inline(part, onLink)}
          </Fragment>
        ))}
      </p>,
    )
  }
  return out
}

const INLINE = /(`[^`]+`)|(\*\*[^*]+\*\*)|(\*[^*\s][^*]*\*)|(\[[^\]]+\]\((https?:\/\/[^)\s]+)\))/g

function inline(text: string, onLink?: (url: string) => void): ReactNode[] {
  const out: ReactNode[] = []
  let last = 0
  let key = 0
  for (const match of text.matchAll(INLINE)) {
    if (match.index! > last) out.push(text.slice(last, match.index))
    const [token] = match
    if (match[1]) out.push(<code key={key++}>{token.slice(1, -1)}</code>)
    else if (match[2]) out.push(<strong key={key++}>{token.slice(2, -2)}</strong>)
    else if (match[3]) out.push(<em key={key++}>{token.slice(1, -1)}</em>)
    else if (match[4]) {
      const label = token.slice(1, token.indexOf(']'))
      const url = match[5]
      out.push(
        <a
          key={key++}
          href={url}
          onClick={(event) => {
            event.preventDefault()
            onLink?.(url)
          }}
        >
          {label}
        </a>,
      )
    }
    last = match.index! + token.length
  }
  if (last < text.length) out.push(text.slice(last))
  return out
}
