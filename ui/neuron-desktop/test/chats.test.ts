import { describe, expect, test } from 'bun:test'

import { nextUnread, triage, type ChatRow } from '../src/chats'
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
