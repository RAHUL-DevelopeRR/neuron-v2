import { describe, expect, test } from 'bun:test'

import {
  MAIN_SCREEN_RESET_SEQUENCES,
  TERMINAL_RESET_SEQUENCES,
} from '../terminal-reset-sequences'

describe('terminal reset sequences', () => {
  test('the first reset leaves the alternate screen', () => {
    expect(TERMINAL_RESET_SEQUENCES.startsWith('\x1b[?1049l')).toBe(true)
  })

  test('a later reset does not leave it again, which would move the cursor up', () => {
    expect(MAIN_SCREEN_RESET_SEQUENCES).not.toContain('\x1b[?1049l')
    expect(`\x1b[?1049l${MAIN_SCREEN_RESET_SEQUENCES}`).toBe(TERMINAL_RESET_SEQUENCES)
  })
})
