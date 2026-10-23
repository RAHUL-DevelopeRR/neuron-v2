import { describe, expect, test } from 'bun:test'

import { accountView, initials, planLabel, planLine, type AccountDetails } from '../src/account'
import { gatewayProblem } from '../src/gateway'

describe('the account view', () => {
  test('initials from the name, then the email', () => {
    expect(initials('Rahul S')).toBe('RS')
    expect(initials('  ada   lovelace byron ')).toBe('AB')
    expect(initials('neo')).toBe('NE')
    expect(initials(null, 'rahul@zero-x.live')).toBe('R')
    expect(initials(null, null)).toBe('?')
  })

  test('plan names', () => {
    expect(planLabel('pro')).toBe('Pro')
    expect(planLabel('MAX')).toBe('Max')
    expect(planLabel('enterprise')).toBe('Enterprise')
    expect(planLabel('')).toBeNull()
  })

  test('from the status alone: the email and plan, no usage yet', () => {
    const view = accountView(true, { email: 'rahul@zero-x.live', plan: 'pro', source: 'device', expires_at: null })
    expect(view).toMatchObject({ signedIn: true, zeroX: true, email: 'rahul@zero-x.live', name: 'rahul', initials: 'R', plan: 'Pro' })
    expect(view.meters).toEqual([])
    expect(planLine(view)).toBe('Pro')
  })

  test('with the account details: the name and the most used of the day', () => {
    const details: AccountDetails = {
      email: 'rahul@zero-x.live',
      name: 'Rahul S',
      plan: 'pro',
      limits: { requests_per_day: 200, tokens_per_day: 1_000_000 },
      usage_today: { requests: 74, tokens: 120_000 },
      resets_at: '2026-10-24T00:00:00Z',
      credits_usd: 4.5,
    }
    const view = accountView(true, null, details)
    expect(view.initials).toBe('RS')
    expect(view.meters.map((meter) => [meter.label, meter.percent])).toEqual([
      ['Requests today', 37],
      ['Tokens today', 12],
    ])
    expect(planLine(view)).toBe('Pro · 37% of today used')
    expect(view.creditsUsd).toBe(4.5)
  })

  test('an own provider key: not a Zero-X account', () => {
    const view = accountView(false, null)
    expect(view).toMatchObject({ signedIn: false, zeroX: false, plan: null, initials: '?' })
    expect(planLine(view)).toBeNull()
  })
})

describe('gateway problems', () => {
  test('typed errors map by code and status', () => {
    expect(gatewayProblem({ message: 'Session expired', status: 401 })?.kind).toBe('sign_in')
    expect(gatewayProblem({ message: 'Pro only', code: 'upgrade_required', upgrade_url: 'https://zero-x.live/pricing' })).toMatchObject({
      kind: 'upgrade',
      upgradeUrl: 'https://zero-x.live/pricing',
    })
    expect(gatewayProblem({ message: 'Daily quota reached', code: 'insufficient_quota', resets_at: '2026-10-24T00:00:00Z' })).toMatchObject({
      kind: 'quota',
      resetsAt: '2026-10-24T00:00:00Z',
    })
    expect(gatewayProblem({ message: 'Busy', status: 503, retry_after_seconds: 20 })).toMatchObject({ kind: 'busy', retryAfterSeconds: 20 })
    expect(gatewayProblem({ message: 'offline', code: 'network' })?.kind).toBe('unreachable')
  })

  test("older engines' messages are recognised", () => {
    expect(gatewayProblem({ message: 'Could not reach Zero-X: connection refused' })?.kind).toBe('unreachable')
    expect(
      gatewayProblem({ message: 'This Zero-X gateway does not support device sign-in yet. Use `neuron auth login --with-token`.' })?.kind,
    ).toBe('unreachable')
    expect(gatewayProblem({ message: 'gateway error insufficient_quota: Daily quota reached' })?.kind).toBe('quota')
    expect(gatewayProblem({ message: 'api returned 401 Unauthorized' })?.kind).toBe('sign_in')
    expect(gatewayProblem({ message: 'service_unavailable: try again in 30 seconds' })).toMatchObject({ kind: 'busy', retryAfterSeconds: 30 })
  })

  test('other errors are not gateway problems', () => {
    expect(gatewayProblem({ message: 'Sign in to Zero-X first: /login.' })).toBeNull()
    expect(gatewayProblem({ message: 'unknown slash command /frobnicate' })).toBeNull()
  })
})
