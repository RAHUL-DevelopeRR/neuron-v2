// The screens before the chat with the mouse: every choice the keys make, a
// click makes too (start-screens.test.tsx has their keys and layout).
import { afterEach, beforeAll, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import {
  setNeuronEngineClientForTesting,
  type NeuronEngineClient,
} from '../../engine/neuron-engine-client'
import { initializeThemeStore } from '../../hooks/use-theme'
import { useEngineStore } from '../../state/engine-store'
import { OnboardingSignIn } from '../onboarding-sign-in'
import { resetOnboardingSignInForTesting, startOnboardingSignIn } from '../start-gate'
import { TrustScreen } from '../trust-screen'

import type { UiEvent } from '../../engine/protocol'

import type { TrustedMcpApproval } from '../../engine/engine-session'
import type { TrustPreviewEvent } from '../../engine/protocol'
import type { TrustStep } from '../../state/start-gate'

const CWD = '/repo'
const PREVIEW: TrustPreviewEvent = {
  type: 'trust_preview',
  cwd: CWD,
  root: CWD,
  is_git_root: true,
  session_only: false,
  trusted: false,
  settings_files: [`${CWD}/.neuron/settings.json`],
  hooks: [],
  mcp_servers: [
    { name: 'postgres-tools', transport: 'stdio', target: 'postgres-mcp', source: `${CWD}/.neuron/settings.json` },
    { name: 'docs-search', transport: 'http', target: 'docs.example', source: `${CWD}/.neuron/settings.json` },
  ],
  permission_rules: [],
  other_settings: [],
}

let cleanup: (() => void) | undefined

beforeAll(() => {
  initializeThemeStore()
})

afterEach(() => {
  cleanup?.()
  cleanup = undefined
  useEngineStore.getState().reset()
})

async function mount(element: React.ReactNode, width = 100, height = 40) {
  const setup = await createTestRenderer({ width, height })
  const root = createRoot(setup.renderer)
  cleanup = () => {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
  const settle = async () => {
    await setup.renderOnce()
    await new Promise((resolve) => setTimeout(resolve, 20))
    await setup.renderOnce()
  }
  flushSync(() => root.render(element))
  await settle()
  const at = (text: string) => {
    const lines = setup.captureCharFrame().split('\n')
    const y = lines.findIndex((line) => line.includes(text))
    if (y < 0) throw new Error(`no "${text}" on screen:\n${lines.join('\n')}`)
    return { x: lines[y]!.indexOf(text) + 1, y }
  }
  return {
    ...setup,
    frame: () => setup.captureCharFrame(),
    async click(text: string) {
      const { x, y } = at(text)
      await setup.mockMouse.click(x, y)
      await settle()
    },
    async hover(text: string) {
      const { x, y } = at(text)
      await setup.mockMouse.moveTo(x, y)
      await settle()
    },
    background(text: string) {
      const { y } = at(text)
      const span = setup.captureSpans().lines[y]!.spans.find((s) => s.text.includes(text))
      return span?.bg.toString()
    },
  }
}

function trustScreen(step: TrustStep = { kind: 'asking' }) {
  const calls = { allow: [] as TrustedMcpApproval[][], quit: 0 }
  const element = (
    <TrustScreen
      cwd={CWD}
      preview={PREVIEW}
      step={step}
      width={100}
      onAllow={(approved) => calls.allow.push(approved)}
      onQuit={() => calls.quit++}
    />
  )
  return { calls, element }
}

describe('the trust screen with the mouse', () => {
  test('a click on Allow allows, a click on Quit quits', async () => {
    const trust = trustScreen()
    const ui = await mount(trust.element)
    await ui.click('Allow Neuron in this folder')
    expect(trust.calls.allow).toEqual([[]])
    await ui.click('2  Quit')
    expect(trust.calls.quit).toBe(1)
  })

  test('a click on a server row ticks it and Allow approves it', async () => {
    const trust = trustScreen()
    const ui = await mount(trust.element)
    await ui.click('] docs-search')
    expect(ui.frame()).toContain('[x] docs-search')
    expect(ui.frame()).toContain('[ ] postgres-tools')
    await ui.click('] postgres-tools')
    await ui.click('] postgres-tools')
    expect(ui.frame()).toContain('[ ] postgres-tools')
    await ui.click('Allow Neuron in this folder')
    expect(trust.calls.allow).toEqual([[{ name: 'docs-search' }]])
  })

  test('while trust is saved a click does nothing but Quit', async () => {
    const trust = trustScreen({ kind: 'saving' })
    const ui = await mount(trust.element)
    await ui.click('] docs-search')
    expect(ui.frame()).toContain('[ ] docs-search')
    await ui.click('Allow Neuron in this folder')
    expect(trust.calls.allow).toEqual([])
    await ui.click('2  Quit')
    expect(trust.calls.quit).toBe(1)
  })

  test('the choice under the pointer is shaded, without moving the keyboard choice', async () => {
    const trust = trustScreen()
    const ui = await mount(trust.element)
    const before = ui.background('2  Quit')
    await ui.hover('2  Quit')
    expect(ui.background('2  Quit')).not.toBe(before)
    // The marker stays on Allow: Enter still allows.
    expect(ui.frame()).toContain('› 1  Allow Neuron in this folder')
  })
})

describe("onboarding's sign-in with the mouse", () => {
  function onboarding() {
    const calls: string[] = []
    const element = (
      <OnboardingSignIn
        width={100}
        onSignIn={() => calls.push('sign_in')}
        onLater={() => calls.push('later')}
        onQuit={() => calls.push('quit')}
      />
    )
    return { calls, element }
  }

  test('a click on Sign in with Zero-X signs in', async () => {
    const screen = onboarding()
    const ui = await mount(screen.element)
    await ui.click('Sign in with Zero-X')
    expect(screen.calls).toEqual(['sign_in'])
  })

  test('each key in the hint is a click target: sign in, later, quit', async () => {
    const screen = onboarding()
    const ui = await mount(screen.element)
    await ui.click('Enter to sign in')
    await ui.click('Esc for later')
    await ui.click('Ctrl+C to quit')
    expect(screen.calls).toEqual(['sign_in', 'later', 'quit'])
  })

  test('during a sign-in, a click on "esc cancel" stops it, as Esc does', async () => {
    let interrupts = 0
    setNeuronEngineClientForTesting({
      connection: 'ready',
      interrupt: () => {
        interrupts++
      },
    } as unknown as NeuronEngineClient)
    try {
      useEngineStore.setState({
        signIn: {
          type: 'sign_in_code',
          url: 'https://zero-x.live/device',
          url_complete: null,
          user_code: 'ABCD-1234',
          expires_in_secs: 600,
          browser_opened: true,
          receivedAt: Date.now(),
          approved: false,
        },
      })
      const screen = onboarding()
      const ui = await mount(screen.element)
      expect(ui.frame()).toContain('ABCD-1234')
      await ui.click('esc cancel')
      expect(interrupts).toBe(1)
      expect(screen.calls).toEqual([])
    } finally {
      setNeuronEngineClientForTesting(null)
    }
  })

  describe('one sign-in at a time', () => {
    // An engine that counts `/login` and ends an exchange when told to.
    function fakeEngine() {
      const listeners = new Set<(event: UiEvent) => void>()
      const engine = {
        connection: 'ready',
        logins: 0,
        slash(command: string) {
          if (command === 'login') engine.logins++
        },
        subscribe(listener: (event: UiEvent) => void) {
          listeners.add(listener)
          return () => listeners.delete(listener)
        },
        onExit: () => () => {},
        interrupt: () => {},
        /** The exchange under way ends (signed in, failed or cancelled). */
        finish() {
          for (const listener of [...listeners]) listener({ type: 'turn_finished' })
        },
      }
      setNeuronEngineClientForTesting(engine as unknown as NeuronEngineClient)
      return engine
    }

    function signInScreen() {
      return (
        <OnboardingSignIn width={100} onSignIn={startOnboardingSignIn} onLater={() => {}} onQuit={() => {}} />
      )
    }

    const tick = () => new Promise((resolve) => setTimeout(resolve, 10))

    afterEach(() => {
      setNeuronEngineClientForTesting(null)
      resetOnboardingSignInForTesting()
    })

    test('a double click on Sign in with Zero-X sends one /login', async () => {
      const engine = fakeEngine()
      const ui = await mount(signInScreen())
      const lines = ui.frame().split('\n')
      const y = lines.findIndex((line) => line.includes('Sign in with Zero-X'))
      const x = lines[y]!.indexOf('Sign in with Zero-X') + 1
      await ui.mockMouse.doubleClick(x, y)
      await tick()
      expect(engine.logins).toBe(1)
      // The hint's key while it runs adds none either.
      await ui.click('Enter to sign in')
      await tick()
      expect(engine.logins).toBe(1)
      // A queued second /login would start here, as the first ends (signed
      // in, or cancelled with Esc): none does.
      engine.finish()
      await tick()
      await tick()
      expect(engine.logins).toBe(1)
    })

    test('once a sign-in has ended (failed or cancelled), the next click starts another', async () => {
      const engine = fakeEngine()
      const ui = await mount(signInScreen())
      await ui.click('Sign in with Zero-X')
      await tick()
      expect(engine.logins).toBe(1)
      engine.finish()
      await tick()
      await ui.click('Sign in with Zero-X')
      await tick()
      expect(engine.logins).toBe(2)
      engine.finish()
      await tick()
    })

    test('with no engine to ask, nothing is left under way', async () => {
      setNeuronEngineClientForTesting({
        connection: 'ready',
        slash: () => {
          throw new Error('not connected')
        },
        subscribe: () => () => {},
        onExit: () => () => {},
      } as unknown as NeuronEngineClient)
      startOnboardingSignIn()
      await tick()
      const engine = fakeEngine()
      startOnboardingSignIn()
      await tick()
      expect(engine.logins).toBe(1)
      engine.finish()
      await tick()
    })
  })

  test('the part under the pointer lights up', async () => {
    const screen = onboarding()
    const ui = await mount(screen.element)
    const lines = () => ui.captureSpans().lines
    const { y } = (() => {
      const all = ui.frame().split('\n')
      return { y: all.findIndex((line) => line.includes('Ctrl+C to quit')) }
    })()
    const style = () =>
      lines()[y]!.spans
        .filter((s) => s.text.includes('Ctrl+C'))
        .map((s) => `${s.fg.toString()}:${s.attributes}`)
        .join()
    const before = style()
    await ui.hover('Ctrl+C to quit')
    expect(style()).not.toBe(before)
  })
})
