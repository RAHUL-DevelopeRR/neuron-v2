import { afterEach, describe, expect, test } from 'bun:test'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'

import { PullRequestChip } from '../src/components/ChatHeader'
import { checkCounts, normalizePullRequestChecks, pullRequestLabel, type PullRequestStatus } from '../src/pullRequest'

afterEach(cleanup)

// From Freebuff's pull-request.test.ts (commit 0dc95cb, Apache-2.0), for the ported function.
test('check runs and commit statuses share a conservative aggregate', () => {
  expect(normalizePullRequestChecks([])).toBe('none')
  expect(normalizePullRequestChecks(null)).toBe('unknown')
  expect(normalizePullRequestChecks([{ conclusion: 'success' }])).toBe('unknown')
  expect(normalizePullRequestChecks([{ status: 'completed', conclusion: 'success' }, { state: 'SUCCESS' }])).toBe('passed')
  expect(normalizePullRequestChecks([{ status: 'COMPLETED', conclusion: 'SKIPPED' }, { state: 'PENDING' }])).toBe('pending')
  expect(normalizePullRequestChecks([{ status: 'IN_PROGRESS' }, { state: 'failure' }])).toBe('failed')
  expect(normalizePullRequestChecks([{ status: 'COMPLETED', conclusion: null }, { state: 'SUCCESS' }])).toBe('unknown')
})

const pr = (extra: Partial<PullRequestStatus> = {}): PullRequestStatus => ({
  url: 'https://github.com/acme/app/pull/12',
  number: 12,
  title: 'Fix the login redirect',
  state: 'open',
  draft: true,
  conflict: false,
  checks: [
    { status: 'COMPLETED', conclusion: 'SUCCESS' },
    { status: 'COMPLETED', conclusion: 'NEUTRAL' },
    { state: 'SUCCESS' },
    { state: 'PENDING' },
  ],
  ...extra,
})

describe('the pull request in words', () => {
  test('passed checks are counted out of all', () => {
    expect(checkCounts(pr().checks)).toEqual({ passed: 3, total: 4 })
    expect(checkCounts([])).toEqual({ passed: 0, total: 0 })
    expect(checkCounts(null)).toBeNull()
  })

  test('the label says number, state and checks', () => {
    expect(pullRequestLabel(pr())).toBe('PR #12 · draft · 3/4 checks')
    expect(pullRequestLabel(pr({ draft: false }))).toBe('PR #12 · open · 3/4 checks')
    expect(pullRequestLabel(pr({ draft: false, conflict: true, checks: [] }))).toBe('PR #12 · open · conflicts')
    expect(pullRequestLabel(pr({ state: 'merged', draft: false }))).toBe('PR #12 · merged')
    expect(pullRequestLabel(pr({ state: 'closed', checks: null }))).toBe('PR #12 · closed')
  })
})

describe('the pull request chip', () => {
  test('shows the pull request and opens it through the safe link path', async () => {
    const opened: string[] = []
    render(<PullRequestChip load={async () => ({ pullRequest: pr() })} onOpen={(url) => opened.push(url)} />)
    const chip = await screen.findByRole('button', { name: /pull request #12/i })
    expect(chip.textContent).toBe('PR #12 · draft · 3/4 checks')
    expect(chip.getAttribute('data-checks')).toBe('pending')
    fireEvent.click(chip)
    expect(opened).toEqual(['https://github.com/acme/app/pull/12'])
  })

  test('nothing shows without a pull request or when it cannot be known', async () => {
    for (const snapshot of [{ pullRequest: null }, { pullRequest: null, unavailable: true }]) {
      let asked = 0
      const { container, unmount } = render(<PullRequestChip load={async () => (asked++, snapshot)} onOpen={() => {}} />)
      await act(async () => {})
      expect(asked).toBe(1)
      expect(container.childElementCount).toBe(0)
      unmount()
    }
    const failing = render(<PullRequestChip load={() => Promise.reject(new Error('gh missing'))} onOpen={() => {}} />)
    await act(async () => {})
    expect(failing.container.childElementCount).toBe(0)
  })

  test('it asks again after a minute', async () => {
    let asked = 0
    const timers: Array<() => void> = []
    render(
      <PullRequestChip
        load={async () => (asked++, { pullRequest: pr() })}
        onOpen={() => {}}
        every={(run) => {
          timers.push(run)
          return () => {}
        }}
      />,
    )
    await screen.findByRole('button', { name: /pull request #12/i })
    expect(asked).toBe(1)
    await act(async () => timers[0]!())
    expect(asked).toBe(2)
  })
})
