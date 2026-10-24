// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the tests
// now cover the settings that remain; a saved agent mode reads as DEFAULT
// while the modes are not offered. See ui/neuron-tui/PORTING.md.
import fs from 'fs'
import os from 'os'
import path from 'path'

import { afterEach, describe, expect, spyOn, test } from 'bun:test'

import * as configDir from '../config-dir'
import {
  getSettingsPath,
  hasSubmittedFirstPrompt,
  loadModePreference,
  loadSettings,
  markFirstPromptSubmitted,
  saveModePreference,
} from '../settings'

let testConfigDir: string | undefined
let getConfigDirSpy: ReturnType<typeof spyOn> | undefined

afterEach(() => {
  getConfigDirSpy?.mockRestore()
  getConfigDirSpy = undefined
  if (testConfigDir) {
    fs.rmSync(testConfigDir, { recursive: true, force: true })
    testConfigDir = undefined
  }
})

const useTempConfigDir = () => {
  testConfigDir = fs.mkdtempSync(path.join(os.tmpdir(), 'neuron-tui-settings-test-'))
  getConfigDirSpy = spyOn(configDir, 'getConfigDir').mockReturnValue(
    testConfigDir,
  )
}

describe('settings', () => {
  test('a saved agent mode reads as DEFAULT: the engine ignores the modes', () => {
    useTempConfigDir()

    expect(loadModePreference()).toBe('DEFAULT')
    saveModePreference('MAX')
    expect(loadModePreference()).toBe('DEFAULT')
  })

  test('the first-prompt mark is kept by the allowlisting loader', () => {
    useTempConfigDir()

    expect(hasSubmittedFirstPrompt()).toBe(false)
    markFirstPromptSubmitted()
    expect(hasSubmittedFirstPrompt()).toBe(true)
  })

  test('keys that are no longer settings are dropped on load', () => {
    useTempConfigDir()
    fs.writeFileSync(
      getSettingsPath(),
      JSON.stringify({
        mode: 'FREE',
        adsEnabled: false,
        byokConnection: { id: 'connection-id', revision: 3 },
      }),
    )

    expect(loadSettings()).toEqual({})
  })
})
