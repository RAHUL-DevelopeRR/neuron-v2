import { afterEach, beforeEach, describe, expect, test } from 'bun:test'

import { STATUS_BAR_STATUS } from '../../testing/status-bar-fixtures'
import { useEngineStore } from '../../state/engine-store'
import { resolveChatKeyboardAction, createDefaultChatKeyboardState } from '../../utils/keyboard-actions'
import { handleEngineEvent } from '../engine-session'
import {
  setNeuronEngineClientForTesting,
  type NeuronEngineClient,
} from '../neuron-engine-client'
import {
  cyclePermissionChoice,
  nextPermissionChoice,
  permissionChoices,
  setPermissionChoice,
  setReasoningEffort,
  switchBranch,
} from '../status-bar-actions'
import { FEATURE_STATUS_BAR, parseUiEventLine, type EngineCommand } from '../protocol'
import {
  branchRows,
  permissionRows,
  thinkRows,
  workspacePickAction,
  workspaceRows,
} from '../../components/status-pickers'
import { statusBarLine } from '../../utils/status-bar-parts'
import { resolveChoiceKey } from '../../components/choice-panel'
import { helpSections } from '../../components/help-banner'
import { reasoningLevels } from '../../components/model-picker'

import type { KeyEvent } from '@opentui/core'

const status = (overrides = {}) => ({ ...STATUS_BAR_STATUS, ...overrides })

describe('the permission cycle', () => {
  test('Read-only, Edit workspace, Plan, and around; Full access only when allowed', () => {
    expect(nextPermissionChoice(status({ permission_mode: 'read-only' }))).toBe('workspace-write')
    expect(nextPermissionChoice(status({ permission_mode: 'workspace-write' }))).toBe('plan')
    expect(nextPermissionChoice(status({ plan_mode: true }))).toBe('read-only')
    const allowed = status({ permission_mode: 'workspace-write', full_access_allowed: true })
    expect(nextPermissionChoice(allowed)).toBe('danger-full-access')
    expect(nextPermissionChoice({ ...allowed, permission_mode: 'danger-full-access' })).toBe('plan')
    expect(permissionChoices(status()).map((option) => option.id)).toEqual([
      'read-only',
      'workspace-write',
      'plan',
    ])
    // An older engine that does not say, already in full access, keeps it listed.
    expect(
      permissionChoices(status({ permission_mode: 'danger-full-access', full_access_allowed: undefined })).map(
        (option) => option.id,
      ),
    ).toContain('danger-full-access')
    expect(nextPermissionChoice(null)).toBeNull()
  })
})

describe('keys', () => {
  const key = (overrides: Partial<KeyEvent>) =>
    ({ name: '', sequence: '', ctrl: false, meta: false, shift: false, option: false, ...overrides }) as KeyEvent
  const state = createDefaultChatKeyboardState()

  test('Shift+Tab cycles the permission mode, but not over a menu', () => {
    const shiftTab = key({ name: 'tab', shift: true })
    expect(resolveChatKeyboardAction(shiftTab, state)).toEqual({ type: 'cycle-permission' })
    expect(
      resolveChatKeyboardAction(shiftTab, { ...state, slashMenuActive: true, slashMatchesLength: 2, inputValue: '/' }),
    ).toEqual({ type: 'slash-menu-complete' })
    expect(
      resolveChatKeyboardAction(shiftTab, { ...state, mentionMenuActive: true, totalMentionMatches: 2 }),
    ).toEqual({ type: 'mention-menu-shift-tab' })
    expect(resolveChatKeyboardAction(shiftTab, { ...state, inputMode: 'help' })).not.toEqual({
      type: 'cycle-permission',
    })
  })

  test('Alt+M/T/W/P/C open the pickers, also with a draft', () => {
    for (const [name, picker] of [
      ['m', 'model'],
      ['t', 'think'],
      ['w', 'workspace'],
      ['p', 'permission'],
      ['c', 'context'],
    ] as const) {
      expect(resolveChatKeyboardAction(key({ name, meta: true }), { ...state, inputValue: 'draft' })).toEqual({
        type: 'open-status-picker',
        picker,
      })
      expect(resolveChatKeyboardAction(key({ name, option: true }), state)).toEqual({
        type: 'open-status-picker',
        picker,
      })
    }
    // Ctrl+T stays expand/collapse; a plain letter is typing.
    expect(resolveChatKeyboardAction(key({ name: 't', ctrl: true }), state)).toEqual({ type: 'toggle-all' })
    expect(resolveChatKeyboardAction(key({ name: 'm' }), state)).toEqual({ type: 'none' })
  })

  test('choice panels: arrows, Enter, Esc and their own letters', () => {
    expect(resolveChoiceKey(key({ name: 'down' }))).toEqual({ type: 'move', delta: 1 })
    expect(resolveChoiceKey(key({ name: 'return' }))).toEqual({ type: 'pick' })
    expect(resolveChoiceKey(key({ name: 'escape' }))).toEqual({ type: 'close' })
    expect(resolveChoiceKey(key({ name: 'n' }), ['n'])).toEqual({ type: 'key', name: 'n' })
    expect(resolveChoiceKey(key({ name: 'n' }))).toEqual({ type: 'none' })
  })
})

