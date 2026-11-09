import { describe, expect, test } from 'bun:test'

import { createUpdateChecker } from '../src/platform/updates'

describe('the update check', () => {
  test('signed builds ask the updater and report what it finds', async () => {
    const checker = createUpdateChecker({ signed: true, check: async () => ({ version: '0.2.0', body: 'Fixes' }) })
    expect(checker.manual).toBe(false)
    expect(await checker.check()).toEqual({ version: '0.2.0', notes: 'Fixes' })
  })

  test('signed builds report no update, and swallow a failed check', async () => {
    expect(await createUpdateChecker({ signed: true, check: async () => null }).check()).toBeNull()
    const failing = createUpdateChecker({
      signed: true,
      check: async () => {
        throw new Error('offline')
      },
    })
    expect(await failing.check()).toBeNull()
  })

  test('an unsigned build never calls the updater: updates are manual', async () => {
    let called = 0
    const checker = createUpdateChecker({
      signed: false,
      check: async () => {
        called++
        return null
      },
    })
    expect(checker.manual).toBe(true)
    expect(await checker.check()).toBeNull()
    expect(called).toBe(0)
  })
})
