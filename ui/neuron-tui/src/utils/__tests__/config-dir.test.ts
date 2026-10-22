// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
import os from 'os'
import path from 'path'

import { afterEach, beforeEach, describe, expect, test } from 'bun:test'

import { ensureCliTestEnv } from '../../__tests__/test-utils'

ensureCliTestEnv()

const { getConfigDir } = await import('../config-dir')

const original = {
  NEURON_TUI_CONFIG_DIR: process.env.NEURON_TUI_CONFIG_DIR,
  NEURON_HOME: process.env.NEURON_HOME,
}

const restore = (name: keyof typeof original) => {
  if (original[name] === undefined) {
    delete process.env[name]
  } else {
    process.env[name] = original[name]
  }
}

beforeEach(() => {
  delete process.env.NEURON_TUI_CONFIG_DIR
  delete process.env.NEURON_HOME
})

afterEach(() => {
  restore('NEURON_TUI_CONFIG_DIR')
  restore('NEURON_HOME')
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

  test('keeps the UI files in NEURON_HOME/tui, beside the engine’s', () => {
    const neuronHome = path.join(os.tmpdir(), 'neuron-home-test')
    process.env.NEURON_HOME = neuronHome

    expect(getConfigDir()).toBe(path.join(neuronHome, 'tui'))
  })

  test('NEURON_TUI_CONFIG_DIR wins over NEURON_HOME', () => {
    const isolatedDir = path.join(os.tmpdir(), 'neuron-tui-cli-config-test')
    process.env.NEURON_TUI_CONFIG_DIR = isolatedDir
    process.env.NEURON_HOME = path.join(os.tmpdir(), 'neuron-home-test')

    expect(getConfigDir()).toBe(isolatedDir)
  })

  test('defaults to ~/.neuron/tui without either', () => {
    expect(getConfigDir()).toBe(path.join(os.homedir(), '.neuron', 'tui'))
  })

  test('an empty or relative NEURON_HOME is not used', () => {
    const fallback = path.join(os.homedir(), '.neuron', 'tui')
    process.env.NEURON_HOME = '   '
    expect(getConfigDir()).toBe(fallback)
    // Never a folder relative to the current project.
    process.env.NEURON_HOME = 'neuron-home'
    expect(getConfigDir()).toBe(fallback)
  })
})
