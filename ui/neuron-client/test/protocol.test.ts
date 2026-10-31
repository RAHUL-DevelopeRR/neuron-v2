import { describe, expect, test } from 'bun:test'
import fs from 'fs'
import path from 'path'

import {
  ENGINE_COMMAND_TYPES,
  JsonLineSplitter,
  UI_EVENT_TYPES,
  commandMessage,
  encodeClientMessage,
  parseClientMessage,
  parseUiEventLine,
  permissionReplyMessage,
  type AccountDetails,
  type AccountInfo,
  type DefinitionInfo,
  type ErrorDetail,
  type GitStatus,
  type HookPreview,
  type McpServerInfo,
  type McpServerPreview,
  type ModelInfo,
  type PermissionReply,
  type PermissionRulePreview,
  type PermissionView,
  type PluginInfo,
  type ReportLink,
  type ReportSection,
  type SessionSummary,
  type TodoItem,
  type TokenCounts,
  type UiEvent,
  type UiEventType,
  type UsageAmounts,
} from '../src/protocol'

const rustCrate = path.resolve(import.meta.dir, '../../../rust/crates/neuron-tui')
const protocolRs = fs.readFileSync(path.join(rustCrate, 'src/protocol.rs'), 'utf8').replace(/\r\n/g, '\n')
const fixtureLines = (name: string) =>
  fs.readFileSync(path.join(rustCrate, 'fixtures', name), 'utf8').split(/\r?\n/).filter(Boolean)

/** The `{ ... }` body that follows `header` in protocol.rs. */
function body(header: string): string {
  const start = protocolRs.indexOf(header)
  expect(start).toBeGreaterThan(-1)
  let depth = 0
  let text = ''
  for (let index = protocolRs.indexOf('{', start); index < protocolRs.length; index++) {
    const char = protocolRs[index]
    if (char === '{') depth++
    if (char === '}') depth--
    text += char
    if (depth === 0) break
  }
  return text
}

const snake = (name: string) => name.replace(/[A-Z]/g, (letter, offset) => (offset ? '_' : '') + letter.toLowerCase())

