import fs from 'fs'
import path from 'path'

import { describe, expect, test } from 'bun:test'

import { SLASH_COMMANDS } from '../slash-commands'

/**
 * The engine answers "… is not available in the full-screen UI yet" for every
 * command its `run_slash` has no arm for (rust/crates/rusty-claude-cli/src/
 * engine.rs). The menu must not offer one of those.
 */
describe('the engine commands the slash menu offers', () => {
  const engineSource = fs.readFileSync(
    path.join(import.meta.dir, '../../../../../rust/crates/rusty-claude-cli/src/engine.rs'),
    'utf8',
  )
  const runSlash = engineSource.slice(
    engineSource.indexOf('fn run_slash('),
    engineSource.indexOf('is not available in the full-screen UI yet'),
  )
  const handled = (name: string) => {
    // `/context` is answered before run_slash, by name.
    if (name === 'context') return engineSource.includes('"context" => self')
    const variant = name.charAt(0).toUpperCase() + name.slice(1)
    return runSlash.includes(`SlashCommand::${variant}`)
  }

  test('are all handled by the engine, none answers "not available"', () => {
    const offered = SLASH_COMMANDS.filter((command) => command.engineCommand).map(
      (command) => command.id,
    )
    expect(offered.length).toBeGreaterThan(0)
    expect(offered.filter((name) => !handled(name))).toEqual([])
  })
})
