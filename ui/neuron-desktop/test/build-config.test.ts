import { describe, expect, test } from 'bun:test'

import { isUpdaterSigned } from '../scripts/updater-signed'

describe('updater signing flag', () => {
  test('needs a real public key', () => {
    expect(isUpdaterSigned({ plugins: { updater: { pubkey: 'dW50cnVzdGVk' } } })).toBe(true)
    for (const pubkey of ['', '   ', '\n\t', undefined, 5]) expect(isUpdaterSigned({ plugins: { updater: { pubkey } } })).toBe(false)
    expect(isUpdaterSigned({})).toBe(false)
  })
})
