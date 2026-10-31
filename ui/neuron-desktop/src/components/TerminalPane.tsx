import { useEffect, useRef, useState } from 'react'
import { FitAddon } from '@xterm/addon-fit'
import { Terminal } from '@xterm/xterm'
import '@xterm/xterm/css/xterm.css'

import { dark, fonts } from '@neuron/client'

import type { Platform, TerminalSession } from '../platform'

/** A shell in the workspace, in xterm.js. It starts when the pane first opens. */
export function TerminalPane({ platform }: { platform: Platform }) {
  const box = useRef<HTMLDivElement>(null)
  const [status, setStatus] = useState<string | null>('Starting the shell…')

  useEffect(() => {
    if (!box.current) return
    const term = new Terminal({
      fontFamily: fonts.mono,
      fontSize: 13,
      cursorBlink: true,
      theme: { background: dark.sunken, foreground: dark.text, cursor: dark.chip, selectionBackground: '#4169c355' },
    })
    const fit = new FitAddon()
    term.loadAddon(fit)
    term.open(box.current)
    fit.fit()
    let session: TerminalSession | null = null
    let disposed = false
    const cleanups: Array<() => void> = []
    platform.openTerminal(term.cols, term.rows).then(
      (opened) => {
        if (disposed) return opened.close()
        session = opened
        setStatus(null)
        cleanups.push(
          opened.onData((data) => term.write(data)),
          opened.onExit((code) => setStatus(`The shell exited${code === null ? '' : ` (code ${code})`}.`)),
        )
        const input = term.onData((data) => opened.write(data))
        cleanups.push(() => input.dispose())
      },
      (error: Error) => setStatus(error.message),
    )
    const observer = new ResizeObserver(() => {
      fit.fit()
      session?.resize(term.cols, term.rows)
    })
    observer.observe(box.current)
    return () => {
      disposed = true
      observer.disconnect()
      for (const cleanup of cleanups) cleanup()
      session?.close()
      term.dispose()
    }
  }, [platform])

  return (
    <div className="terminal-pane">
      {status && <div className="terminal-status muted">{status}</div>}
      <div className="terminal-host" ref={box} />
    </div>
  )
}