describe('picker rows', () => {
  test("the model picker's thinking submenu lists the levels, then the default", () => {
    expect(reasoningLevels({ efforts: ['low', 'high'] })).toEqual(['low', 'high', ''])
    expect(reasoningLevels({ efforts: ['none', 'default'] })).toEqual(['none', 'default'])
    expect(reasoningLevels({ efforts: null })).toEqual([''])
  })

  test('the help lists the status bar keys, as Opt on a Mac', () => {
    const bar = helpSections('win32').find((section) => section.title === 'Status bar')!
    expect(bar.rows.map(([key]) => key)).toEqual(['Shift+Tab', 'Alt+M', 'Alt+T', 'Alt+W', 'Alt+P', 'Alt+C'])
    expect(helpSections('darwin').find((section) => section.title === 'Status bar')!.rows[1]![0]).toBe('Opt+M')
  })

  test('thinking levels with the default, the current one marked', () => {
    expect(thinkRows(status(), null).map((row) => [row.label, row.current ?? false])).toEqual([
      ['off', false],
      ['low', false],
      ['medium', false],
      ['high', true],
      ['default', false],
    ])
  })

  test('permission modes with their descriptions', () => {
    const rows = permissionRows(status({ plan_mode: true }))
    expect(rows.find((row) => row.current)?.id).toBe('plan')
    expect(rows.map((row) => row.id)).not.toContain('danger-full-access')
  })

  test('a permission choice sent and not shown yet is the one marked', () => {
    const rows = permissionRows(status(), 'plan')
    expect(rows.find((row) => row.current)?.id).toBe('plan')
  })

  test('switching the workspace during a turn takes a second Enter on the same row', () => {
    const other = { id: '/work/b', current: false }
    expect(workspacePickAction({ id: '/work/a', current: true }, true, null)).toBe('close')
    expect(workspacePickAction(other, false, null)).toBe('switch')
    expect(workspacePickAction(other, true, null)).toBe('confirm')
    expect(workspacePickAction(other, true, '/work/c')).toBe('confirm')
    expect(workspacePickAction(other, true, '/work/b')).toBe('switch')
  })

  test('workspaces: the current one first, no duplicates', () => {
    const rows = workspaceRows('/work/a', [{ path: '/work/b' }, { path: '/work/a' }])
    expect(rows.map((row) => [row.label, row.current])).toEqual([
      ['a', true],
      ['b', false],
    ])
  })

  test('branches: the current one first, upstream marks', () => {
    const rows = branchRows({
      current: 'main',
      branches: [
        { name: 'feat/x', upstream: 'origin/feat/x', ahead: 2, behind: 1, last_commit_ms: 2 },
        { name: 'main', upstream: 'origin/main', ahead: 0, behind: 0, last_commit_ms: 1 },
        { name: 'spike', upstream: null, ahead: null, behind: null, last_commit_ms: 0 },
      ],
    })
    expect(rows.map((row) => [row.label, row.detail, row.current])).toEqual([
      ['main', 'origin/main', true],
      ['feat/x', 'origin/feat/x ↑2 ↓1', false],
      ['spike', null, false],
    ])
  })
})

