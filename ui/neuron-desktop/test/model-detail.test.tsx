/**
 * What the model picker says about a model besides its name. The gateway's fields are codes ("upgrade_required",
 * "blurred") and timestamps; a person reads words, the terminal UI's words (components/model-picker.tsx there):
 * why a model cannot be picked, when the gateway gives one back, and a health worth a glance. Plan names (free,
 * paid, pro) stay: they are plans, not providers.
 */
import { afterEach, describe, expect, setSystemTime, test } from 'bun:test'
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'

import type { ModelInfo } from '@neuron/client'

import { ModelPicker } from '../src/components/ModelPicker'
import { backInLabel, isBlurred, modelDetail } from '../src/modelNames'

// 10:00 UTC. The gateway's `available_again_at` is relative to it.
const NOW = Date.parse('2026-10-21T10:00:00Z')
const at = (minutesFromNow: number) => new Date(NOW + minutesFromNow * 60_000).toISOString()

afterEach(() => {
  cleanup()
  setSystemTime()
})

const model = (id: string, over: Partial<ModelInfo> = {}): ModelInfo => ({
  id,
  display_name: id.toUpperCase(),
  provider: null,
  available: true,
  unavailable_reason: null,
  context_window: null,
  tiers: ['pro'],
  ...over,
})

/** The picker open on `models`: each row's text, its detail, and whether it can be picked. */
function open(models: ModelInfo[], picked: string[] = []) {
  setSystemTime(new Date(NOW))
  render(<ModelPicker current="" models={models} disabled={false} onOpen={() => {}} onPick={(id) => picked.push(id)} />)
  fireEvent.click(screen.getByTitle('Switch model'))
  const list = screen.getByRole('listbox', { name: 'Models' })
  const rows = within(list).getAllByRole('option') as HTMLButtonElement[]
  return {
    list,
    rows,
    details: rows.map((row) => row.querySelectorAll('span')[1]?.textContent),
    disabled: rows.map((row) => row.disabled),
  }
}

describe('why a model cannot be picked, in words', () => {
  test("the gateway's reason codes read as the terminal UI's words", () => {
    const { details, disabled } = open([
      model('a', { available: false, unavailable_reason: 'upgrade_required' }),
      model('b', { available: false, unavailable_reason: 'region_unavailable' }),
      model('c', { available: false, unavailable_reason: 'not_configured' }),
    ])
    expect(details).toEqual(['needs a plan upgrade', 'not available in your region', 'not configured on this gateway'])
    expect(disabled).toEqual([true, true, true])
  })

  test('a model that is not available, with no reason given, is not on the plan', () => {
    for (const unavailable_reason of [null, undefined, '']) {
      const { details, disabled } = open([model('a', { available: false, unavailable_reason })])
      expect(details).toEqual(['not on your plan'])
      expect(disabled).toEqual([true])
      cleanup()
    }
  })

  test('a reason that is not one of the codes is shown as the gateway sent it', () => {
    // Free text cannot be scrubbed of a provider's name by the model-name rules (they work on the end of a name),
    // so it is shown as sent rather than half cleaned.
    const { details } = open([model('a', { available: false, unavailable_reason: 'under maintenance until Friday' })])
    expect(details).toEqual(['under maintenance until Friday'])
  })

  test('a reason that is the name of something every object has is still just text', () => {
    const { details } = open([
      model('a', { available: false, unavailable_reason: 'constructor' }),
      model('b', { available: false, unavailable_reason: '__proto__' }),
      model('c', { available: false, unavailable_reason: 'toString' }),
    ])
    expect(details).toEqual(['constructor', '__proto__', 'toString'])
  })
})

