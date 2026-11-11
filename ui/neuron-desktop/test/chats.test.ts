import { describe, expect, test } from 'bun:test'

import { PIN_LIMIT, isPinned, nextUnread, splitPinned, togglePin, triage, type ChatRow } from '../src/chats'
import { rankCommands, type PaletteCommand } from '../src/palette'
import { SHORTCUTS, keysOf, shortcutFor } from '../src/shortcuts'

const row = (id: string, extra: Partial<ChatRow> = {}): ChatRow => ({
  id,
  title: id,
  branch: null,
  worktree: false,
  activity: 'idle',
  unread: false,
  projectName: 'app',
  needsSince: null,
  stopNote: null,
  ...extra,
})

describe('nextUnread', () => {
  test('a turn that ends out of view is unread', () => {
    const next = nextUnread(new Set(), 'a', 'running', 'idle', false)
    expect([...next]).toEqual(['a'])
  })
  test('a turn that ends in view is not', () => {
    const prev = new Set<string>()
    expect(nextUnread(prev, 'a', 'running', 'idle', true)).toBe(prev)
  })
  test('other changes out of view leave it alone', () => {
    const prev = new Set<string>()
    for (const [from, to] of [
      ['idle', 'running'],
      ['running', 'needs_you'],
      ['needs_you', 'running'],
      ['running', 'stopped'],
      [null, 'idle'],
    ] as const) {
      expect(nextUnread(prev, 'a', from, to, false)).toBe(prev)
    }
  })
  test('looking at the chat clears it', () => {
    const next = nextUnread(new Set(['a', 'b']), 'a', 'idle', 'idle', true)
    expect([...next]).toEqual(['b'])
  })
  test('is pure: the old set is not changed', () => {
    const prev = new Set(['a'])
    nextUnread(prev, 'b', 'running', 'idle', false)
    nextUnread(prev, 'a', 'idle', 'idle', true)
    expect([...prev]).toEqual(['a'])
  })
})

describe('triage', () => {
  test('puts every chat in one group, the one waiting longest first', () => {
    const groups = triage([
      row('idle'),
      row('wait-new', { activity: 'needs_you', needsSince: 200 }),
      row('run', { activity: 'running' }),
      row('done', { unread: true }),
      row('wait-old', { activity: 'needs_you', needsSince: 100 }),
      row('stopped', { activity: 'stopped' }),
      row('stopped-unread', { activity: 'stopped', unread: true }),
      row('run-unread', { activity: 'running', unread: true }),
    ])
    expect(groups.needsYou.map((entry) => entry.id)).toEqual(['wait-old', 'wait-new'])
    expect(groups.running.map((entry) => entry.id)).toEqual(['run', 'run-unread'])
    expect(groups.doneUnread.map((entry) => entry.id)).toEqual(['done', 'stopped-unread'])
    expect(groups.idle.map((entry) => entry.id)).toEqual(['idle', 'stopped'])
  })
  test('empty in, empty out', () => {
    expect(triage([])).toEqual({ needsYou: [], running: [], doneUnread: [], idle: [] })
  })
})

describe('a failed turn', () => {
  test('needs the user while idle, after the chats that wait on a request', () => {
    const groups = triage([
      row('failed', { failed: true }),
      row('wait', { activity: 'needs_you', needsSince: 100 }),
      row('failed-running', { activity: 'running', failed: true }),
    ])
    expect(groups.needsYou.map((entry) => entry.id)).toEqual(['wait', 'failed'])
    expect(groups.running.map((entry) => entry.id)).toEqual(['failed-running'])
  })
})

