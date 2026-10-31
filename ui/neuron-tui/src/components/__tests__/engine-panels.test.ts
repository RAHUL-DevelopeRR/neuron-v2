import { describe, expect, test } from 'bun:test'

import { MODEL_MIN_WIDTH, fitParts, footerModel, footerParts, formatGit } from '../engine-footer'
import {
  fitModelRow,
  modelRows,
  truncateToWidth,
  unavailableMessage,
  visibleWindow,
} from '../model-picker'
import { formatRemaining, signInUrl, signInView } from '../sign-in-panel'

import type { ModelsEvent, StatusEvent } from '../../engine/protocol'

const STATUS: StatusEvent = {
  type: 'status',
  model: 'claude-sonnet-4-6',
  permission_mode: 'workspace-write',
  session_id: 'session-1',
  cwd: '/work',
  workspace_trusted: true,
  plan_mode: false,
  context_percent: 4,
  git: { branch: 'main', changed: 3, staged: 1, unstaged: 2, untracked: 0, conflicted: 0 },
  signed_in: true,
  account: { email: null, plan: 'pro', source: 'stored login', expires_at: null },
}

// The fake gateway's catalog, as the engine passes it on.
const MODELS: ModelsEvent = {
  type: 'models',
  current: 'zx-fast',
  models: [
    {
      id: 'zx-fast',
      display_name: 'Fast',
      available: true,
      unavailable_reason: null,
      context_window: 131_072,
      tiers: ['free'],
    },
    {
      id: 'zx-big',
      display_name: 'Big',
      available: false,
      unavailable_reason: 'Pro plan only',
      context_window: 262_144,
      tiers: ['pro'],
    },
  ],
}

describe('sign-in panel', () => {
  const signIn = {
    type: 'sign_in_code' as const,
    url: 'https://zero-x.live/device',
    url_complete: null,
    user_code: 'BCDF-GHJK',
    expires_in_secs: 600,
    browser_opened: false,
    receivedAt: 1_000,
    approved: false,
  }

  test('shows the code and its time left, then the session start', () => {
    expect(signInView(null, signIn, 1_000 + 65_000)).toEqual({
      kind: 'code',
      url: 'https://zero-x.live/device',
      userCode: 'BCDF-GHJK',
      remainingSecs: 535,
      browserOpened: false,
    })
    expect(signInView(null, signIn, 1_000 + 700_000)).toMatchObject({ remainingSecs: 0 })
    expect(signInView(null, { ...signIn, approved: true }, 0)).toEqual({ kind: 'starting' })
  })

  test('asks for /login only in the signed-out stage', () => {
    expect(
      signInView({ ...STATUS, session_id: '', signed_in: false, account: null }, null, 0),
    ).toEqual({ kind: 'signed_out', expired: false, email: null })
    expect(signInView({ ...STATUS, signed_in: false }, null, 0)).toBeNull()
    expect(signInView(STATUS, null, 0)).toBeNull()
  })

  test('a stored login that expired or was revoked asks to sign in again', () => {
    // The engine reads a revoked login as signed in (it has not expired) but
    // starts signed out with it.
    const revoked: StatusEvent = {
      ...STATUS,
      session_id: '',
      signed_in: true,
      sign_in_required: true,
      account: { email: 'ana@example.com', plan: 'free', source: 'stored login', expires_at: null },
    }
    expect(signInView(revoked, null, 0)).toEqual({
      kind: 'signed_out',
      expired: true,
      email: 'ana@example.com',
    })
    // The footer no longer shows the plan as if it were usable.
    expect(footerParts(revoked, null).map((part) => part.text)).toEqual([
      'claude-sonnet-4-6',
      'workspace-write',
      'main +1 ~2',
      'Zero-X session expired: /login',
    ])
  })

  test('the sign-in link is a full https URL', () => {
    expect(signInUrl('https://zero-x.live/device?code=AB')).toBe('https://zero-x.live/device?code=AB')
    expect(signInUrl('zero-x.live/device')).toBe('https://zero-x.live/device')
    expect(signInView(null, { ...signIn, url: 'zero-x.live/device' }, 1_000)).toMatchObject({
      url: 'https://zero-x.live/device',
    })
  })

  test('formats the countdown as m:ss', () => {
    expect(formatRemaining(535)).toBe('8:55')
    expect(formatRemaining(9)).toBe('0:09')
  })
})

