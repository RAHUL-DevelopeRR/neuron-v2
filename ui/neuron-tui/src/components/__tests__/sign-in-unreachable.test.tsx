import { afterEach, beforeAll, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import { isUnreachableSignIn } from '../../engine/engine-session'
import { initializeThemeStore } from '../../hooks/use-theme'
import { useEngineStore } from '../../state/engine-store'
import { SignInPanel, signInView } from '../sign-in-panel'

import type { StatusEvent } from '../../engine/protocol'

const SIGNED_OUT: StatusEvent = {
  type: 'status',
  model: 'auto',
  permission_mode: 'workspace-write',
  session_id: '',
  cwd: '/work',
  workspace_trusted: true,
  plan_mode: false,
  context_percent: 0,
  git: null,
  signed_in: false,
  sign_in_required: true,
} as StatusEvent

const UNREACHABLE = 'Could not reach Zero-X: error sending request for url (https://zero-x.live/auth/device/code)'

beforeAll(() => {
  initializeThemeStore()
})

afterEach(() => {
  useEngineStore.getState().reset()
})

describe("the sign-in panel's Can't reach Zero-X state", () => {
  test("the engine's unreachable, no-device-sign-in and server-error answers are it", () => {
    expect(isUnreachableSignIn(UNREACHABLE)).toBe(true)
    expect(
      isUnreachableSignIn(
        'This Zero-X gateway does not support device sign-in yet. Use `neuron auth login --with-token`.',
      ),
    ).toBe(true)
    expect(
      isUnreachableSignIn(
        'Zero-X answered the sign-in request with an error (HTTP 503). Try again in a moment.',
      ),
    ).toBe(true)
    // These reached Zero-X.
    expect(isUnreachableSignIn('Sign-in was denied in the browser.')).toBe(false)
    expect(isUnreachableSignIn('The sign-in code expired. Run `neuron login` again.')).toBe(false)
    expect(
      isUnreachableSignIn('Zero-X answered the sign-in request with an error (HTTP 400): bad client.'),
    ).toBe(false)
  })

  test('it shows while signed out, and a new code or a sign-in clears it', () => {
    expect(signInView(SIGNED_OUT, null, 0, UNREACHABLE)).toEqual({
      kind: 'unreachable',
      reason: UNREACHABLE,
    })
    const store = useEngineStore.getState()
    store.setSignInProblem(UNREACHABLE)
    store.setSignIn({
      type: 'sign_in_code',
      user_code: 'BCDF-GHJK',
      url: 'https://zero-x.live/device',
      url_complete: null,
      expires_in_secs: 600,
      browser_opened: false,
    } as never)
    expect(useEngineStore.getState().signInProblem).toBeNull()

    store.setSignInProblem(UNREACHABLE)
    store.setStatus({ ...SIGNED_OUT, signed_in: true, sign_in_required: false, session_id: 's' })
    expect(useEngineStore.getState().signInProblem).toBeNull()
  })

  for (const [width, height] of [
    [80, 24],
    [200, 50],
  ] as const) {
    test(`it reads plainly at ${width}x${height}`, async () => {
      useEngineStore.getState().setStatus(SIGNED_OUT)
      useEngineStore.getState().setSignInProblem(UNREACHABLE)
      const setup = await createTestRenderer({ width, height })
      const root = createRoot(setup.renderer)
      flushSync(() => root.render(<SignInPanel width={width - 2} />))
      try {
        await setup.renderOnce()
        const frame = setup
          .captureCharFrame()
          .split('\n')
          .map((line) => line.trimEnd())
          .filter((line) => line)
          .join('\n')
        expect(frame).toContain("Can't reach Zero-X")
        expect(frame).toContain('/login')
        expect(frame).toMatchSnapshot()
      } finally {
        flushSync(() => root.unmount())
        setup.renderer.destroy()
      }
    })
  }
})
