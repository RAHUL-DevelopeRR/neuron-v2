import { describe, expect, test } from 'bun:test'

import {
  BRAND,
  LOGO,
  LOGO_LETTER_COLORS,
  LOGO_SMALL,
  SHADOW_CHARS,
  getSheenColor,
  logoLetterColor,
  logoShadowColor,
  parseLogoLines,
} from '../logo'

const brightness = (hex: string) =>
  [1, 3, 5].reduce((sum, at) => sum + parseInt(hex.slice(at, at + 2), 16), 0)

describe('logo colours', () => {
  test('the letters of Neuron are NeuronCLI\'s: N blue, e red, u and r orange, o and n green', () => {
    expect(LOGO_LETTER_COLORS).toEqual([
      BRAND.blue,
      BRAND.red,
      BRAND.orange,
      BRAND.orange,
      BRAND.green,
      BRAND.green,
    ])
  })

  test('each column of the block logo takes the colour of the letter it draws', () => {
    const small = parseLogoLines(LOGO_SMALL)
    parseLogoLines(LOGO).forEach((line, index) => {
      // The first and the last letters are both the N of the small logo.
      expect(line.slice(0, 11)).toBe(small[index])
      expect(` ${line.slice(45)}`).toBe(small[index])
    })
    const columns = [0, 10, 11, 18, 19, 27, 28, 35, 36, 44, 45, 54]
    expect(columns.map(logoLetterColor)).toEqual([
      BRAND.blue, BRAND.blue,
      BRAND.red, BRAND.red,
      BRAND.orange, BRAND.orange,
      BRAND.orange, BRAND.orange,
      BRAND.green, BRAND.green,
      BRAND.green, BRAND.green,
    ])
  })

  test('a shadow is its letter dimmed toward the background', () => {
    expect(brightness(logoShadowColor(0, 'dark'))).toBeLessThan(brightness(BRAND.blue))
    expect(brightness(logoShadowColor(0, 'light'))).toBeGreaterThan(brightness(BRAND.blue))
  })

  test('blocks keep their letter colour and the sheen lights shadows in it', () => {
    expect(getSheenColor('█', 12, 0, SHADOW_CHARS, 'dark')).toBe(BRAND.red)
    // Forward: lit at or behind the sheen, dimmed ahead of it.
    expect(getSheenColor('╗', 12, 20, SHADOW_CHARS, 'dark')).toBe(BRAND.red)
    expect(getSheenColor('╗', 30, 20, SHADOW_CHARS, 'dark')).toBe(logoShadowColor(30, 'dark'))
    // Reverse: dimmed again behind the sheen, still lit ahead of it.
    expect(getSheenColor('╗', 12, 20, SHADOW_CHARS, 'dark', true)).toBe(logoShadowColor(12, 'dark'))
    expect(getSheenColor('╗', 30, 20, SHADOW_CHARS, 'dark', true)).toBe(BRAND.orange)
  })
})
