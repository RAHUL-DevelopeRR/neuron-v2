import fs from 'fs'
import os from 'os'
import path from 'path'

import { afterEach, describe, expect, test } from 'bun:test'

import {
  PARENT_ARMED,
  PARENT_DISARMED,
  getTerminalWatchdogDiagnostics,
  startTerminalWatchdog,
  stopTerminalWatchdog,
} from '../terminal-watchdog'

const savedEnv = process.env.NEURON_TUI_DISARM_FILE
const savedTty = process.stdout.isTTY
const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'neuron-tui-parent-watchdog-'))

afterEach(() => {
  stopTerminalWatchdog()
  if (savedEnv === undefined) delete process.env.NEURON_TUI_DISARM_FILE
  else process.env.NEURON_TUI_DISARM_FILE = savedEnv
  process.stdout.isTTY = savedTty
})

describe('neuron as the watchdog (NEURON_TUI_DISARM_FILE)', () => {
  test('the UI starts no watchdog of its own: it says armed, then disarmed after its reset', () => {
    const file = path.join(dir, 'state-1')
    process.env.NEURON_TUI_DISARM_FILE = file
    process.stdout.isTTY = true

    startTerminalWatchdog()
    expect(fs.readFileSync(file, 'utf8')).toBe(PARENT_ARMED)
    // No child process: neuron, the parent, waits for the UI anyway.
    expect(getTerminalWatchdogDiagnostics()).toEqual({ armed: true, external: true, pid: undefined })

    stopTerminalWatchdog()
    expect(fs.readFileSync(file, 'utf8')).toBe(PARENT_DISARMED)
    expect(getTerminalWatchdogDiagnostics().armed).toBe(false)
  })

  test('off a terminal there is nothing to reset, so nothing is armed', () => {
    const file = path.join(dir, 'state-2')
    process.env.NEURON_TUI_DISARM_FILE = file
    process.stdout.isTTY = false

    startTerminalWatchdog()
    expect(fs.existsSync(file)).toBe(false)
  })
})