describe('pins', () => {
  test('a pinned chat is listed above the groups, the rest keep their order', () => {
    const rows = [row('a'), row('b', { sessionId: 's-b' }), row('c')]
    const { pinned, rest } = splitPinned(rows, new Set(['s-b', 'c']))
    expect(pinned.map((entry) => entry.id)).toEqual(['b', 'c'])
    expect(rest.map((entry) => entry.id)).toEqual(['a'])
    expect(splitPinned(rows, new Set()).pinned).toEqual([])
  })

  test('a pin follows the session when the chat has one, else the chat', () => {
    const withSession = row('a', { sessionId: 's-a' })
    const pinned = togglePin(new Set(), withSession)
    expect([...pinned]).toEqual(['s-a'])
    expect(isPinned(withSession, pinned)).toBe(true)
    expect([...togglePin(new Set(), row('b'))]).toEqual(['b'])
    // Unpinning clears either key.
    expect([...togglePin(new Set(['a', 's-a']), withSession)]).toEqual([])
  })

  test('is pure and keeps the newest pins within the limit', () => {
    const before = new Set(['x'])
    togglePin(before, row('y'))
    expect([...before]).toEqual(['x'])
    let pins: ReadonlySet<string> = new Set()
    for (let index = 0; index < PIN_LIMIT + 5; index++) pins = togglePin(pins, row(`r${index}`))
    expect(pins.size).toBe(PIN_LIMIT)
    expect(pins.has(`r${PIN_LIMIT + 4}`)).toBe(true)
    expect(pins.has('r0')).toBe(false)
  })
})

describe('rankCommands', () => {
  const commands: PaletteCommand[] = ['Open settings', 'New chat', 'Search sessions', 'Go to the chat that needs you', 'Chat: fix login'].map((title, index) => ({
    id: String(index),
    title,
    group: index === 4 ? 'Chats' : 'Actions',
    run: () => {},
  }))
  test('no query keeps the order', () => {
    expect(rankCommands(commands, '  ').map((command) => command.title)).toEqual(commands.map((command) => command.title))
  })
  test('a start beats a word start beats a substring', () => {
    const titles = rankCommands(commands, 'chat').map((command) => command.title)
    expect(titles[0]).toBe('Chat: fix login')
    expect(titles).toContain('New chat')
    expect(titles).not.toContain('Open settings')
    expect(titles.indexOf('New chat')).toBeLessThan(titles.indexOf('Go to the chat that needs you') + 1)
  })
  test('scattered letters still find a command, words all have to fit', () => {
    expect(rankCommands(commands, 'ssn').map((command) => command.title)).toEqual(['Search sessions'])
    expect(rankCommands(commands, 'new chat').map((command) => command.title)).toEqual(['New chat'])
    expect(rankCommands(commands, 'zzz')).toEqual([])
  })
  test('matches case-insensitively and in the group name', () => {
    expect(rankCommands(commands, 'SETTINGS')[0].title).toBe('Open settings')
    expect(rankCommands(commands, 'actions').length).toBe(4)
  })
})

describe('shortcuts', () => {
  const press = (key: string, extra: Partial<KeyboardEvent> = {}) => ({ key, ctrlKey: true, metaKey: false, shiftKey: false, altKey: false, ...extra })
  test('the table is the handler', () => {
    expect(shortcutFor(press('k'))).toBe('palette')
    expect(shortcutFor(press('n'))).toBe('newChat')
    expect(shortcutFor(press('N', { shiftKey: true }))).toBe('newWorktreeChat')
    expect(shortcutFor(press('j'))).toBe('nextWaiting')
    expect(shortcutFor(press('ArrowUp', { altKey: true }))).toBe('previousChat')
    expect(shortcutFor(press('ArrowDown', { altKey: true, ctrlKey: false, metaKey: true }))).toBe('nextChat')
    expect(shortcutFor(press('/'))).toBe('shortcuts')
  })
  test('needs Ctrl or Cmd and the exact modifiers', () => {
    expect(shortcutFor(press('k', { ctrlKey: false }))).toBeNull()
    expect(shortcutFor(press('k', { altKey: true }))).toBeNull()
    expect(shortcutFor(press('ArrowUp'))).toBeNull()
  })
  test('no two shortcuts share keys', () => {
    const seen = SHORTCUTS.map((shortcut) => keysOf(shortcut))
    expect(new Set(seen).size).toBe(seen.length)
  })
  test('keys read as people write them', () => {
    const byId = Object.fromEntries(SHORTCUTS.map((shortcut) => [shortcut.id, shortcut]))
    expect(keysOf(byId.newWorktreeChat)).toBe('Ctrl+Shift+N')
    expect(keysOf(byId.nextChat)).toBe('Ctrl+Alt+↓')
    expect(keysOf(byId.newWorktreeChat, true)).toBe('⌘⇧N')
  })
})
