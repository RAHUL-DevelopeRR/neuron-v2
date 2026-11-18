import { afterEach, expect, test } from 'bun:test'
import { cleanup, render } from '@testing-library/react'

import { TerminalPane } from '../src/components/TerminalPane'
import type { TerminalSession } from '../src/platform'
import { FakeEngine, fakePlatform } from './fake-platform'

afterEach(cleanup)

const session = () => {
  const closed: number[] = []
  const exits: Array<(code: number | null) => void> = []
  const opened: TerminalSession = {
    write: () => {},
    resize: () => {},
    close: () => void closed.push(1),
    onData: () => () => {},
    onExit: (listener) => {
      exits.push(listener)
      return () => {}
    },
  }
  return { opened, closed, exits }
}

const pause = () => new Promise((resolve) => setTimeout(resolve, 20))

test('a shell that opens after the pane is gone is closed once, and a late exit sets nothing', async () => {
  const errors: unknown[] = []
  const original = console.error
  console.error = (...args: unknown[]) => void errors.push(args)
  try {
    const platform = fakePlatform(new FakeEngine())
    const late = session()
    let open: (value: TerminalSession) => void = () => {}
    platform.openTerminal = () => new Promise<TerminalSession>((resolve) => (open = resolve))
    const view = render(<TerminalPane platform={platform} folder="/work/app" />)
    view.unmount()
    open(late.opened)
    await pause()
    expect(late.closed).toEqual([1])

    // A shell that was open: closed once on unmount; its exit afterwards sets no status.
    const live = session()
    platform.openTerminal = async () => live.opened
    const second = render(<TerminalPane platform={platform} folder="/work/app" />)
    await pause()
    second.unmount()
    for (const exit of live.exits) exit(0)
    expect(live.closed).toEqual([1])
    expect(errors).toEqual([])
  } finally {
    console.error = original
  }
})