describe('model picker', () => {
  test('rows name the model, its context and plans, or why it is unavailable', () => {
    expect(modelRows(MODELS)).toEqual([
      {
        id: 'zx-fast',
        label: 'Fast',
        detail: 'zx-fast · 131.1K context · free',
        available: true,
        current: true,
        marker: null,
        unavailableReason: null,
      },
      {
        id: 'zx-big',
        label: 'Big',
        detail: 'zx-big · 262.1K context · Pro plan only',
        available: false,
        current: false,
        marker: null,
        unavailableReason: 'Pro plan only',
      },
    ])
    expect(modelRows(null)).toEqual([])
  })

  test('Enter on a model that cannot be picked says why', () => {
    const [, big] = modelRows(MODELS)
    expect(unavailableMessage(big!)).toBe("Big can't be picked: Pro plan only.")
    const blurred = modelRows(
      {
        ...MODELS,
        models: [
          { ...MODELS.models[0]!, status: 'blurred', available_again_at: '2026-10-21T13:20:00Z' },
        ],
      },
      Date.parse('2026-10-21T10:00:00Z'),
    )[0]!
    expect(unavailableMessage(blurred)).toBe(
      "Fast can't be picked: the gateway took it out of rotation; back in 3h 20m.",
    )
  })

  test('long rows end in an ellipsis: the detail gives way first, then the name', () => {
    const [fast] = modelRows(MODELS)
    // "Fast  (current)  zx-fast · 131.1K context · free" is 49 columns.
    expect(fitModelRow(fast!, 60)).toEqual({
      label: 'Fast',
      current: '  (current)',
      marker: '',
      detail: '  zx-fast · 131.1K context · free',
    })
    expect(fitModelRow(fast!, 30)).toEqual({
      label: 'Fast',
      current: '  (current)',
      marker: '',
      detail: '  zx-fast · 13…',
    })
    const long = { ...fast!, label: 'A very long model display name indeed' }
    expect(fitModelRow(long, 20)).toEqual({
      label: 'A very l…',
      current: '  (current)',
      marker: '',
      detail: '',
    })
    expect(truncateToWidth('abcdef', 4)).toBe('abc…')
    expect(truncateToWidth('abc', 4)).toBe('abc')
  })

  test('the current model matches with the engine’s openai/ wire prefix too', () => {
    const rows = modelRows({ ...MODELS, current: 'openai/zx-fast' })
    expect(rows.map((row) => row.current)).toEqual([true, false])
  })

  test('gateway reason codes read as words; not_configured is not a plan upgrade', () => {
    const model = MODELS.models[1]!
    const reasons = (reason: string | null) =>
      modelRows({
        ...MODELS,
        models: [
          { ...model, display_name: null, context_window: null, unavailable_reason: reason },
        ],
      })[0]!.detail
    expect(reasons('not_configured')).toBe('not configured on this gateway')
    expect(reasons('upgrade_required')).toBe('needs a plan upgrade')
    expect(reasons('region_unavailable')).toBe('not available in your region')
    expect(reasons(null)).toBe('not on your plan')
  })

  describe('gateway health', () => {
    const NOW = Date.parse('2026-10-21T10:00:00Z')
    const withStatus = (
      status: 'available' | 'degraded' | 'busy' | 'blurred' | null | undefined,
      availableAgainAt: string | null = null,
    ): ModelsEvent => ({
      ...MODELS,
      models: [{ ...MODELS.models[0]!, status, available_again_at: availableAgainAt }],
    })

    test('a blurred model is greyed until it is back, with the time left', () => {
      const [row] = modelRows(withStatus('blurred', '2026-10-21T13:20:00Z'), NOW)
      expect(row!.available).toBe(false)
      expect(row!.detail).toBe('zx-fast · 131.1K context · back in 3h 20m')
      expect(row!.marker).toBeNull()
    })

    test('a blurred model without a return time is just unavailable', () => {
      const [row] = modelRows(withStatus('blurred'), NOW)
      expect(row!.available).toBe(false)
      expect(row!.detail).toBe('zx-fast · 131.1K context · temporarily unavailable')
    })

    test('a blurred model whose time has passed can be picked again', () => {
      const [row] = modelRows(withStatus('blurred', '2026-10-21T09:59:00Z'), NOW)
      expect(row!.available).toBe(true)
      expect(row!.detail).toBe('zx-fast · 131.1K context · free')
    })

    test('degraded and busy are marked but stay selectable', () => {
      for (const status of ['degraded', 'busy'] as const) {
        const [row] = modelRows(withStatus(status), NOW)
        expect(row!.available).toBe(true)
        expect(row!.marker).toBe(status)
      }
      // Older engines send no status; "available" needs no marker.
      expect(modelRows(withStatus(undefined), NOW)[0]!.marker).toBeNull()
      expect(modelRows(withStatus('available'), NOW)[0]!.marker).toBeNull()
    })
  })

  test('the visible window keeps the focused row in view', () => {
    expect(visibleWindow(3, 2, 8)).toEqual([0, 3])
    expect(visibleWindow(20, 0, 8)).toEqual([0, 8])
    expect(visibleWindow(20, 10, 8)).toEqual([6, 14])
    expect(visibleWindow(20, 19, 8)).toEqual([12, 20])
  })
})

