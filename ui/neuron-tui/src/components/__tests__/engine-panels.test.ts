import { describe, expect, test } from 'bun:test'

import { fitParts, footerParts, formatGit } from '../engine-footer'
import { modelRows, visibleWindow } from '../model-picker'
import { formatRemaining, signInView } from '../sign-in-panel'

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
    expect(signInView({ ...STATUS, session_id: '', signed_in: false }, null, 0)).toEqual({
      kind: 'signed_out',
    })
    expect(signInView({ ...STATUS, signed_in: false }, null, 0)).toBeNull()
    expect(signInView(STATUS, null, 0)).toBeNull()
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
      },
      {
        id: 'zx-big',
        label: 'Big',
        detail: 'zx-big · 262.1K context · Pro plan only',
        available: false,
        current: false,
        marker: null,
      },
    ])
    expect(modelRows(null)).toEqual([])
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

  test('warns about an untrusted folder and an expired login', () => {
    const parts = footerParts(
      { ...STATUS, workspace_trusted: false, git: null, signed_in: false, plan_mode: true },
      null,
    )
    expect(parts).toEqual([
      { text: 'claude-sonnet-4-6', tone: 'normal' },
      { text: 'workspace-write · plan', tone: 'muted' },
      { text: 'untrusted folder', tone: 'warning' },
      { text: 'ctx 4%', tone: 'muted' },
      { text: 'Zero-X session expired: /login', tone: 'warning' },
    ])
    expect(footerParts(null, null)).toEqual([])
  })

  test('parts that do not fit are dropped from the end', () => {
    const parts = footerParts(STATUS, null)
    // "claude-sonnet-4-6 · workspace-write" is 35 columns.
    expect(fitParts(parts, 35).map((part) => part.text)).toEqual([
      'claude-sonnet-4-6',
      'workspace-write',
    ])
    expect(fitParts(parts, 5)).toEqual([])
  })
})
