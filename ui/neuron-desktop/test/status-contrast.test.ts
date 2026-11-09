/**
 * The status labels of the Extensions list are small text (10.5px, upper case), so they need 4.5:1 against what
 * they sit on, in both themes. axe cannot say so here: happy-dom has no layout and no colour, so the suite runs
 * it with color-contrast off. This reads the real colour tokens in styles.css and does the sum itself.
 */
import { describe, expect, test } from 'bun:test'
import fs from 'fs'
import path from 'path'

const CSS = fs.readFileSync(path.resolve(import.meta.dir, '../src/styles.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')

/** What the rules whose selector list has `selector` declare, a later rule winning over an earlier one. */
function declared(selector: string): Record<string, string> {
  const found: Record<string, string> = {}
  for (const rule of CSS.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    if (!rule[1]!.split(',').some((part) => part.trim() === selector)) continue
    for (const declaration of rule[2]!.split(';')) {
      const [name, ...value] = declaration.split(':')
      if (name && value.length > 0) found[name.trim()] = value.join(':').trim()
    }
  }
  return found
}

const dark = declared(':root')
// The light theme only says what differs; the rest is the dark theme's.
const light = { ...dark, ...declared(":root[data-theme='light']") }

type Rgb = [number, number, number]

/** `value` as a colour: a `var(--token)` is followed in `tokens`, a `#rrggbb` is read. */
function colour(value: string, tokens: Record<string, string>, depth = 0): Rgb {
  const text = value.trim()
  const reference = /^var\((--[\w-]+)\)$/.exec(text)
  if (reference) {
    const next = tokens[reference[1]!]
    if (next === undefined || depth > 8) throw new Error(`no colour for ${text}`)
    return colour(next, tokens, depth + 1)
  }
  const hex = /^#([0-9a-f]{6})$/i.exec(text)
  if (!hex) throw new Error(`not a plain colour: ${text}`)
  const digits = hex[1]!
  return [0, 2, 4].map((at) => parseInt(digits.slice(at, at + 2), 16)) as Rgb
}

/** WCAG 2 relative luminance. */
function luminance([red, green, blue]: Rgb): number {
  const linear = (channel: number) => {
    const value = channel / 255
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4
  }
  return 0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
}

/** WCAG 2 contrast ratio, 1 to 21. */
function ratio(first: Rgb, second: Rgb): number {
  const [lighter, darker] = [luminance(first), luminance(second)].sort((a, b) => b - a)
  return (lighter! + 0.05) / (darker! + 0.05)
}

describe('the contrast sum', () => {
  test('is right for known pairs', () => {
    expect(ratio([0, 0, 0], [255, 255, 255])).toBeCloseTo(21, 5)
    expect(ratio([255, 255, 255], [255, 255, 255])).toBeCloseTo(1, 5)
    // The amber on the light background, as measured in the review: about 2.0.
    expect(ratio(colour('#f0a028', {}), colour('#f7f7fb', {}))).toBeCloseTo(2.01, 1)
  })

  test('reads the tokens of each theme, the light one falling back to the dark one', () => {
    expect(colour('var(--bg)', dark)).toEqual([0x1a, 0x1a, 0x2e])
    expect(colour('var(--bg)', light)).toEqual([0xf7, 0xf7, 0xfb])
    // Not overridden in the light theme: it is the dark theme's.
    expect(colour('var(--orange)', light)).toEqual(colour('var(--orange)', dark))
  })
})

// What the labels can sit on: the page, a panel and a field. (A row's hover tint is a few percent of blue.)
const SURFACES = ['--bg', '--panel', '--sunken']
// The statuses that are amber or red: waiting on the user, still starting, or refused.
const STATUSES = ['starting', 'pending', 'needs_approval', 'needs_connect', 'blocked']

describe('the Extensions status labels can be read in both themes', () => {
  for (const [theme, tokens] of [
    ['dark', dark],
    ['light', light],
  ] as const) {
    for (const status of STATUSES) {
      test(`${status}, in the ${theme} theme, against every surface`, () => {
        const value = declared(`.status-${status}`).color
        expect(value).toBeDefined()
        for (const surface of SURFACES) {
          const measured = ratio(colour(value!, tokens), colour(`var(${surface})`, tokens))
          expect({ surface, colour: value, ratio: Number(measured.toFixed(2)), readable: measured >= 4.5 }).toMatchObject({ readable: true })
        }
      })
    }
  }
})