/** The variants of `pub enum <name>`, as serde's snake_case tags, with their fields. */
function variants(enumName: string): Map<string, string[]> {
  const found = new Map<string, string[]>()
  let level = 0
  let current: string[] | null = null
  for (const line of body(`pub enum ${enumName} {`).split('\n')) {
    const trimmed = line.trim()
    // Variants sit at depth 1: a capitalised name at the start of a line, after its doc comments.
    if (level === 1 && /^[A-Z]\w*\s*(\{|\(|,|$)/.test(trimmed)) {
      current = []
      found.set(snake(/^[A-Z]\w*/.exec(trimmed)![0]), current)
      // A one-line variant: `TextDelta { text: String },`.
      for (const field of trimmed.matchAll(/(?:\{|,)\s*(\w+):/g)) current.push(field[1])
    } else if (level === 2 && current && /^\w+:/.test(trimmed)) {
      current.push(/^\w+/.exec(trimmed)![0])
    }
    for (const char of line) {
      if (char === '{') level++
      if (char === '}') level--
    }
  }
  return found
}

/** The field names of every `pub struct` in protocol.rs. */
function structs(): Map<string, string[]> {
  const found = new Map<string, string[]>()
  for (const match of protocolRs.matchAll(/pub struct (\w+) \{/g)) {
    const fields = [...body(match[0]).matchAll(/^\s*pub (\w+):/gm)].map((field) => field[1])
    found.set(match[1], fields)
  }
  return found
}

/**
 * Every field name of `T`: a list that leaves one out, or names one `T` lacks,
 * does not typecheck (`bun run typecheck`).
 */
const fieldsOf =
  <T>() =>
  <const K extends readonly (keyof T & string)[]>(
    keys: K & (Exclude<keyof T, K[number]> extends never ? unknown : 'lists every field'),
  ): string[] => [...keys]

const STRUCT_FIELDS: Record<string, string[]> = {
  TokenCounts: fieldsOf<TokenCounts>()(['input', 'output', 'cache_read', 'cache_write']),
  PermissionView: fieldsOf<PermissionView>()(['tool', 'current_mode', 'required_mode', 'reason', 'input', 'session_prefixes']),
  GitStatus: fieldsOf<GitStatus>()(['branch', 'changed', 'staged', 'unstaged', 'untracked', 'conflicted']),
  ModelInfo: fieldsOf<ModelInfo>()([
    'id',
    'display_name',
    'available',
    'unavailable_reason',
    'context_window',
    'tiers',
    'status',
    'available_again_at',
    'supports_images',
  ]),
  AccountInfo: fieldsOf<AccountInfo>()(['email', 'plan', 'source', 'expires_at']),
  McpServerInfo: fieldsOf<McpServerInfo>()(['name', 'scope', 'transport', 'status', 'tools', 'error']),
  PluginInfo: fieldsOf<PluginInfo>()(['id', 'name', 'version', 'description', 'kind', 'enabled']),
  DefinitionInfo: fieldsOf<DefinitionInfo>()(['name', 'description', 'source', 'active']),
  SessionSummary: fieldsOf<SessionSummary>()([
    'id',
    'title',
    'model',
    'message_count',
    'updated_at_ms',
    'current',
    'branch',
    'project',
    'git_branch',
    'run_status',
    'failed',
    'client',
  ]),
  ReportSection: fieldsOf<ReportSection>()(['heading', 'markdown']),
  ReportLink: fieldsOf<ReportLink>()(['label', 'url']),
  UsageAmounts: fieldsOf<UsageAmounts>()(['requests', 'tokens']),
  AccountDetails: fieldsOf<AccountDetails>()([
    'email',
    'name',
    'plan',
    'plan_name',
    'source',
    'expires_at',
    'limits',
    'usage_today',
    'resets_at',
    'credit_balance_cents',
    'suspended',
    'dashboard_url',
    'pricing_url',
    'usage_updated_at',
    'usage_error',
  ]),
  ErrorDetail: fieldsOf<ErrorDetail>()(['code', 'status', 'upgrade_url', 'resets_at', 'retry_after_seconds']),
  HookPreview: fieldsOf<HookPreview>()(['event', 'command', 'source']),
  McpServerPreview: fieldsOf<McpServerPreview>()(['name', 'transport', 'target', 'source']),
  PermissionRulePreview: fieldsOf<PermissionRulePreview>()(['effect', 'rule', 'source']),
  TodoItem: fieldsOf<TodoItem>()(['content', 'status', 'active_form']),
  PermissionReply: fieldsOf<PermissionReply>()(['id', 'decision']),
}

type EventFields<T extends UiEventType> = Exclude<keyof Extract<UiEvent, { type: T }>, 'type'>
const eventFields =
  <T extends UiEventType>(_type: T) =>
  <const K extends readonly EventFields<T>[]>(
    keys: K & (Exclude<EventFields<T>, K[number]> extends never ? unknown : 'lists every field'),
  ): string[] => [...(keys as readonly string[])]

/** Every event's fields, as protocol.rs names them; a new event or field fails until listed. */
const EVENT_FIELDS: { [T in UiEventType]: string[] } = {
  ready: eventFields('ready')(['protocol', 'version', 'features']),
  status: eventFields('status')([
    'model',
    'permission_mode',
    'session_id',
    'cwd',
    'workspace_trusted',
    'plan_mode',
    'context_percent',
    'git',
    'signed_in',
    'sign_in_required',
    'account',
    'supports_images',
  ]),
  session_starting: eventFields('session_starting')([]),
  sign_in_code: eventFields('sign_in_code')(['url', 'url_complete', 'user_code', 'expires_in_secs', 'browser_opened']),
  models: eventFields('models')(['current', 'models']),
  text_delta: eventFields('text_delta')(['text']),
  text_block_end: eventFields('text_block_end')([]),
  reasoning_started: eventFields('reasoning_started')(['chars', 'redacted']),
  reasoning_delta: eventFields('reasoning_delta')(['text']),
  tool_call_started: eventFields('tool_call_started')(['id', 'name', 'input', 'cut_off']),
  tool_result: eventFields('tool_result')(['id', 'name', 'output', 'is_error']),
  usage: eventFields('usage')(['turn', 'cumulative', 'estimated']),
  permission_requested: eventFields('permission_requested')(['id', 'request']),
  notice: eventFields('notice')(['message']),
  turn_finished: eventFields('turn_finished')([]),
  error: eventFields('error')(['message', 'detail']),
  sessions: eventFields('sessions')(['current', 'sessions', 'scope']),
  transcript: eventFields('transcript')(['session_id', 'entries', 'truncated']),
  workspace_trusted: eventFields('workspace_trusted')(['cwd', 'restart_required', 'root', 'session_only']),
  question: eventFields('question')(['id', 'question', 'options']),
  todos: eventFields('todos')(['items']),
  subagent_started: eventFields('subagent_started')(['id', 'description', 'subagent_type']),
  subagent_finished: eventFields('subagent_finished')(['id', 'status', 'is_error', 'result', 'note']),
  extensions: eventFields('extensions')(['mcp_servers', 'plugins', 'skills', 'agents']),
  report: eventFields('report')(['command', 'title', 'sections', 'links']),
  account: eventFields('account')(['signed_in', 'account']),
  trust_preview: eventFields('trust_preview')([
    'cwd',
    'root',
    'is_git_root',
    'session_only',
    'trusted',
    'settings_files',
    'hooks',
    'mcp_servers',
    'permission_rules',
    'other_settings',
  ]),
}

const sorted = (values: Iterable<string>) => [...values].sort()

describe('the protocol mirrors protocol.rs', () => {
  test('every UiEvent variant has a tag here, and no more', () => {
    expect(sorted(variants('UiEvent').keys())).toEqual(sorted(UI_EVENT_TYPES))
  })

  test('every EngineCommand variant has a tag here, and no more', () => {
    expect(sorted(variants('EngineCommand').keys())).toEqual(sorted(ENGINE_COMMAND_TYPES))
  })

  test('every UiEvent variant has the fields listed here', () => {
    const rust = variants('UiEvent')
    for (const type of UI_EVENT_TYPES) {
      expect({ type, fields: sorted(rust.get(type) ?? []) }).toEqual({ type, fields: sorted(EVENT_FIELDS[type]) })
    }
  })

  test('every struct has the fields listed here', () => {
    const rust = structs()
    expect(sorted(rust.keys())).toEqual(sorted(Object.keys(STRUCT_FIELDS)))
    for (const [name, fields] of rust) {
      expect({ name, fields: sorted(fields) }).toEqual({ name, fields: sorted(STRUCT_FIELDS[name] ?? []) })
    }
  })

  test('the protocol version matches', () => {
    expect(protocolRs).toContain('pub const PROTOCOL_VERSION: u32 = 1;')
  })
})

describe('the fake engine samples (rust/crates/neuron-tui/fixtures)', () => {
  test('every sample event parses, with only the fields known here', () => {
    const lines = fixtureLines('protocol-v2-events.jsonl')
    expect(lines.length).toBeGreaterThan(0)
    for (const line of lines) {
      const parsed = parseUiEventLine(line)
      expect(parsed.ok).toBe(true)
      if (!parsed.ok) continue
      const known = new Set(['type', ...EVENT_FIELDS[parsed.event.type]])
      expect(Object.keys(parsed.event).filter((key) => !known.has(key))).toEqual([])
    }
    const types = new Set(lines.map((line) => JSON.parse(line).type))
    for (const type of ['report', 'account', 'trust_preview', 'sessions', 'error', 'workspace_trusted']) {
      expect(types.has(type)).toBe(true)
    }
  })

  test('every sample command passes the untrusted-peer check unchanged', () => {
    for (const line of fixtureLines('protocol-v2-commands.jsonl')) {
      expect(parseClientMessage(JSON.parse(line))).toEqual(JSON.parse(line))
    }
  })
})

describe('lines', () => {
  test('client messages are flat, kind-tagged JSON lines', () => {
    expect(encodeClientMessage(commandMessage({ type: 'submit', text: 'hi', attachments: [] }))).toBe(
      '{"kind":"command","type":"submit","text":"hi","attachments":[]}\n',
    )
    expect(JSON.parse(encodeClientMessage(permissionReplyMessage({ id: 3, decision: { kind: 'allow_once' } })))).toEqual({
      kind: 'permission_reply',
      id: 3,
      decision: { kind: 'allow_once' },
    })
    expect(encodeClientMessage(commandMessage({ type: 'list_sessions', scope: 'all' }))).toBe(
      '{"kind":"command","type":"list_sessions","scope":"all"}\n',
    )
  })

  test('event lines parse; unknown types and junk are reported, not thrown', () => {
    expect(parseUiEventLine('{"type":"turn_finished"}')).toEqual({ ok: true, event: { type: 'turn_finished' } })
    expect(parseUiEventLine('{"type":"future_event"}')).toEqual({ ok: false, reason: 'unknown_type', type: 'future_event' })
    expect(parseUiEventLine('not json').ok).toBe(false)
    expect(parseUiEventLine('{"no":"type"}').ok).toBe(false)
  })

  test('the splitter keeps partial lines and strips CRs', () => {
    const splitter = new JsonLineSplitter()
    expect(splitter.push('{"a":1}\r\n{"b"')).toEqual(['{"a":1}'])
    expect(splitter.push(':2}\n\n')).toEqual(['{"b":2}'])
    expect(splitter.push('{"c":3}')).toEqual([])
    expect(splitter.flush()).toBe('{"c":3}')
  })
})

describe('client messages from untrusted peers', () => {
  test('valid commands and replies come through, with only their known fields', () => {
    expect(parseClientMessage({ kind: 'command', type: 'submit', text: 'go', attachments: [{ kind: 'file', path: 'a.rs' }], extra: 1 })).toEqual({
      kind: 'command',
      type: 'submit',
      text: 'go',
      attachments: [{ kind: 'file', path: 'a.rs' }],
    })
    expect(parseClientMessage({ kind: 'command', type: 'slash', command: 'model' })).toEqual({
      kind: 'command',
      type: 'slash',
      command: 'model',
      args: '',
    })
    expect(parseClientMessage({ kind: 'command', type: 'list_extensions', verbose: true })).toEqual({ kind: 'command', type: 'list_extensions' })
    expect(parseClientMessage({ kind: 'command', type: 'list_sessions' })).toEqual({ kind: 'command', type: 'list_sessions' })
    expect(parseClientMessage({ kind: 'command', type: 'list_sessions', scope: 'all' })).toEqual({
      kind: 'command',
      type: 'list_sessions',
      scope: 'all',
    })
    expect(parseClientMessage({ kind: 'command', type: 'capabilities', features: ['reports'], client: 'desktop', extra: 1 })).toEqual({
      kind: 'command',
      type: 'capabilities',
      features: ['reports'],
      client: 'desktop',
    })
    expect(parseClientMessage({ kind: 'command', type: 'trust_preview' })).toEqual({ kind: 'command', type: 'trust_preview' })
    expect(parseClientMessage({ kind: 'command', type: 'refresh_account', force: true })).toEqual({ kind: 'command', type: 'refresh_account' })
    expect(parseClientMessage({ kind: 'permission_reply', id: 4, decision: { kind: 'deny', note: 'no' } })).toEqual({
      kind: 'permission_reply',
      id: 4,
      decision: { kind: 'deny', note: 'no' },
    })
  })

  test('malformed ones are refused', () => {
    for (const bad of [
      null,
      'submit',
      { kind: 'command', type: 'submit' },
      { kind: 'command', type: 'submit', text: 'x', attachments: [{ kind: 'socket', path: '/' }] },
      { kind: 'command', type: 'format_disk' },
      { kind: 'command', type: 'list_sessions', scope: 'galaxy' },
      { kind: 'command', type: 'capabilities', features: [1] },
      { kind: 'permission_reply', id: 'one', decision: { kind: 'allow_once' } },
      { kind: 'permission_reply', id: 1, decision: { kind: 'allow_forever' } },
      { kind: 'other' },
    ]) {
      expect(parseClientMessage(bad)).toBeNull()
    }
  })
})
