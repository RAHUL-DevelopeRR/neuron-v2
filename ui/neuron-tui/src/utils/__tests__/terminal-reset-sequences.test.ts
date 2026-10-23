import fs from 'fs'
import path from 'path'

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

  test('neuron resets the terminal with the same sequences after a UI that died', () => {
    // full_screen.rs writes them when the UI exits still "armed".
    const source = fs.readFileSync(
      path.join(import.meta.dir, '../../../../../rust/crates/rusty-claude-cli/src/full_screen.rs'),
      'utf8',
    )
    const literal = /TERMINAL_RESET_SEQUENCES: &str =\s*"([^"]*)"/.exec(source)?.[1]
    expect(literal?.replace(/\\x1b/g, '\x1b')).toBe(TERMINAL_RESET_SEQUENCES)
  })
})
