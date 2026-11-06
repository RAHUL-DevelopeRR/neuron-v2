import { describe, expect, test } from 'bun:test'
import fs from 'fs'
import path from 'path'

import {
  CAPABILITY_GIT_VIEWS,
  CAPABILITY_LIVE_OUTPUT,
  CAPABILITY_LIVE_PROGRESS,
  CAPABILITY_QUESTIONS,
  CAPABILITY_REPORTS,
  DESKTOP_CAPABILITIES,
  ENGINE_COMMAND_TYPES,
  FEATURE_ACCOUNT,
  FEATURE_CAPABILITIES,
  FEATURE_CHECKPOINTS,
  FEATURE_ERROR_DETAILS,
  FEATURE_EXTENSIONS,
  FEATURE_GIT_VIEWS,
  FEATURE_LIVE_OUTPUT,
  FEATURE_LIVE_PROGRESS,
  FEATURE_MCP_TOGGLE,
  FEATURE_MODEL_IMAGES,
  FEATURE_REPORTS,
  FEATURE_SESSIONS,
  FEATURE_SESSION_SCOPE,
  FEATURE_SUBAGENTS,
  FEATURE_TODOS,
  FEATURE_TRUST_PREVIEW,
  FEATURE_TRUST_WORKSPACE,
  JsonLineSplitter,
  UI_EVENT_TYPES,
  commandMessage,
  encodeClientMessage,
  parseClientMessage,
  parseUiEventLine,
  permissionReplyMessage,
  type AccountDetails,
  type AccountInfo,
  type ChangedFile,
  type CheckpointInfo,
  type DefinitionInfo,
  type DiffFile,
  type EngineCommand,
  type NotRestoredFile,
  type NoticeKind,
  type OutputStream,
  type RewindMode,
  type SessionRunStatus,
  type SessionScope,
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
const liveLines = fs
  .readFileSync(path.join(import.meta.dir, 'fixtures/protocol-v2-live.jsonl'), 'utf8')
  .split(/\r?\n/)
  .filter(Boolean)

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
  McpServerInfo: fieldsOf<McpServerInfo>()(['name', 'scope', 'transport', 'status', 'tools', 'error', 'enabled']),
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
  ChangedFile: fieldsOf<ChangedFile>()(['path', 'status', 'added', 'removed']),
  DiffFile: fieldsOf<DiffFile>()(['path', 'status', 'added', 'removed', 'patch', 'binary']),
  CheckpointInfo: fieldsOf<CheckpointInfo>()([
    'id',
    'created_at_ms',
    'prompt_preview',
    'files',
    'shell_commands',
    'before_rewind_to',
  ]),
  NotRestoredFile: fieldsOf<NotRestoredFile>()(['path', 'reason']),
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
    'served_model',
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
  notice: eventFields('notice')(['message', 'kind']),
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
  tool_input_started: eventFields('tool_input_started')(['id', 'name']),
  tool_input_progress: eventFields('tool_input_progress')(['id', 'name', 'bytes', 'lines', 'target', 'preview']),
  output_progress: eventFields('output_progress')(['output_tokens']),
  request_usage: eventFields('request_usage')(['usage']),
  tool_output_delta: eventFields('tool_output_delta')(['tool_use_id', 'stream', 'chunk']),
  changes: eventFields('changes')(['files']),
  diff: eventFields('diff')(['title', 'files', 'truncated']),
  commit_proposal: eventFields('commit_proposal')(['message', 'branch', 'files', 'stage_all']),
  checkpoints: eventFields('checkpoints')(['items']),
  rewound: eventFields('rewound')([
    'checkpoint',
    'mode',
    'dry_run',
    'restored',
    'removed',
    'not_restored',
    'shell_commands',
    'removed_turns',
    'prompt',
  ]),
}

type CommandFields<T extends EngineCommand['type']> = Exclude<keyof Extract<EngineCommand, { type: T }>, 'type'>
const commandFields =
  <T extends EngineCommand['type']>(_type: T) =>
  <const K extends readonly CommandFields<T>[]>(
    keys: K & (Exclude<CommandFields<T>, K[number]> extends never ? unknown : 'lists every field'),
  ): string[] => [...(keys as readonly string[])]

