import { describe, expect, test } from 'bun:test'

import { statusAddendum } from '../status-report'

import type { StatusEvent } from '../protocol'

const STATUS: StatusEvent = {
  type: 'status',
  model: 'auto',
  permission_mode: 'workspace-write',
  session_id: 'session-1',
  cwd: '/work',
  workspace_trusted: true,
  plan_mode: false,
  context_percent: 4,
  git: null,
  signed_in: true,
  sign_in_required: false,
  account: {
    email: 'ana@example.com',
    plan: 'free',
    source: 'stored login, credentials file',
    expires_at: 1_795_241_682,
  },
}

describe('statusAddendum', () => {
  test('names the signed-in account and the folder trust', () => {
    expect(statusAddendum(STATUS)).toBe(
      [
        'Zero-X',
        '  Account          ana@example.com',
        '  Plan             free',
        '  Signed in        yes (stored login, credentials file)',
        '  Session expires  2026-11-21 06:14 UTC',
        '',
        'Workspace trust',
        '  Folder           trusted',
      ].join('\n'),
    )
  })

  test('an untrusted folder says what it means and how to trust it', () => {
    const text = statusAddendum({ ...STATUS, workspace_trusted: false })!
    expect(text).toContain('  Folder           not trusted')
    expect(text).toContain('settings, hooks and MCP servers')
    expect(text).toContain('neuron --no-tui')
    expect(text).toContain('trustedRoots')
  })

  test('an expired or revoked login, no login, or another provider', () => {
    const revoked = { ...STATUS, session_id: '', sign_in_required: true }
    expect(statusAddendum(revoked)).toContain('Signed in        expired or revoked: type /login')
    expect(statusAddendum({ ...revoked, signed_in: false, account: null })).toContain(
      'Signed in        no: type /login',
    )
    expect(statusAddendum({ ...STATUS, signed_in: false, account: null })).toContain(
      'Signed in        no (this session uses another provider)',
    )
    expect(statusAddendum(null)).toBeNull()
  })
})
