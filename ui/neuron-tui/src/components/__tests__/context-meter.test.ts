import { describe, expect, test } from 'bun:test'

import { clampPercent, displayPercent, meterCells, meterTone } from '../context-meter'

const chars = (percent: number, cells = 10, mode: 'unicode' | 'console' = 'unicode') =>
  meterCells(percent, cells, mode)
    .map((cell) => cell.char)
    .join('')

describe('the context meter', () => {
  test('cells fill by tenths, with partly used cells', () => {
    expect(chars(0)).toBe('○○○○○○○○○○')
    expect(chars(0.4)).toBe('◔○○○○○○○○○')
    expect(chars(5)).toBe('◑○○○○○○○○○')
    expect(chars(10)).toBe('●○○○○○○○○○')
    expect(chars(15)).toBe('●◑○○○○○○○○')
    expect(chars(35)).toBe('●●●◑○○○○○○')
    expect(chars(79)).toBe('●●●●●●●◕○○')
    expect(chars(80)).toBe('●●●●●●●●○○')
    expect(chars(95)).toBe('●●●●●●●●●◑')
    expect(chars(100)).toBe('●●●●●●●●●●')
    expect(chars(999)).toBe('●●●●●●●●●●')
    expect(chars(Number.NaN)).toBe('○○○○○○○○○○')
  })

  test('five cells are worth a fifth each', () => {
    expect(chars(35, 5)).toBe('●◕○○○')
    expect(chars(10, 5)).toBe('◑○○○○')
    expect(chars(0, 0)).toBe('')
  })

  test('console fonts round each cell to full or empty', () => {
    expect(chars(35, 10, 'console')).toBe('●●●●○○○○○○')
    expect(chars(0.4, 10, 'console')).toBe('○○○○○○○○○○')
    expect(chars(35, 10, 'console')).not.toMatch(/[◔◑◕]/)
  })

  test('the tone follows the fill: room, half, compaction, nearly full', () => {
    expect(meterTone(0)).toBe('success')
    expect(meterTone(49)).toBe('success')
    expect(meterTone(50)).toBe('foreground')
    expect(meterTone(79)).toBe('foreground')
    expect(meterTone(80)).toBe('warning')
    expect(meterTone(95)).toBe('error')
    expect(meterTone(999)).toBe('error')
    expect(meterTone(Number.NaN)).toBe('success')
  })

  test('filled cells take the tone; the empty cell where compaction starts is marked', () => {
    const cells = meterCells(35, 10, 'unicode')
    expect(cells[0]!.tone).toBe('success')
    expect(cells[5]!.tone).toBe('muted')
    expect(cells[8]!.tone).toBe('border')
    expect(meterCells(35, 5, 'unicode')[4]!.tone).toBe('border')
    expect(meterCells(90, 10, 'unicode')[8]!.tone).toBe('warning')
  })

  test('the percentage never reads 0% while anything is used, and is clamped', () => {
    expect(displayPercent(0)).toBe(0)
    expect(displayPercent(0.4)).toBe(1)
    expect(displayPercent(35.4)).toBe(35)
    expect(displayPercent(999)).toBe(100)
    expect(clampPercent(-5)).toBe(0)
    expect(clampPercent(Number.POSITIVE_INFINITY)).toBe(0)
  })
})
