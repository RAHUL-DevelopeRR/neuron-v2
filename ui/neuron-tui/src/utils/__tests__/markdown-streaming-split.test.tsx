import { describe, expect, test } from 'bun:test'
import React from 'react'

import {
  hasIncompleteCodeFence,
  renderMarkdownHead,
  renderMarkdownTail,
  renderStreamingMarkdown,
  streamingMarkdownSplit,
} from '../markdown-renderer'

type El = React.ReactElement<Record<string, unknown>>

/**
 * What a terminal draws from rendered nodes: the text with each span's
 * colours and attributes, keys and fragments left out.
 */
function drawn(node: React.ReactNode): string {
  if (node === null || node === undefined || typeof node === 'boolean') return ''
  if (typeof node === 'string' || typeof node === 'number') return String(node)
  if (Array.isArray(node)) return node.map(drawn).join('')
  if (React.isValidElement(node)) {
    const { children, ...props } = (node as El).props
    const inner = drawn(children as React.ReactNode)
    if (node.type === React.Fragment) return inner
    const style = Object.entries(props)
      .filter(([, value]) => value !== undefined)
      .map(([key, value]) => `${key}=${JSON.stringify(value)}`)
      .join(' ')
    return `<${String(node.type)} ${style}>${inner}</>`
  }
  return ''
}

/** A streaming message drawn the new way: finished blocks apart from the rest. */
function drawnSplit(content: string): string | null {
  const split = streamingMarkdownSplit(content)
  if (split <= 0) return null
  return (
    drawn(renderMarkdownHead(content.slice(0, split))) +
    drawn(renderMarkdownTail(content.slice(split), hasIncompleteCodeFence(content)))
  )
}

const SAMPLE = [
  '# Cart page',
  '',
  'The **cart** keeps its state in one store, and `useCart()` reads it.',
  'A second line of the same paragraph.',
  '',
  '- one item',
  '- two items',
  '',
  '  still the second item',
  '',
  'After the list, a paragraph with a path: C:\\Users\\me\\.neuron\\config and ~\\.neuron.',
  '',
  '1. first',
  '2. second',
  '',
  '> a quote',
  '> on two lines',
  '',
  'Then code:',
  '',
  '```ts',
  'const total = items.reduce((sum, item) => sum + item.price, 0)',
  '',
  'export default total',
  '```',
  '',
  '| a | b |',
  '|---|---|',
  '| 1 | 2 |',
  '',
  '---',
  '',
  'Inline ``` in a sentence, and *emphasis* with _underscores_.',
  '',
  '    indented code',
  '',
  'Last paragraph, still streaming',
].join('\n')

describe('a streaming message drawn in two parts', () => {
  test('draws every prefix exactly as the whole text would', () => {
    let splits = 0
    for (let end = 1; end <= SAMPLE.length; end++) {
      const content = SAMPLE.slice(0, end)
      const split = drawnSplit(content)
      if (split === null) continue
      splits++
      expect({ end, drawn: split }).toEqual({ end, drawn: drawn(renderStreamingMarkdown(content)) })
    }
    // It did cut: most prefixes have finished blocks.
    expect(splits).toBeGreaterThan(SAMPLE.length / 2)
    // Hundreds of prefixes drawn twice each: past bun's 5 s on a busy machine.
  }, 60_000)

  test('cuts only where a plain block starts after a blank line', () => {
    expect(streamingMarkdownSplit('one\n\ntwo\n')).toBe(5)
    // The line is still streaming: it may become a list item.
    expect(streamingMarkdownSplit('one\n\ntw')).toBe(-1)
    // Lists, quotes and indented lines can belong to what is above them.
    expect(streamingMarkdownSplit('one\n\n- two\n')).toBe(-1)
    expect(streamingMarkdownSplit('one\n\n> two\n')).toBe(-1)
    expect(streamingMarkdownSplit('- one\n\n  two\n')).toBe(-1)
    expect(streamingMarkdownSplit('one\n\n3. two\n')).toBe(-1)
    // Not inside a fence, nor right at a fence still open.
    expect(streamingMarkdownSplit('```\na\n\nb\n')).toBe(-1)
    expect(streamingMarkdownSplit('one\n\n```ts\nconst a = 1\n')).toBe(-1)
    expect(streamingMarkdownSplit('one\n\ntwo\n```ts\nconst a = 1\n')).toBe(5)
    // HTML blocks that blank lines do not end.
    expect(streamingMarkdownSplit('<!-- note\n\nstill note\n')).toBe(-1)
  })
})