/** Every command's fields, as protocol.rs names them. */
const COMMAND_FIELDS: { [T in EngineCommand['type']]: string[] } = {
  submit: commandFields('submit')(['text', 'attachments']),
  slash: commandFields('slash')(['command', 'args']),
  set_model: commandFields('set_model')(['model']),
  list_models: commandFields('list_models')([]),
  interrupt: commandFields('interrupt')([]),
  shutdown: commandFields('shutdown')([]),
  capabilities: commandFields('capabilities')(['features', 'client']),
  answer: commandFields('answer')(['id', 'text']),
  list_sessions: commandFields('list_sessions')(['scope']),
  resume: commandFields('resume')(['session']),
  trust_workspace: commandFields('trust_workspace')([]),
  list_extensions: commandFields('list_extensions')([]),
  set_mcp_server_enabled: commandFields('set_mcp_server_enabled')(['name', 'enabled']),
  trust_preview: commandFields('trust_preview')([]),
  refresh_account: commandFields('refresh_account')([]),
  commit: commandFields('commit')(['message']),
  rewind: commandFields('rewind')(['checkpoint', 'mode', 'dry_run']),
}

/** A typed list of a union's members: a missing member fails typecheck. */
const membersOf =
  <T extends string>() =>
  <const K extends readonly T[]>(
    values: K & (Exclude<T, K[number]> extends never ? unknown : 'lists every member'),
  ): string[] => [...values]

const UNIT_ENUMS: Record<string, string[]> = {
  OutputStream: membersOf<OutputStream>()(['stdout', 'stderr']),
  NoticeKind: membersOf<NoticeKind>()(['refusal']),
  SessionScope: membersOf<SessionScope>()(['project', 'all']),
  SessionRunStatus: membersOf<SessionRunStatus>()(['idle', 'open', 'running']),
}

/** The snake_case values of `pub enum <name>` (unit variants only). */
function unitVariants(enumName: string): string[] {
  return body(`pub enum ${enumName} {`)
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => /^[A-Z]\w*,?$/.test(line))
    .map((line) => snake(line.replace(',', '')))
}

