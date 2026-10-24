import { afterEach, describe, expect, test } from 'bun:test'

import {
  NEURON_TITLE,
  POP_TITLE,
  PUSH_TITLE,
  getCurrentTerminalTitle,
  reassertTerminalTitle,
  restoreTerminalTitle,
  saveTerminalTitle,
  setNeuronTitle,
  setTerminalTitle,
  setTerminalTitleHostForTesting,
  terminalTitleDrifted,
  type TerminalTitleHost,
} from '../terminal-title'

/** A console whose title the UI (OSC) and others (PowerShell, cmd.exe) set. */
function fakeHost(platform: NodeJS.Platform, initialTitle = 'Windows PowerShell') {
  const writes: string[] = []
  const state = { title: initialTitle as string | null, setCalls: [] as string[] }
  const host: TerminalTitleHost = {
    platform,
    write: (sequence) => {
      writes.push(sequence)
      const osc = /^\x1b\]0;(.*)\x07$/.exec(sequence)
      if (osc) state.title = osc[1]!
      return true
    },
    getConsoleTitle: () => (platform === 'win32' ? state.title : null),
    setConsoleTitle: (title) => {
      state.setCalls.push(title)
      state.title = title
      return true
    },
  }
  return { host, writes, state }
}

afterEach(() => setTerminalTitleHostForTesting(null))

describe('the window title', () => {
  test('Windows: the console title is saved, "Neuron" set at start, the original restored at exit', () => {
    const { host, state } = fakeHost('win32', 'Windows PowerShell')
    setTerminalTitleHostForTesting(host)

    saveTerminalTitle()
    setNeuronTitle()
    expect(state.title).toBe(NEURON_TITLE)
    expect(getCurrentTerminalTitle()).toBe('Neuron')

    setTerminalTitle('fix the failing test')
    expect(state.title).toBe('Neuron: fix the failing test')

    restoreTerminalTitle()
    expect(state.setCalls).toEqual(['Windows PowerShell'])
    expect(state.title).toBe('Windows PowerShell')
  })

  test('the UI title is set again when something else retitled the window', () => {
    const { host, state } = fakeHost('win32', 'cmd')
    setTerminalTitleHostForTesting(host)
    saveTerminalTitle()
    setTerminalTitle('run the tests')

    // The watchdog's PowerShell, then a bash tool call's cmd.exe.
    state.title = 'Windows PowerShell'
    expect(terminalTitleDrifted()).toBe(true)
    reassertTerminalTitle()
    expect(state.title).toBe('Neuron: run the tests')
    expect(terminalTitleDrifted()).toBe(false)

    state.title = 'npm test'
    reassertTerminalTitle()
    expect(state.title).toBe('Neuron: run the tests')
  })

  test('elsewhere the title is pushed on the title stack at start and popped at exit', () => {
    const { host, writes } = fakeHost('linux')
    setTerminalTitleHostForTesting(host)

    saveTerminalTitle()
    setNeuronTitle()
    restoreTerminalTitle()

    expect(writes[0]).toBe(PUSH_TITLE)
    expect(writes[1]).toBe('\x1b]0;Neuron\x07')
    expect(writes.at(-1)).toBe(POP_TITLE)
    // A title that can't be read back never counts as changed.
    expect(terminalTitleDrifted()).toBeNull()
  })

  test('restoring is once, and nothing sets a title after it', () => {
    const { host, state, writes } = fakeHost('win32', 'Original')
    setTerminalTitleHostForTesting(host)
    saveTerminalTitle()
    setNeuronTitle()

    restoreTerminalTitle()
    restoreTerminalTitle()
    const written = writes.length
    reassertTerminalTitle()
    setTerminalTitle('late')
    setNeuronTitle()

    expect(state.setCalls).toEqual(['Original'])
    expect(writes.length).toBe(written)
    expect(state.title).toBe('Original')
  })

  test('without a saved title the UI title is cleared, as before', () => {
    const { host, writes } = fakeHost('win32', null as unknown as string)
    setTerminalTitleHostForTesting(host)
    saveTerminalTitle()
    setNeuronTitle()
    restoreTerminalTitle()
    expect(writes.at(-1)).toBe('\x1b]0;\x07')
  })

  test('a prompt title is sanitized and cut to fit', () => {
    const { host, state } = fakeHost('win32')
    setTerminalTitleHostForTesting(host)
    setTerminalTitle('line one\nline two\x1b[31m')
    expect(state.title).toBe('Neuron: line one line two [31m')
    setTerminalTitle('x'.repeat(200))
    expect(state.title!.length).toBe(60)
    expect(state.title!.endsWith('…')).toBe(true)
    // An empty prompt keeps the current title.
    setTerminalTitle('   ')
    expect(state.title!.endsWith('…')).toBe(true)
  })
})
