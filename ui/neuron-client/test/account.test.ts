import { describe, expect, test } from 'bun:test'

import { accountView, initials, planLabel, planLine } from '../src/account'
import { gatewayProblem } from '../src/gateway'
import type { AccountDetails, ErrorDetail } from '../src/protocol'

const details = (over: Partial<AccountDetails> = {}): AccountDetails => ({
  email: 'rahul@zero-x.live',
  name: 'Rahul S',
  plan: 'pro',
  plan_name: 'Pro',
  source: 'device',
  expires_at: null,
  limits: { requests: 200, tokens: 1_000_000 },
  usage_today: { requests: 74, tokens: 120_000 },
  resets_at: '2026-10-24T00:00:00Z',
  credit_balance_cents: 450,
  suspended: false,
  dashboard_url: 'https://zero-x.live/dashboard',
  pricing_url: 'https://zero-x.live/pricing',
  usage_updated_at: 1_792_800_000,
  usage_error: null,
  ...over,
})

const detail = (over: Partial<ErrorDetail>): ErrorDetail => ({
  code: 'unknown',
  status: null,
  upgrade_url: null,
  resets_at: null,
  retry_after_seconds: null,
  ...over,
})

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
    expect(view).toMatchObject({ signedIn: true, email: 'rahul@zero-x.live', name: 'rahul', initials: 'R', plan: 'Pro' })
    expect(view.meters).toEqual([])
    expect(planLine(view)).toBe('Pro')
  })

  test("with the engine's account: the name, the most used of the day, credits and links", () => {
    const view = accountView(true, null, details())
    expect(view.initials).toBe('RS')
    expect(view.meters.map((meter) => [meter.label, meter.percent])).toEqual([
      ['Requests today', 37],
      ['Tokens today', 12],
    ])
    expect(planLine(view)).toBe('Pro · 37% of today used')
    expect(view).toMatchObject({ creditsUsd: 4.5, dashboardUrl: 'https://zero-x.live/dashboard', pricingUrl: 'https://zero-x.live/pricing' })
  })

  test("the plan's own name wins; a usage error is kept", () => {
    const view = accountView(true, null, details({ plan_name: 'Pro (yearly)', limits: null, usage_today: null, usage_error: 'gateway timed out' }))
    expect(view.plan).toBe('Pro (yearly)')
    expect(view.todayPercent).toBeNull()
    expect(view.usageError).toBe('gateway timed out')
  })

  test('signed out, or an own provider key', () => {
    const view = accountView(false, null)
    expect(view).toMatchObject({ signedIn: false, plan: null, initials: '?' })
    expect(planLine(view)).toBeNull()
  })
})

describe('gateway problems', () => {
  test("the engine's error details map by code and status", () => {
    expect(gatewayProblem({ message: 'Session expired', detail: detail({ code: 'invalid_session', status: 401 }) })?.kind).toBe('sign_in')
    expect(gatewayProblem({ message: 'Sign in first', detail: detail({ code: 'sign_in_required' }) })?.kind).toBe('sign_in')
    expect(
      gatewayProblem({ message: 'Pro only', detail: detail({ code: 'upgrade_required', status: 403, upgrade_url: 'https://zero-x.live/pricing' }) }),
    ).toMatchObject({ kind: 'upgrade', upgradeUrl: 'https://zero-x.live/pricing' })
    expect(
      gatewayProblem({ message: 'Daily quota reached', detail: detail({ code: 'insufficient_quota', status: 429, resets_at: '2026-10-24T00:00:00Z' }) }),
    ).toMatchObject({ kind: 'quota', resetsAt: '2026-10-24T00:00:00Z' })
    expect(gatewayProblem({ message: 'Busy', detail: detail({ code: 'service_unavailable', status: 503, retry_after_seconds: 20 }) })).toMatchObject({
      kind: 'busy',
      retryAfterSeconds: 20,
    })
    expect(gatewayProblem({ message: 'offline', detail: detail({ code: 'unreachable' }) })?.kind).toBe('unreachable')
    // An older gateway without device sign-in answers 404.
    expect(gatewayProblem({ message: 'not offered', detail: detail({ code: 'not_available', status: 404 }) })?.kind).toBe('unreachable')
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
    expect(gatewayProblem({ message: 'bad flag', detail: detail({ code: 'invalid_request', status: 400 }) })).toBeNull()
  })
})
