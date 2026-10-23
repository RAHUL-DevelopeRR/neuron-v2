/** The Markdown parser behind the transcript, and the sessions list grouping. */
import { describe, expect, test } from 'bun:test'

import { parseInline, parseMarkdown } from '../src/markdown/parse'
import type { Presence } from '../src/relay/contract'
import type { HostView } from '../src/relay/phone'
import { ago, groupSessions } from '../src/store/sessions'

describe('parseMarkdown', () => {
  test('reads the blocks an agent writes', () => {
    const blocks = parseMarkdown(
      ['# Plan', '', '1. Read it', '2. Fix **it**', '', '> careful', '> here', '', '```ts', 'const a = 1', '```', 'Done, see', '[docs](https://zero-x.live/docs).'].join('\n'),
    )
    expect(blocks).toEqual([
      { kind: 'heading', level: 1, inlines: [{ kind: 'text', text: 'Plan' }] },
      { kind: 'list', ordered: true, items: [[{ kind: 'text', text: 'Read it' }], [{ kind: 'text', text: 'Fix ' }, { kind: 'strong', text: 'it' }]] },
      { kind: 'quote', inlines: [{ kind: 'text', text: 'careful here' }] },
      { kind: 'code', lang: 'ts', text: 'const a = 1' },
      { kind: 'paragraph', lines: [[{ kind: 'text', text: 'Done, see' }], [{ kind: 'link', text: 'docs', url: 'https://zero-x.live/docs' }, { kind: 'text', text: '.' }]] },
    ])
  })

  test('a fence still streaming is code to the end', () => {
    expect(parseMarkdown('Look:\n```\nline one\nline tw')).toEqual([
      { kind: 'paragraph', lines: [[{ kind: 'text', text: 'Look:' }]] },
      { kind: 'code', lang: null, text: 'line one\nline tw' },
    ])
  })

  test('only http(s) links become links', () => {
    expect(parseInline('[x](javascript:alert(1)) and [y](https://a.b)')).toEqual([
      { kind: 'text', text: '[x](javascript:alert(1)) and ' },
      { kind: 'link', text: 'y', url: 'https://a.b' },
    ])
    expect(parseInline('`a` *b* **c**')).toEqual([
      { kind: 'code', text: 'a' },
      { kind: 'text', text: ' ' },
      { kind: 'em', text: 'b' },
      { kind: 'text', text: ' ' },
      { kind: 'strong', text: 'c' },
    ])
  })

  test('lines that only look like block starts do not stall it', () => {
    expect(parseMarkdown('  # not a heading\nnext')).toEqual([
      { kind: 'paragraph', lines: [[{ kind: 'text', text: '  # not a heading' }], [{ kind: 'text', text: 'next' }]] },
    ])
    expect(parseMarkdown('')).toEqual([])
  })
})

describe('groupSessions', () => {
  const presence = (project: string, status: Presence['status'], last_seq: number, updated_at_ms: number, title: string | null = null): Presence => ({
    v: 1,
    project,
    title,
    branch: 'main',
    model: null,
    status,
    last_seq,
    updated_at_ms,
  })
  const hosts: HostView[] = [
    {
      hostId: 'host-aaaaaaaa',
      name: 'Laptop',
      online: true,
      shares: [
        { hostId: 'host-aaaaaaaa', shareId: 'share-1111111', presence: presence('api', 'idle', 10, 100, 'Fix the build'), at: 1 },
        { hostId: 'host-aaaaaaaa', shareId: 'share-2222222', presence: presence('web', 'running', 3, 50), at: 1 },
        { hostId: 'host-aaaaaaaa', shareId: 'share-3333333', presence: null, at: 1 },
      ],
    },
    { hostId: 'host-bbbbbbbb', name: 'Desktop', online: true, shares: [{ hostId: 'host-bbbbbbbb', shareId: 'share-4444444', presence: presence('api', 'needs_me', 7, 10), at: 1 }] },
    { hostId: 'host-cccccccc', name: 'Old box', online: false, shares: [] },
  ]

  test('groups by project across computers, what needs the user first', () => {
    const groups = groupSessions(hosts, { 'host-aaaaaaaa/share-1111111': 10 })
    expect(groups.map((group) => group.project)).toEqual(['api', 'web'])
    expect(groups[0].rows.map((row) => [row.computer, row.badge, row.title])).toEqual([
      ['Desktop', 'needs_me', 'New session'],
      ['Laptop', null, 'Fix the build'],
    ])
    expect(groups[1].rows[0].badge).toBe('running')
  })

  test('idle sessions with news since the phone looked are unread', () => {
    expect(groupSessions(hosts, { 'host-aaaaaaaa/share-1111111': 9 })[0].rows[1].badge).toBe('unread')
    expect(groupSessions(hosts, {})[0].rows[1].badge).toBe('unread')
  })

  test('ago', () => {
    expect(ago(1_000, 10_000)).toBe('just now')
    expect(ago(0, 5 * 60_000)).toBe('5 min ago')
    expect(ago(0, 3 * 3600_000)).toBe('3 h ago')
  })
})