const TS_FEATURE_CONSTANTS: Record<string, string> = {
  FEATURE_CAPABILITIES,
  FEATURE_SESSIONS,
  FEATURE_TRUST_WORKSPACE,
  FEATURE_TODOS,
  FEATURE_SUBAGENTS,
  FEATURE_EXTENSIONS,
  FEATURE_ERROR_DETAILS,
  FEATURE_ACCOUNT,
  FEATURE_REPORTS,
  FEATURE_SESSION_SCOPE,
  FEATURE_TRUST_PREVIEW,
  FEATURE_MODEL_IMAGES,
  FEATURE_LIVE_PROGRESS,
  FEATURE_GIT_VIEWS,
  FEATURE_CHECKPOINTS,
  FEATURE_MCP_TOGGLE,
  FEATURE_LIVE_OUTPUT,
  CAPABILITY_QUESTIONS,
  CAPABILITY_REPORTS,
  CAPABILITY_LIVE_PROGRESS,
  CAPABILITY_GIT_VIEWS,
  CAPABILITY_LIVE_OUTPUT,
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

  test('every EngineCommand variant has the fields listed here', () => {
    const rust = variants('EngineCommand')
    for (const type of ENGINE_COMMAND_TYPES) {
      expect({ type, fields: sorted(rust.get(type) ?? []) }).toEqual({ type, fields: sorted(COMMAND_FIELDS[type]) })
    }
  })

  test('every FEATURE_* and CAPABILITY_* constant of protocol.rs is exported here with the same value', () => {
    const consts = [...protocolRs.matchAll(/^pub const ((?:FEATURE|CAPABILITY)_\w+): &str = "([^"]+)";/gm)]
    expect(consts.length).toBeGreaterThan(15)
    for (const [, name, value] of consts) {
      expect({ name, value: TS_FEATURE_CONSTANTS[name] }).toEqual({ name, value })
    }
  })

  test('every feature string of ENGINE_FEATURES has a constant here', () => {
    const start = protocolRs.indexOf('pub const ENGINE_FEATURES: &[&str] = &[')
    expect(start).toBeGreaterThan(-1)
    const list = protocolRs.slice(start, protocolRs.indexOf('];', start))
    const literals = [...list.matchAll(/^\s*"(\w+)",/gm)].map((match) => match[1])
    expect(literals.sort()).toEqual(['extensions', 'sessions', 'subagents', 'todos', 'trust_workspace'])
    const known = new Set(Object.values(TS_FEATURE_CONSTANTS))
    for (const literal of literals) expect(known.has(literal)).toBe(true)
    for (const name of list.matchAll(/^\s*(FEATURE_\w+),/gm)) expect(name[1] in TS_FEATURE_CONSTANTS).toBe(true)
  })

  test('the unit enums have the same snake_case values as the TypeScript unions', () => {
    for (const [name, values] of Object.entries(UNIT_ENUMS)) {
      expect({ name, values: sorted(unitVariants(name)) }).toEqual({ name, values: sorted(values) })
    }
  })

  test('the rewind modes are the three the engine documents', () => {
    const modes: RewindMode[] = ['code_and_chat', 'chat_only', 'code_only']
    for (const mode of modes) expect(protocolRs).toContain('`' + mode + '`')
  })

  test('the desktop declares what it renders', () => {
    expect([...DESKTOP_CAPABILITIES].sort()).toEqual(['git_views', 'live_output', 'live_progress', 'questions', 'reports'])
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

describe('the live protocol samples (test/fixtures/protocol-v2-live.jsonl)', () => {
  const eventLines = liveLines.filter((line) => JSON.parse(line).kind !== 'command')
  const commandLines = liveLines.filter((line) => JSON.parse(line).kind === 'command')

  test('every event line parses, with only the fields known here, and every new event appears', () => {
    for (const line of eventLines) {
      const parsed = parseUiEventLine(line)
      expect(parsed.ok).toBe(true)
      if (!parsed.ok) continue
      const known = new Set(['type', ...EVENT_FIELDS[parsed.event.type]])
      expect(Object.keys(parsed.event).filter((key) => !known.has(key))).toEqual([])
    }
    const types = new Set(eventLines.map((line) => JSON.parse(line).type))
    for (const type of [
      'tool_input_started',
      'tool_input_progress',
      'output_progress',
      'request_usage',
      'tool_output_delta',
      'changes',
      'diff',
      'commit_proposal',
      'checkpoints',
      'rewound',
    ]) {
      expect(types.has(type)).toBe(true)
    }
  })

  test('every command line passes the untrusted-peer check unchanged, and every new command appears', () => {
    for (const line of commandLines) expect(parseClientMessage(JSON.parse(line))).toEqual(JSON.parse(line))
    const types = new Set(commandLines.map((line) => JSON.parse(line).type))
    for (const type of ['set_mcp_server_enabled', 'commit', 'rewind']) expect(types.has(type)).toBe(true)
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
    expect(parseClientMessage({ kind: 'command', type: 'set_mcp_server_enabled', name: 'gh', enabled: false, x: 1 })).toEqual({
      kind: 'command',
      type: 'set_mcp_server_enabled',
      name: 'gh',
      enabled: false,
    })
    expect(parseClientMessage({ kind: 'command', type: 'commit', message: 'fix: it', x: 1 })).toEqual({
      kind: 'command',
      type: 'commit',
      message: 'fix: it',
    })
    expect(parseClientMessage({ kind: 'command', type: 'rewind', checkpoint: 0, mode: 'chat_only', x: 1 })).toEqual({
      kind: 'command',
      type: 'rewind',
      checkpoint: 0,
      mode: 'chat_only',
    })
    expect(parseClientMessage({ kind: 'command', type: 'rewind', checkpoint: 2, mode: 'code_only', dry_run: true })).toEqual({
      kind: 'command',
      type: 'rewind',
      checkpoint: 2,
      mode: 'code_only',
      dry_run: true,
    })
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
      { kind: 'command', type: 'set_mcp_server_enabled', name: '', enabled: true },
      { kind: 'command', type: 'set_mcp_server_enabled', name: 'gh', enabled: 'yes' },
      { kind: 'command', type: 'set_mcp_server_enabled', name: 'gh' },
      { kind: 'command', type: 'commit', message: '   ' },
      { kind: 'command', type: 'commit' },
      { kind: 'command', type: 'rewind', checkpoint: -1, mode: 'chat_only' },
      { kind: 'command', type: 'rewind', checkpoint: 1.5, mode: 'chat_only' },
      { kind: 'command', type: 'rewind', checkpoint: 2 ** 60, mode: 'chat_only' },
      { kind: 'command', type: 'rewind', checkpoint: '1', mode: 'chat_only' },
      { kind: 'command', type: 'rewind', checkpoint: 1, mode: 'everything' },
      { kind: 'command', type: 'rewind', checkpoint: 1 },
      { kind: 'command', type: 'rewind', checkpoint: 1, mode: 'chat_only', dry_run: 'true' },
      { kind: 'permission_reply', id: 'one', decision: { kind: 'allow_once' } },
      { kind: 'permission_reply', id: 1, decision: { kind: 'allow_forever' } },
      { kind: 'other' },
    ]) {
      expect(parseClientMessage(bad)).toBeNull()
    }
  })
})

describe('untrusted peers: attachments and length caps', () => {
  const submit = (extra: Record<string, unknown>) => parseClientMessage({ kind: 'command', type: 'submit', text: 'go', ...extra })

  test('attachments are rebuilt from their known fields only', () => {
    expect(
      submit({
        attachments: [
          { kind: 'file', path: 'a.rs', evil: 1 },
          { kind: 'folder', path: 'src/lib' },
          { kind: 'image', path: 'shot.png', media_type: 'image/png', extra: true },
        ],
      }),
    ).toEqual({
      kind: 'command',
      type: 'submit',
      text: 'go',
      attachments: [
        { kind: 'file', path: 'a.rs' },
        { kind: 'folder', path: 'src/lib' },
        { kind: 'image', path: 'shot.png', media_type: 'image/png' },
      ],
    })
  })

  test('images need an allowed media type', () => {
    for (const media_type of ['image/png', 'image/jpeg', 'image/gif', 'image/webp']) {
      expect(submit({ attachments: [{ kind: 'image', path: 'a', media_type }] })).not.toBeNull()
    }
    expect(submit({ attachments: [{ kind: 'image', path: 'a', media_type: 'image/svg+xml' }] })).toBeNull()
    expect(submit({ attachments: [{ kind: 'image', path: 'a' }] })).toBeNull()
    expect(submit({ attachments: [{ kind: 'video', path: 'a' }] })).toBeNull()
  })

  test('paths must be non-empty, relative, without .. or NUL', () => {
    for (const path of ['', '/x', '\\x', 'C:\\x', 'C:x', 'c:/x', '\\\\server\\share', '../x', 'a/../b', 'a\\..\\b', '..', 'a\0b']) {
      expect(submit({ attachments: [{ kind: 'file', path }] })).toBeNull()
    }
    expect(submit({ attachments: [{ kind: 'file', path: 'a..b/c.rs' }] })).not.toBeNull()
  })

  test('at most 20 attachments and 256 KiB of text', () => {
    const one = { kind: 'file', path: 'a' }
    expect(submit({ attachments: Array(20).fill(one) })).not.toBeNull()
    expect(submit({ attachments: Array(21).fill(one) })).toBeNull()
    expect(parseClientMessage({ kind: 'command', type: 'submit', text: 'x'.repeat(256 * 1024) })).not.toBeNull()
    expect(parseClientMessage({ kind: 'command', type: 'submit', text: 'x'.repeat(256 * 1024 + 1) })).toBeNull()
  })

  test('other fields are capped', () => {
    const cmd = (type: string, rest: Record<string, unknown>) => parseClientMessage({ kind: 'command', type, ...rest })
    const cases: Array<[string, (n: number) => Record<string, unknown>, number]> = [
      ['commit', (n) => ({ message: 'x'.repeat(n) }), 16 * 1024],
      ['set_mcp_server_enabled', (n) => ({ name: 'x'.repeat(n), enabled: true }), 256],
      ['capabilities', (n) => ({ features: ['f'], client: 'x'.repeat(n) }), 64],
      ['answer', (n) => ({ id: 1, text: 'x'.repeat(n) }), 256 * 1024],
      ['slash', (n) => ({ command: 'x'.repeat(n) }), 64],
      ['slash', (n) => ({ command: 'm', args: 'x'.repeat(n) }), 64 * 1024],
      ['set_model', (n) => ({ model: 'x'.repeat(n) }), 256],
      ['resume', (n) => ({ session: 'x'.repeat(n) }), 512],
    ]
    for (const [type, make, cap] of cases) {
      expect(cmd(type, make(cap))).not.toBeNull()
      expect(cmd(type, make(cap + 1))).toBeNull()
    }
    expect(cmd('capabilities', { features: Array(32).fill('f') })).not.toBeNull()
    expect(cmd('capabilities', { features: Array(33).fill('f') })).toBeNull()
    expect(cmd('capabilities', { features: ['x'.repeat(64)] })).not.toBeNull()
    expect(cmd('capabilities', { features: ['x'.repeat(65)] })).toBeNull()
  })
})
