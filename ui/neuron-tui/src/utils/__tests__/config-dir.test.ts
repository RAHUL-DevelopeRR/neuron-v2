// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
import os from 'os'
import path from 'path'

import { afterEach, describe, expect, test } from 'bun:test'

import { ensureCliTestEnv } from '../../__tests__/test-utils'

ensureCliTestEnv()

const { getConfigDir } = await import('../config-dir')

const originalConfigDir = process.env.NEURON_TUI_CONFIG_DIR

afterEach(() => {
  if (originalConfigDir === undefined) {
    delete process.env.NEURON_TUI_CONFIG_DIR
  } else {
    process.env.NEURON_TUI_CONFIG_DIR = originalConfigDir
  }
})

describe('getConfigDir', () => {
  test('uses an absolute NEURON_TUI_CONFIG_DIR override', () => {
    const isolatedDir = path.join(os.tmpdir(), 'neuron-tui-cli-config-test')
    process.env.NEURON_TUI_CONFIG_DIR = isolatedDir

    expect(getConfigDir()).toBe(isolatedDir)
  })

  test('rejects a relative NEURON_TUI_CONFIG_DIR override', () => {
    process.env.NEURON_TUI_CONFIG_DIR = 'relative-settings'

    expect(() => getConfigDir()).toThrow(
      'NEURON_TUI_CONFIG_DIR must be an absolute path',
    )
  })
})
