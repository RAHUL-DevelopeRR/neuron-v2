import { describe, expect, test } from 'bun:test'

import type { SessionSummary } from '@neuron/client'

import { groupRows, noteSeen, projectOf, sessionRows } from '../src/sessions'

const open = { name: 'app', path: '/work/app' }
const summary = (id: string, updated: number, extra: Record<string, unknown> = {}): SessionSummary =>
  ({ id, title: id, model: null, message_count: 2, updated_at_ms: updated, current: false, branch: null, ...extra }) as SessionSummary

describe('the session list model', () => {
  test("a session's project: a path, an object, or the open one", () => {
    expect(projectOf(summary('a', 1, { project: 'C:\\work\\site\\' }), open)).toEqual({ name: 'site', path: 'C:\\work\\site\\' })
    expect(projectOf(summary('b', 1, { project: { path: '/work/lib', name: 'Library' } }), open)).toEqual({ name: 'Library', path: '/work/lib' })
    expect(projectOf(summary('c', 1), open)).toBe(open)
  })

  test('one badge per row: running, then the terminal, then failed, then unread', () => {
    const rows = sessionRows(
      [summary('current', 50), summary('elsewhere', 40, { run_status: 'running' }), summary('broken', 30), summary('news', 20), summary('quiet', 10)],
      { open, currentId: 'current', busy: true, seen: { seen: { news: 5, quiet: 10 }, failed: { broken: true, elsewhere: true } } },
    )
    expect(rows.map((row) => [row.id, row.badge])).toEqual([
      ['current', 'running'],
      ['elsewhere', 'terminal'],
      ['broken', 'failed'],
      ['news', 'unread'],
      ['quiet', null],
    ])
  })

  test('empty sessions are hidden, except the open one', () => {
    const rows = sessionRows([summary('fresh', 1, { message_count: 0 }), summary('empty', 1, { message_count: 0 })], {
      open,
      currentId: 'fresh',
      busy: false,
      seen: { seen: {}, failed: {} },
    })
    expect(rows.map((row) => [row.id, row.title])).toEqual([['fresh', 'fresh']])
  })

  test('the seen state: first sight is not unread; the open session is always seen', () => {
    let state = noteSeen({ seen: {}, failed: {} }, [summary('a', 10), summary('b', 20)], 'a')
    expect(state.seen).toEqual({ a: 10, b: 20 })
    state = noteSeen(state, [summary('a', 30), summary('b', 40)], 'a')
    // `a` is open, so seen up to now; `b` moved on while away, so it stays behind (unread).
    expect(state.seen).toEqual({ a: 30, b: 20 })
    expect(noteSeen(state, [summary('a', 30)], 'a')).toBe(state)
  })

  test('groups by project keep the open project, even with no sessions', () => {
    const rows = sessionRows([summary('x', 100, { project: '/work/site' })], { open, currentId: null, busy: false, seen: { seen: {}, failed: {} } })
    expect(groupRows(rows, 'project', open).map((group) => [group.label, group.rows.length])).toEqual([
      ['app', 0],
      ['site', 1],
    ])
    expect(groupRows(rows, 'status', open).map((group) => group.label)).toEqual(['Everything else'])
  })
})