describe('engine footer', () => {
  test('names the model auto picked', () => {
    expect(footerModel({ ...STATUS, model: 'auto' })).toBe('auto')
    expect(footerModel({ ...STATUS, model: 'auto', served_model: 'groq/qwen/qwen3.8-27b' })).toBe(
      'auto → groq/qwen/qwen3.8-27b',
    )
    expect(footerModel({ ...STATUS, served_model: STATUS.model })).toBe(STATUS.model)
    expect(footerParts({ ...STATUS, model: 'auto', served_model: 'm' }, null)[0]?.text).toBe('auto → m')
  })

  test('git shows the branch and its non-zero counts', () => {
    expect(formatGit(STATUS.git!)).toBe('main +1 ~2')
    expect(
      formatGit({ branch: null, changed: 4, staged: 0, unstaged: 0, untracked: 3, conflicted: 1 }),
    ).toBe('detached ?3 !1')
  })

  test('parts: model, mode, git, tokens, context and account', () => {
    const parts = footerParts(STATUS, {
      turn: { input: 10, output: 5, cache_read: 0, cache_write: 0 },
      cumulative: { input: 12_000, output: 2_100, cache_read: 400, cache_write: 0 },
      estimated: false,
    })
    expect(parts.map((part) => part.text)).toEqual([
      'claude-sonnet-4-6',
      'workspace-write',
      'main +1 ~2',
      '↑12.4K ↓2.1K',
      'ctx 4%',
      'Zero-X pro',
    ])
  })

  test('tokens count up while a turn streams, as estimates, until its usage settles them', () => {
    const usage = {
      turn: { input: 10, output: 5, cache_read: 0, cache_write: 0 },
      cumulative: { input: 12_000, output: 2_100, cache_read: 400, cache_write: 0 },
      estimated: false,
    }
    const tokens = (live: { input: number; output: number } | null, last = usage) =>
      footerParts(STATUS, last, live).find((part) => part.text.startsWith('↑'))?.text
    expect(tokens({ input: 9_000, output: 1_200 })).toBe('↑~21.4K ↓~3.3K')
    // The first turn: no totals yet, only what it used so far.
    expect(tokens({ input: 0, output: 450 }, null as never)).toBe('↑~0 ↓~450')
    expect(tokens(null)).toBe('↑12.4K ↓2.1K')
  })

  test('warns about an untrusted folder and an expired login', () => {
    const parts = footerParts(
      { ...STATUS, workspace_trusted: false, git: null, signed_in: false, plan_mode: true },
      null,
    )
    expect(parts.map(({ text, tone }) => ({ text, tone }))).toEqual([
      { text: 'claude-sonnet-4-6', tone: 'normal' },
      { text: 'workspace-write · plan', tone: 'muted' },
      { text: 'untrusted folder', tone: 'warning' },
      { text: 'ctx 4%', tone: 'muted' },
      { text: 'Zero-X session expired: /login', tone: 'warning' },
    ])
    expect(footerParts(null, null)).toEqual([])
  })

  test('the signed-in account shows its email after the plan', () => {
    const parts = footerParts(
      { ...STATUS, account: { ...STATUS.account!, email: 'ana@example.com' } },
      null,
    )
    expect(parts.map((part) => part.text).slice(-2)).toEqual(['Zero-X pro', 'ana@example.com'])
  })

  test('parts that do not fit are dropped, least important first', () => {
    const parts = footerParts(STATUS, null)
    // "claude-sonnet-4-6 · Zero-X pro" is 30 columns: the plan stays before the mode.
    expect(fitParts(parts, 35).map((part) => part.text)).toEqual([
      'claude-sonnet-4-6',
      'Zero-X pro',
    ])
    expect(fitParts(parts, 5)).toEqual([])
  })

  test('at 120 columns a long model id is cut short before the email is dropped', () => {
    // The e2e footer: before, the email was the first part to go.
    const status: StatusEvent = {
      ...STATUS,
      model: 'nvidia/nvidia/nemotron-3-ultra-550b-a55b',
      workspace_trusted: false,
      git: { branch: 'main', changed: 0, staged: 0, unstaged: 0, untracked: 0, conflicted: 0 },
      context_percent: 0,
      account: { ...STATUS.account!, plan: 'max', email: 'rahul@zero-x.live' },
    }
    const usage = {
      turn: { input: 10, output: 5, cache_read: 0, cache_write: 0 },
      cumulative: { input: 3_600, output: 39, cache_read: 0, cache_write: 0 },
      estimated: false,
    }
    // The footer box draws in the width less its 2 columns of padding.
    const shown = fitParts(footerParts(status, usage), 118).map((part) => part.text)
    expect(shown).toEqual([
      'nvidia/nvidia/nemotron-3-ultra-…',
      'workspace-write',
      'untrusted folder',
      'main',
      'ctx 0%',
      'Zero-X max',
      'rahul@zero-x.live',
    ])
    expect(shown.join(' · ').length).toBeLessThanOrEqual(118)
  })

  test('the email outlasts the git and token parts, and the model is never cut below its minimum', () => {
    const status: StatusEvent = {
      ...STATUS,
      model: 'nvidia/nvidia/nemotron-3-ultra-550b-a55b',
      workspace_trusted: false,
      git: { branch: 'feature/long-branch-name', changed: 4, staged: 1, unstaged: 2, untracked: 1, conflicted: 0 },
      account: { ...STATUS.account!, plan: 'free', email: 'someone.long@example.com' },
    }
    const usage = {
      turn: { input: 10, output: 5, cache_read: 0, cache_write: 0 },
      cumulative: { input: 19_100, output: 646, cache_read: 0, cache_write: 0 },
      estimated: false,
    }
    const shown = fitParts(footerParts(status, usage), 118).map((part) => part.text)
    expect(shown).toContain('someone.long@example.com')
    expect(shown).toContain('Zero-X free')
    expect(shown).not.toContain('↑19.1K ↓646')
    expect(shown[0]!.endsWith('…')).toBe(true)
    expect(shown[0]!.length).toBeGreaterThanOrEqual(MODEL_MIN_WIDTH)
    expect(shown.join(' · ').length).toBeLessThanOrEqual(118)

    // Too narrow for the plan next to even the shortest model id: the plan goes.
    const narrow = fitParts(footerParts(status, usage), 20).map((part) => part.text)
    expect(narrow).toEqual(['nvidia/nvidia/nemot…'])
    expect(fitParts(footerParts(status, usage), 5)).toEqual([])
  })
})
