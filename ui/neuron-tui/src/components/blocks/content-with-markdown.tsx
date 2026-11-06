import { memo, useMemo } from 'react'

import {
  renderMarkdown,
  renderMarkdownHead,
  renderMarkdownTail,
  renderStreamingMarkdown,
  hasIncompleteCodeFence,
  hasMarkdown,
  streamingMarkdownSplit,
  type MarkdownPalette,
} from '../../utils/markdown-renderer'

interface ContentWithMarkdownProps {
  content: string
  isStreaming: boolean
  codeBlockWidth: number
  palette: MarkdownPalette
}

/**
 * A text block as Markdown. While it streams, its finished blocks (up to the
 * last blank line that ends one, see streamingMarkdownSplit) are parsed once
 * and kept; each update parses only what came after them, instead of the
 * whole text every time. The result is what the whole text would draw.
 */
export const ContentWithMarkdown = memo(
  ({
    content,
    isStreaming,
    codeBlockWidth,
    palette,
  }: ContentWithMarkdownProps) => {
    const markdown = hasMarkdown(content)
    const split = markdown && isStreaming ? streamingMarkdownSplit(content) : -1
    const head = split > 0 ? content.slice(0, split) : ''
    const headNode = useMemo(
      () => (head ? renderMarkdownHead(head, { codeBlockWidth, palette }) : null),
      [head, codeBlockWidth, palette],
    )
    if (!markdown) {
      return content
    }
    const options = { codeBlockWidth, palette }
    if (headNode !== null) {
      const tail = renderMarkdownTail(content.slice(split), hasIncompleteCodeFence(content), options)
      return (
        <>
          {headNode}
          {tail}
        </>
      )
    }
    if (isStreaming) {
      return renderStreamingMarkdown(content, options)
    }
    return renderMarkdown(content, options)
  },
)
