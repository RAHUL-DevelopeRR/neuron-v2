import { describe, expect, test } from 'bun:test'

import { createDoubleClickDetector, DOUBLE_CLICK_MS } from '../double-click'

function detector() {
  let now = 1_000
  const clicks = createDoubleClickDetector(DOUBLE_CLICK_MS, () => now)
  return {
    click: (id: string) => clicks.click(id),
    reset: () => clicks.reset(),
    wait: (ms: number) => {
      now += ms
    },
  }
}

describe('double clicks', () => {
  test('two clicks on the same thing within the window make one', () => {
    const d = detector()
    expect(d.click('a')).toBe(false)
    d.wait(DOUBLE_CLICK_MS - 1)
    expect(d.click('a')).toBe(true)
  })

  test('too slow, or on something else, is two single clicks', () => {
    const d = detector()
    expect(d.click('a')).toBe(false)
    d.wait(DOUBLE_CLICK_MS + 1)
    expect(d.click('a')).toBe(false)
    expect(d.click('b')).toBe(false)
  })

  test('a third quick click starts a new pair; reset forgets the last click', () => {
    const d = detector()
    d.click('a')
    expect(d.click('a')).toBe(true)
    expect(d.click('a')).toBe(false)
    d.reset()
    expect(d.click('a')).toBe(false)
  })
})