describe('a model the gateway took out of rotation', () => {
  test('says when it is back, and cannot be picked until then', () => {
    const picked: string[] = []
    const { rows, details, disabled } = open([model('a', { status: 'blurred', available_again_at: at(200) })], picked)
    expect(details).toEqual(['back in 3h 20m'])
    expect(disabled).toEqual([true])
    fireEvent.click(rows[0]!)
    expect(picked).toEqual([])
  })

  test('with no time to come back, is just unavailable', () => {
    for (const available_again_at of [null, undefined, 'not a date']) {
      const { details, disabled } = open([model('a', { status: 'blurred', available_again_at })])
      expect(details).toEqual(['temporarily unavailable'])
      expect(disabled).toEqual([true])
      cleanup()
    }
  })

  test('can be picked again once its time has passed, with its plans as before', () => {
    const picked: string[] = []
    const { rows, details, disabled } = open([model('a', { status: 'blurred', available_again_at: at(-1), tiers: ['free', 'paid'] })], picked)
    expect(details).toEqual(['free, paid'])
    expect(disabled).toEqual([false])
    fireEvent.click(rows[0]!)
    expect(picked).toEqual(['a'])
  })

  test('the time left is in the words of the terminal UI: minutes, hours and minutes, never less than a minute', () => {
    const left = (minutes: number) => backInLabel(at(minutes), NOW)
    expect(left(0.2)).toBe('back in 1m')
    expect(left(1)).toBe('back in 1m')
    expect(left(59)).toBe('back in 59m')
    expect(left(60)).toBe('back in 1h')
    expect(left(61)).toBe('back in 1h 1m')
    expect(left(200)).toBe('back in 3h 20m')
    expect(left(25 * 60)).toBe('back in 25h')
    // A part of a minute counts as the whole of it, and a time that has just passed is not a negative one.
    expect(backInLabel(new Date(NOW + 119_000).toISOString(), NOW)).toBe('back in 2m')
    expect(left(0)).toBe('back in 1m')
    expect(left(-5)).toBe('back in 1m')
    expect(backInLabel(null, NOW)).toBeNull()
    expect(backInLabel(undefined, NOW)).toBeNull()
    expect(backInLabel('soon', NOW)).toBeNull()
  })
})

describe('a model that can be picked: its health in words, and its plans', () => {
  test('degraded and busy are said, with the plans, and can be picked', () => {
    const picked: string[] = []
    const { rows, details, disabled } = open(
      [model('a', { status: 'degraded' }), model('b', { status: 'busy', tiers: ['free', 'paid'] })],
      picked,
    )
    expect(details).toEqual(['degraded · pro', 'busy · free, paid'])
    expect(disabled).toEqual([false, false])
    fireEvent.click(rows[1]!)
    expect(picked).toEqual(['b'])
  })

  test('no other status is printed as the gateway sent it', () => {
    for (const status of ['available', null, undefined, 'quarantined', 'something_new'] as const) {
      const { details } = open([model('a', { status: status as ModelInfo['status'] })])
      expect(details).toEqual(['pro'])
      cleanup()
    }
  })

  test('a model with no plans has no detail line to say', () => {
    expect(open([model('a', { tiers: [] })]).details).toEqual([''])
  })
})

describe('what the picker prints', () => {
  test('none of the gateway’s codes or timestamps reaches the screen', () => {
    const { list } = open([
      model('a', { available: false, unavailable_reason: 'upgrade_required' }),
      model('b', { available: false, unavailable_reason: 'region_unavailable' }),
      model('c', { available: false, unavailable_reason: 'not_configured' }),
      model('d', { status: 'blurred', available_again_at: at(90) }),
      model('e', { status: 'blurred' }),
      model('f', { status: 'degraded' }),
    ])
    const text = list.textContent ?? ''
    for (const raw of ['upgrade_required', 'region_unavailable', 'not_configured', 'blurred', '2026-', 'T11:30']) {
      expect([raw, text.includes(raw)]).toEqual([raw, false])
    }
  })
})

describe('the helper the picker uses', () => {
  const detail = (over: Partial<ModelInfo>) => modelDetail(model('a', over), NOW)

  test('says what the picker shows, for a model by itself', () => {
    expect(detail({})).toBe('pro')
    expect(detail({ available: false, unavailable_reason: 'upgrade_required' })).toBe('needs a plan upgrade')
    expect(detail({ status: 'blurred', available_again_at: at(200) })).toBe('back in 3h 20m')
    expect(detail({ status: 'degraded', tiers: ['free'] })).toBe('degraded · free')
  })

  test('knows a model that is out of rotation, until its time has passed', () => {
    const blurred = (over: Partial<ModelInfo>) => isBlurred(model('a', { status: 'blurred', ...over }), NOW)
    expect(blurred({ available_again_at: at(1) })).toBe(true)
    expect(blurred({ available_again_at: null })).toBe(true)
    expect(blurred({ available_again_at: 'later' })).toBe(true)
    expect(blurred({ available_again_at: at(-1) })).toBe(false)
    expect(blurred({ available_again_at: at(0) })).toBe(false)
    expect(isBlurred(model('a', { status: 'degraded', available_again_at: at(5) }), NOW)).toBe(false)
    expect(isBlurred(model('a'), NOW)).toBe(false)
  })

  test('uses the time it is asked at, now by default', () => {
    setSystemTime(new Date(NOW))
    expect(modelDetail(model('a', { status: 'blurred', available_again_at: at(200) }))).toBe('back in 3h 20m')
  })
})
