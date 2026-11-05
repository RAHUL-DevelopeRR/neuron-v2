import { afterEach, describe, expect, test } from 'bun:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { initializeThemeStore } from '../../hooks/use-theme'
import { useQuotaStore } from '../../state/quota-store'
import { QuotaErrorCard } from '../quota-error-card'
import { UserErrorBanner } from '../user-error-banner'

initializeThemeStore()

const ERROR =
  'This request needs 84,937 tokens but only 74,329 of your 1,000,000 tokens for this 5-hour window are left (insufficient_quota); resets at 2026-11-04T18:48:59.950Z'

afterEach(() => {
  useQuotaStore.getState().clear()
  useQuotaStore.getState().setAutoContinue(false, false)
})

describe('QuotaErrorCard', () => {
  test('counts down and offers auto-continue before the reset', () => {
    const resumeAt = Date.now() + 2 * 3_600_000 + 15 * 60_000
    const block = { messageId: 'ai-1', code: 'insufficient_quota', resumeAt, at: Date.now() }
    const markup = renderToStaticMarkup(<QuotaErrorCard block={block} error={ERROR} />)
    expect(markup).toContain('Usage limit reached')
    expect(markup).toContain('Resets in 2h 1')
    expect(markup).toContain('[ ] Auto-continue when limits reset')
  })

  // The armed countdown's wording is in utils/__tests__/quota-card.test.ts:
  // static rendering reads the store's initial state only.

  test('reads as reset once the limit has passed', () => {
    const block = { messageId: 'ai-1', code: 'insufficient_quota', resumeAt: Date.now() - 60_000, at: 0 }
    const markup = renderToStaticMarkup(<QuotaErrorCard block={block} error={ERROR} />)
    expect(markup).toContain('Usage limit reset')
    expect(markup).not.toContain('Auto-continue when limits reset')
  })
})

describe('UserErrorBanner with a passed reset time', () => {
  test('notes that the limit has reset', () => {
    // The screenshot's error, from a day already gone.
    const markup = renderToStaticMarkup(<UserErrorBanner error={ERROR} />)
    expect(markup).toContain('This limit reset at')
    expect(markup).toContain('send a message to go on')
  })
})