describe('commands sent to the engine', () => {
  let sent: EngineCommand[] = []
  beforeEach(() => {
    sent = []
    useEngineStore.getState().reset()
    setNeuronEngineClientForTesting({
      connection: 'ready',
      send: (command: EngineCommand) => sent.push(command),
      slash: (command: string, args = '') => sent.push({ type: 'slash', command, args }),
      subscribe: () => () => {},
      onExit: () => () => {},
    } as unknown as NeuronEngineClient)
  })
  afterEach(() => setNeuronEngineClientForTesting(null))

  test('with feature status_bar: the quiet commands', () => {
    useEngineStore.getState().setReady('7.0.0', [FEATURE_STATUS_BAR])
    setPermissionChoice('plan')
    setReasoningEffort(null)
    switchBranch('main', false)
    expect(sent).toEqual([
      { type: 'set_permission_mode', mode: 'plan' },
      { type: 'set_reasoning_effort', effort: null },
      { type: 'switch_branch', name: 'main', create: false },
    ])
  })

  test('Shift+Tab twice before the engine answers goes two steps, not the same one twice', () => {
    useEngineStore.getState().setReady('7.0.0', [FEATURE_STATUS_BAR])
    // Started with Full access, now Edit workspace: two presses mean Plan.
    useEngineStore.getState().setStatus(status({ full_access_allowed: true }))
    expect(cyclePermissionChoice()).toBe('danger-full-access')
    expect(cyclePermissionChoice()).toBe('plan')
    expect(sent).toEqual([
      { type: 'set_permission_mode', mode: 'danger-full-access' },
      { type: 'set_permission_mode', mode: 'plan' },
    ])
    // The status from the end of a running turn still shows the old mode.
    useEngineStore.getState().setStatus(status({ full_access_allowed: true }))
    expect(useEngineStore.getState().pendingPermission).toBe('plan')
    // The first choice's answer: still pending.
    useEngineStore
      .getState()
      .setStatus(status({ full_access_allowed: true, permission_mode: 'danger-full-access' }))
    expect(useEngineStore.getState().pendingPermission).toBe('plan')
    // The second's: done, and the next press cycles from Plan.
    useEngineStore
      .getState()
      .setStatus(status({ full_access_allowed: true, permission_mode: 'danger-full-access', plan_mode: true }))
    expect(useEngineStore.getState().pendingPermission).toBeNull()
    expect(cyclePermissionChoice()).toBe('read-only')
  })

  test('Edit, Plan, Read-only during a turn; an error drops the pending choice', () => {
    useEngineStore.getState().setReady('7.0.0', [FEATURE_STATUS_BAR])
    useEngineStore.getState().setStatus(status())
    cyclePermissionChoice()
    cyclePermissionChoice()
    expect(sent).toEqual([
      { type: 'set_permission_mode', mode: 'plan' },
      { type: 'set_permission_mode', mode: 'read-only' },
    ])
    handleEngineEvent({ type: 'error', message: 'Full access was not enabled at launch.' })
    expect(useEngineStore.getState().pendingPermission).toBeNull()
    expect(nextPermissionChoice(useEngineStore.getState().status, null)).toBe('plan')
  })

  test('the bar shows a choice that is not in effect yet', () => {
    useEngineStore.getState().setReady('7.0.0', [FEATURE_STATUS_BAR])
    useEngineStore.getState().setStatus(status())
    setPermissionChoice('plan')
    const { status: current, account, models, pendingPermission } = useEngineStore.getState()
    const text = (pending: string | null) =>
      statusBarLine({ status: current, account, models, mode: 'unicode', pendingPermission: pending }, 160)!
        .left.find((segment) => segment.target === 'permission')!
        .spans.map((span) => span.text)
        .join('')
    expect(text(pendingPermission)).toBe('✎ Edit workspace → ◇ Plan next')
    expect(text(null)).toBe('✎ Edit workspace')
    // Once a status shows it, nothing is pending.
    expect(text('workspace-write')).toBe('✎ Edit workspace')
  })

  test('without it: the slash commands, and no branch switching', async () => {
    useEngineStore.getState().setReady('7.0.0', [])
    setPermissionChoice('plan')
    expect(switchBranch('main', false)).toBe(false)
    await Promise.resolve()
    expect(sent).toEqual([{ type: 'slash', command: 'plan', args: 'on' }])
  })
})

describe('events', () => {
  beforeEach(() => useEngineStore.getState().reset())

  test('account events reach the store', () => {
    const parsed = parseUiEventLine(
      JSON.stringify({ type: 'account', signed_in: true, account: { email: 'dev@example.com', plan: 'free', usage_window: { tokens: 1, limit_tokens: 10, remaining_tokens: 9, resets_at: null, window_seconds: 18000 } } }),
    )
    expect(parsed.ok).toBe(true)
    if (!parsed.ok) return
    handleEngineEvent(parsed.event)
    expect(useEngineStore.getState().account?.usage_window?.limit_tokens).toBe(10)
    handleEngineEvent({ type: 'account', signed_in: false, account: null })
    expect(useEngineStore.getState().account).toBeNull()
  })

  test('branches events reach the store', () => {
    handleEngineEvent({ type: 'branches', current: 'main', branches: [], dirty: true })
    expect(useEngineStore.getState().branches).toEqual({ current: 'main', branches: [], dirty: true })
  })

  test('an older status without the new fields still parses', () => {
    const parsed = parseUiEventLine(
      '{"type":"status","model":"m","permission_mode":"read-only","session_id":"s","cwd":".","workspace_trusted":true,"plan_mode":false,"context_percent":3,"git":null}',
    )
    expect(parsed.ok && parsed.event.type).toBe('status')
  })
})
