import { describe, expect, test } from 'bun:test'

import { STATUS_BAR_ACCOUNT, STATUS_BAR_STATUS } from '../../testing/status-bar-fixtures'
import { accountRows, accountTitle, formatReset, sessionTokens, updatedAgo } from '../account-usage'
import { breakdownRows, contextUsage, usableTokens } from '../context-breakdown'

describe('context usage', () => {
  test('the fill is what a request sends over the usable window', () => {
    const usage = contextUsage(STATUS_BAR_STATUS)!
    expect(usage.estimated).toBe(false)
    expect(usage.usable).toBe(1_048_576 - 65_536)
    expect(Math.round(usage.percent)).toBe(35)
    expect(Math.round(usage.thresholdPercent)).toBe(80)
  })

  test('rows add up to the usable window, and Free is never negative', () => {
    const usage = contextUsage(STATUS_BAR_STATUS)!
    const rows = breakdownRows(usage)
    expect(rows.map((row) => row.label)).toEqual([
      'System prompt',
      'Tools (23)',
      'Project instructions & memory',
      'Conversation',
      'Tool outputs',
      'Free',
    ])
    const total = rows.reduce((sum, row) => sum + (row.tokens ?? 0), 0)
    expect(total).toBe(usableTokens(STATUS_BAR_STATUS.context!))
    expect(rows[2]!.detail).toBe('neuron-v2/NEURON.md, .neuron/memory.md')

    const over = contextUsage({
      ...STATUS_BAR_STATUS,
      context: { ...STATUS_BAR_STATUS.context!, conversation_tokens: 5_000_000, used_tokens: 5_164_200 },
    })!
    expect(over.percent).toBe(100)
    expect(breakdownRows(over).at(-1)!.tokens).toBe(0)
  })

  test('older engines: one estimated conversation row from context_percent', () => {
    const usage = contextUsage({ ...STATUS_BAR_STATUS, context: undefined, context_percent: 140 })!
    expect(usage).toMatchObject({ percent: 100, estimated: true, usable: null })
    expect(breakdownRows(usage)).toEqual([
      { label: 'Conversation', tokens: null, percent: 100, detail: null },
    ])
    expect(contextUsage({ ...STATUS_BAR_STATUS, context: undefined, session_id: '' })).toBeNull()
    expect(contextUsage(null)).toBeNull()
  })
})

describe('account rows', () => {
  const now = Date.parse('2026-11-03T11:22:00Z')

  test('the 5-hour window and today, with when each resets', () => {
    const rows = accountRows(STATUS_BAR_ACCOUNT, now, 'UTC')
    expect(rows).toEqual([
      { label: '5-hour window', value: '120K / 1M tokens', percent: 12, resets: '14:32 (in 3h 10m)' },
      { label: 'Today', value: '37 / 100 requests', percent: null, resets: '00:00 (in 12h 38m)' },
    ])
    expect(accountTitle(STATUS_BAR_ACCOUNT)).toBe('Free (dev@example.com)')
  })

  test('without the window: today only; a read error says why', () => {
    expect(accountRows({ ...STATUS_BAR_ACCOUNT, usage_window: undefined }, now, 'UTC').map((row) => row.label)).toEqual([
      'Today',
    ])
    expect(accountRows({ ...STATUS_BAR_ACCOUNT, usage_error: 'timed out' }, now)).toEqual([
      { label: 'Usage', value: 'unavailable: timed out', percent: null, resets: null },
    ])
  })

  test('reset wording and freshness', () => {
    expect(formatReset('2026-11-10T00:00:00Z', now, 'UTC')).toBe('Nov 10')
    expect(formatReset(null, now)).toBeNull()
    expect(formatReset('not a date', now)).toBeNull()
    expect(updatedAgo({ ...STATUS_BAR_ACCOUNT, usage_updated_at: now / 1000 - 130 }, now)).toBe('Updated 2m ago')
    expect(updatedAgo({ ...STATUS_BAR_ACCOUNT, usage_updated_at: now / 1000 - 5 }, now)).toBe('Updated just now')
  })

  test('session tokens add a running turn and mark it ~', () => {
    const usage = {
      turn: { input: 1, output: 1, cache_read: 0, cache_write: 0 },
      cumulative: { input: 10_000, output: 1_000, cache_read: 2_400, cache_write: 0 },
      estimated: false,
    }
    expect(sessionTokens(usage, null)).toBe('↑12.4K ↓1K')
    expect(sessionTokens(usage, { input: 5_000, output: 500 })).toBe('↑~17.4K ↓~1.5K')
    expect(sessionTokens(null, null)).toBeNull()
  })
})
