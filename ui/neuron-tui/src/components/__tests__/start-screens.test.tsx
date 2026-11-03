import { afterEach, beforeAll, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import { initializeThemeStore } from '../../hooks/use-theme'
import { useEngineStore } from '../../state/engine-store'
import { startScreen, useStartGate, type StartGateInput } from '../../state/start-gate'
import { OnboardingSignIn, onboardingKeyAction } from '../onboarding-sign-in'
import { ReportPanel } from '../report-panel'
import { TrustScreen, trustKeyAction } from '../trust-screen'

import type { ReportEvent, StatusEvent, TrustPreviewEvent } from '../../engine/protocol'
import type { GlyphMode } from '../../utils/glyphs'

const STATUS: StatusEvent = {
  type: 'status',
  model: 'auto',
  permission_mode: 'workspace-write',
  session_id: 'session-1',
  cwd: 'C:\\RAHUL\\PROJECTS _OF_Rahul\\zero-x.live',
  workspace_trusted: false,
  plan_mode: false,
  context_percent: 0,
  git: null,
  signed_in: true,
  sign_in_required: false,
} as StatusEvent

const SIGNED_OUT = { ...STATUS, session_id: '', signed_in: false, sign_in_required: true } as StatusEvent

const PREVIEW: TrustPreviewEvent = {
  type: 'trust_preview',
  cwd: STATUS.cwd,
  root: STATUS.cwd,
  is_git_root: true,
  session_only: false,
  trusted: false,
  settings_files: [`${STATUS.cwd}\\.neuron\\settings.json`],
  hooks: [
    { event: 'PreToolUse', command: 'npm run lint', source: `${STATUS.cwd}\\.neuron\\settings.json` },
    { event: 'PostToolUse', command: 'npm test', source: `${STATUS.cwd}\\.neuron\\settings.json` },
  ],
  mcp_servers: [
    { name: 'postgres-tools', transport: 'stdio', target: 'postgres-mcp', source: `${STATUS.cwd}\\.neuron\\settings.json` },
  ],
  permission_rules: [
    { effect: 'allow', rule: 'Bash(npm test)', source: `${STATUS.cwd}\\.neuron\\settings.json` },
    { effect: 'allow', rule: 'Bash(npm run lint)', source: `${STATUS.cwd}\\.neuron\\settings.json` },
    { effect: 'ask', rule: 'Bash(git push)', source: `${STATUS.cwd}\\.neuron\\settings.json` },
  ],
  other_settings: [],
}

const HOME_PREVIEW: TrustPreviewEvent = {
  ...PREVIEW,
  cwd: 'C:\\Users\\rahul',
  root: 'C:\\Users\\rahul',
  is_git_root: false,
  session_only: true,
  settings_files: [],
  hooks: [],
  mcp_servers: [],
  permission_rules: [],
}

const USAGE_REPORT: ReportEvent = {
  type: 'report',
  command: 'usage',
  title: 'Usage today',
  sections: [
    { heading: 'Pro plan', markdown: '- Requests: **37** of 1,000\n- Tokens: **412,300** of 5,000,000\n- Resets at 05:30 (UTC)' },
    { heading: 'Credit', markdown: '$4.20 left' },
  ],
  links: [
    { label: 'Dashboard', url: 'https://zero-x.live/dashboard' },
    { label: 'Plans', url: 'https://zero-x.live/pricing' },
  ],
}

beforeAll(() => {
  initializeThemeStore()
})

afterEach(() => {
  useEngineStore.getState().reset()
  useStartGate.getState().reset()
})

async function frameOf(element: React.ReactNode, width: number, height: number): Promise<string> {
  const setup = await createTestRenderer({ width, height })
  const root = createRoot(setup.renderer)
  flushSync(() => root.render(element))
  try {
    await setup.renderOnce()
    return setup
      .captureCharFrame()
      .split('\n')
      .map((line) => line.trimEnd())
      .filter((line) => line)
      .join('\n')
  } finally {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
}

const gate = (overrides: Partial<StartGateInput>): StartGateInput => ({
  connection: 'ready',
  status: STATUS,
  features: ['trust_workspace', 'trust_preview'],
  firstStatusSeen: true,
  onboardingSignIn: false,
  trustStep: { kind: 'asking' },
  ...overrides,
})

describe('what comes before the chat', () => {
  test('nothing runs before the first engine says how things stand', () => {
    expect(startScreen(gate({ firstStatusSeen: false, connection: 'starting', status: null }))).toBe('starting')
    // An engine that failed to start: the chat says why.
    expect(startScreen(gate({ firstStatusSeen: false, connection: 'exited', status: null }))).toBe('chat')
  })

  test('onboarding signs in first, then the folder is trusted, then the chat', () => {
    const signedOut = gate({ status: SIGNED_OUT, onboardingSignIn: true })
    expect(startScreen(signedOut)).toBe('sign_in')
    expect(startScreen({ ...signedOut, status: STATUS })).toBe('trust')
    expect(startScreen({ ...signedOut, status: { ...STATUS, workspace_trusted: true } })).toBe('chat')
    // "Not now" on the sign-in: the trust screen still comes.
    expect(startScreen({ ...signedOut, onboardingSignIn: false })).toBe('trust')
  })

  test('the trust screen stays up while the engine restarts with the folder settings', () => {
    expect(startScreen(gate({ trustStep: { kind: 'restarting' }, connection: 'starting', status: null }))).toBe('trust')
    expect(startScreen(gate({ trustStep: { kind: 'restarting' }, connection: 'exited', status: null }))).toBe('chat')
  })

  test('an engine without trust_workspace keeps the chat, with its untrusted footer', () => {
    expect(startScreen(gate({ features: [] }))).toBe('chat')
  })

  test('a later restart (a resumed chat) keeps the chat on screen', () => {
    expect(startScreen(gate({ connection: 'starting', status: null }))).toBe('chat')
  })

  test('the first status decides onboarding; later ones do not', () => {
    useStartGate.getState().noteStatus(SIGNED_OUT)
    expect(useStartGate.getState().onboardingSignIn).toBe(true)
    useStartGate.getState().reset()
    useStartGate.getState().noteStatus(STATUS)
    useStartGate.getState().noteStatus(SIGNED_OUT)
    expect(useStartGate.getState().onboardingSignIn).toBe(false)
  })
})

describe('the trust screen keys', () => {
  const key = (name: string, extra: Record<string, unknown> = {}) => ({ name, ctrl: false, sequence: name, ...extra })

  test('1 and 2 choose, the arrows move, Enter confirms, Esc and Ctrl-C quit', () => {
    expect(trustKeyAction(key('1'), 1)).toEqual({ type: 'choose', choice: 'allow' })
    expect(trustKeyAction(key('2'), 0)).toEqual({ type: 'choose', choice: 'quit' })
    expect(trustKeyAction(key('down'), 0)).toEqual({ type: 'select', index: 1 })
    // The arrows stop at the ends.
    expect(trustKeyAction(key('down'), 1)).toEqual({ type: 'select', index: 1 })
    expect(trustKeyAction(key('up'), 0)).toEqual({ type: 'select', index: 0 })
    expect(trustKeyAction(key('return', { sequence: '\r' }), 0)).toEqual({ type: 'choose', choice: 'allow' })
    expect(trustKeyAction(key('return', { sequence: '\r' }), 1)).toEqual({ type: 'choose', choice: 'quit' })
    expect(trustKeyAction(key('escape', { sequence: '\x1b' }), 0)).toEqual({ type: 'choose', choice: 'quit' })
    expect(trustKeyAction(key('c', { ctrl: true, sequence: '\x03' }), 0)).toEqual({ type: 'choose', choice: 'quit' })
    expect(trustKeyAction(key('3'), 0)).toEqual({ type: 'none' })
    expect(trustKeyAction(key('space', { sequence: ' ' }), 0)).toEqual({ type: 'none' })
  })
})

describe('the onboarding sign-in keys', () => {
  test('Enter signs in, Esc leaves it for later, Ctrl-C quits; a running sign-in keeps Esc', () => {
    expect(onboardingKeyAction({ name: 'return', ctrl: false }, false)).toBe('sign_in')
    expect(onboardingKeyAction({ name: 'escape', ctrl: false }, false)).toBe('later')
    expect(onboardingKeyAction({ name: 'c', ctrl: true }, false)).toBe('quit')
    expect(onboardingKeyAction({ name: 'escape', ctrl: false }, true)).toBe('none')
    expect(onboardingKeyAction({ name: 'c', ctrl: true }, true)).toBe('quit')
  })
})

const noop = () => {}

describe('the screens before the chat render plainly', () => {
  for (const [width, height] of [
    [80, 24],
    [200, 50],
  ] as const) {
    test(`the trust screen at ${width}x${height}`, async () => {
      const frame = await frameOf(
        <TrustScreen cwd={STATUS.cwd} preview={PREVIEW} step={{ kind: 'asking' }} width={width} onAllow={noop} onQuit={noop} />,
        width,
        height,
      )
      expect(frame).toContain('Before Neuron works here')
      expect(frame).toContain('(git root: this folder)')
      expect(frame).toContain('• 2 hooks that run commands around tool calls')
      expect(frame).toContain('• 1 MCP server: postgres-tools')
      expect(frame).toContain('• 3 permission rules from .neuron/settings.json')
      expect(frame).toContain('› 1  Allow Neuron in this folder')
      expect(frame).toContain('↑↓ or 1–2 to choose · Enter to confirm · Esc to quit')
      expect(frame).toMatchSnapshot()
    })

    test(`onboarding's sign-in at ${width}x${height}`, async () => {
      useEngineStore.getState().setStatus(SIGNED_OUT)
      const frame = await frameOf(
        <OnboardingSignIn width={width} onSignIn={noop} onLater={noop} onQuit={noop} />,
        width,
        height,
      )
      expect(frame).toContain('Welcome to Neuron')
      expect(frame).toContain('› Sign in with Zero-X')
      expect(frame).toMatchSnapshot()
    })

    test(`a /usage report at ${width}x${height}`, async () => {
      const frame = await frameOf(
        <ReportPanel report={USAGE_REPORT} width={width} maxHeight={Math.floor(height / 2)} />,
        width,
        height,
      )
      expect(frame).toContain('Usage today')
      expect(frame).toContain('Requests: 37 of 1,000')
      expect(frame).toContain('https://zero-x.live/dashboard')
      expect(frame).toContain('esc close')
      expect(frame).toMatchSnapshot()
    })
  }

  test("onboarding's sign-in when Zero-X can't be reached: Enter tries again", async () => {
    useEngineStore.getState().setStatus(SIGNED_OUT)
    useEngineStore.getState().setSignInProblem('Could not reach Zero-X: connection refused')
    const frame = await frameOf(
      <OnboardingSignIn width={80} onSignIn={noop} onLater={noop} onQuit={noop} />,
      80,
      24,
    )
    expect(frame).toContain('Neuron could not start a sign-in with Zero-X.')
    expect(frame).toContain('connection refused')
    expect(frame).toContain('press Enter to try again')
  })

  test('the home folder is trusted for this session only, and adds nothing', async () => {
    const frame = await frameOf(
      <TrustScreen cwd={HOME_PREVIEW.cwd} preview={HOME_PREVIEW} step={{ kind: 'asking' }} width={80} onAllow={noop} onQuit={noop} />,
      80,
      24,
    )
    expect(frame).toContain('for this session only')
    expect(frame).toContain('does not turn on any hooks, MCP servers or permission rules')
    expect(frame).not.toContain('git root')
  })

  test('while the engine restarts, and when trust could not be saved', async () => {
    const restarting = await frameOf(
      <TrustScreen cwd={STATUS.cwd} preview={PREVIEW} step={{ kind: 'restarting' }} width={80} onAllow={noop} onQuit={noop} />,
      80,
      24,
    )
    expect(restarting).toContain('Starting Neuron with this folder’s settings…')
    expect(restarting).not.toContain('Enter to confirm')
    const failed = await frameOf(
      <TrustScreen
        cwd={STATUS.cwd}
        preview={PREVIEW}
        step={{ kind: 'failed', error: 'could not trust the folder: access denied' }}
        width={80}
        onAllow={noop}
        onQuit={noop}
      />,
      80,
      24,
    )
    expect(failed).toContain('Neuron could not save that this folder is trusted.')
    expect(failed).toContain('access denied')
  })

  for (const mode of ['console'] as GlyphMode[]) {
    test(`in a classic console (ASCII mode) at 80x24`, async () => {
      const trust = await frameOf(
        <TrustScreen
          cwd={STATUS.cwd}
          preview={PREVIEW}
          step={{ kind: 'asking' }}
          width={80}
          onAllow={noop}
          onQuit={noop}
          glyphMode={mode}
        />,
        80,
        24,
      )
      expect(trust).toContain('> 1  Allow Neuron in this folder')
      // A classic console showed the arrows as "??".
      expect(trust).toContain('Up/Down or 1–2 to choose · Enter to confirm · Esc to quit')
      expect(trust).not.toContain('↑')
      expect(trust).toMatchSnapshot()
      useEngineStore.getState().setStatus(SIGNED_OUT)
      const signIn = await frameOf(
        <OnboardingSignIn width={80} onSignIn={noop} onLater={noop} onQuit={noop} glyphMode={mode} />,
        80,
        24,
      )
      expect(signIn).toContain('> Sign in with Zero-X')
      expect(signIn).toMatchSnapshot()
    })
  }
})
