import { describe, expect, test } from 'bun:test'
import fs from 'fs'
import path from 'path'

import {
  EngineNotConnectedError,
  engineArgs,
  runTurn,
} from '../neuron-engine-client'
import { StdioEngineClient } from '../stdio-engine-client'
import {
  ENGINE_COMMAND_TYPES,
  JsonLineSplitter,
  UI_EVENT_TYPES,
  commandMessage,
  encodeClientMessage,
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
} from '../protocol'

const rustCrate = path.resolve(
  import.meta.dir,
  '../../../../../rust/crates/neuron-tui',
)
const protocolRs = fs
  .readFileSync(path.join(rustCrate, 'src/protocol.rs'), 'utf8')
  .replace(/\r\n/g, '\n')
const fixtureLines = (name: string) =>
  fs
    .readFileSync(path.join(rustCrate, 'fixtures', name), 'utf8')
    .split(/\r?\n/)
    .filter(Boolean)

/** The `{ ... }` body that follows `header` in protocol.rs. */
function rustBody(header: string): string {
  const start = protocolRs.indexOf(header)
  expect(start).toBeGreaterThan(-1)
  let depth = 0
  let text = ''
  for (
    let index = protocolRs.indexOf('{', start);
    index < protocolRs.length;
    index++
  ) {
    const char = protocolRs[index]
    if (char === '{') depth++
    if (char === '}') depth--
    text += char
    if (depth === 0) break
  }
  return text
}

const snakeCase = (name: string) =>
  name.replace(
    /[A-Z]/g,
    (letter, offset) => (offset ? '_' : '') + letter.toLowerCase(),
  )

/** The variants of `pub enum <name>` as serde's snake_case tags, with their fields. */
function rustVariants(enumName: string): Map<string, string[]> {
  const found = new Map<string, string[]>()
  let level = 0
  let current: string[] | null = null
  for (const line of rustBody(`pub enum ${enumName} {`).split('\n')) {
    const trimmed = line.trim()
    // Variants sit at depth 1: a capitalised name at the start of a line.
    if (level === 1 && /^[A-Z]\w*\s*(\{|\(|,|$)/.test(trimmed)) {
      current = []
      found.set(snakeCase(/^[A-Z]\w*/.exec(trimmed)![0]), current)
      // A one-line variant: `TextDelta { text: String },`.
      for (const field of trimmed.matchAll(/(?:\{|,)\s*(\w+):/g)) {
        current.push(field[1]!)
      }
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
function rustStructs(): Map<string, string[]> {
  const found = new Map<string, string[]>()
  for (const match of protocolRs.matchAll(/pub struct (\w+) \{/g)) {
    const fields = [...rustBody(match[0]).matchAll(/^\s*pub (\w+):/gm)].map(
      (field) => field[1]!,
    )
    found.set(match[1]!, fields)
  }
  return found
}

/**
 * Every field name of `T`: a list that leaves one out, or names one `T`
 * lacks, does not typecheck (`bun run typecheck`).
 */
const fieldsOf =
  <T>() =>
  <const K extends readonly (keyof T & string)[]>(
    keys: K &
      (Exclude<keyof T, K[number]> extends never ? unknown : 'lists every field'),
  ): string[] => [...keys]

const STRUCT_FIELDS: Record<string, string[]> = {
  TokenCounts: fieldsOf<TokenCounts>()([
    'input',
    'output',
    'cache_read',
    'cache_write',
  ]),
  PermissionView: fieldsOf<PermissionView>()([
    'tool',
    'current_mode',
    'required_mode',
    'reason',
    'input',
    'session_prefixes',
  ]),
  GitStatus: fieldsOf<GitStatus>()([
    'branch',
    'changed',
    'staged',
    'unstaged',
    'untracked',
    'conflicted',
  ]),
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
  McpServerInfo: fieldsOf<McpServerInfo>()([
    'name',
    'scope',
    'transport',
    'status',
    'tools',
    'error',
  ]),
  PluginInfo: fieldsOf<PluginInfo>()([
    'id',
    'name',
    'version',
    'description',
    'kind',
    'enabled',
  ]),
  DefinitionInfo: fieldsOf<DefinitionInfo>()([
    'name',
    'description',
    'source',
    'active',
  ]),
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
  ErrorDetail: fieldsOf<ErrorDetail>()([
    'code',
    'status',
    'upgrade_url',
    'resets_at',
    'retry_after_seconds',
  ]),
  HookPreview: fieldsOf<HookPreview>()(['event', 'command', 'source']),
  McpServerPreview: fieldsOf<McpServerPreview>()([
    'name',
    'transport',
    'target',
    'source',
  ]),
  PermissionRulePreview: fieldsOf<PermissionRulePreview>()([
    'effect',
    'rule',
    'source',
  ]),
  TodoItem: fieldsOf<TodoItem>()(['content', 'status', 'active_form']),
  PermissionReply: fieldsOf<PermissionReply>()(['id', 'decision']),
}

type EventFields<T extends UiEventType> = Exclude<
  keyof Extract<UiEvent, { type: T }>,
  'type'
>
const eventFields =
  <T extends UiEventType>(_type: T) =>
  <const K extends readonly EventFields<T>[]>(
    keys: K &
      (Exclude<EventFields<T>, K[number]> extends never
        ? unknown
        : 'lists every field'),
  ): string[] => [...(keys as readonly string[])]

/** Every event's fields as protocol.rs names them; a new one fails until listed. */
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
  sign_in_code: eventFields('sign_in_code')([
    'url',
    'url_complete',
    'user_code',
    'expires_in_secs',
    'browser_opened',
  ]),
  session_starting: eventFields('session_starting')([]),
  models: eventFields('models')(['current', 'models']),
  text_delta: eventFields('text_delta')(['text']),
  text_block_end: eventFields('text_block_end')([]),
  reasoning_started: eventFields('reasoning_started')(['chars', 'redacted']),
  reasoning_delta: eventFields('reasoning_delta')(['text']),
  tool_call_started: eventFields('tool_call_started')([
    'id',
    'name',
    'input',
    'cut_off',
  ]),
  tool_result: eventFields('tool_result')(['id', 'name', 'output', 'is_error']),
  usage: eventFields('usage')(['turn', 'cumulative', 'estimated']),
  permission_requested: eventFields('permission_requested')(['id', 'request']),
  notice: eventFields('notice')(['message']),
  turn_finished: eventFields('turn_finished')([]),
  error: eventFields('error')(['message', 'detail']),
  sessions: eventFields('sessions')(['current', 'sessions', 'scope']),
  transcript: eventFields('transcript')(['session_id', 'entries', 'truncated']),
  workspace_trusted: eventFields('workspace_trusted')([
    'cwd',
    'restart_required',
    'root',
    'session_only',
  ]),
  question: eventFields('question')(['id', 'question', 'options']),
  todos: eventFields('todos')(['items']),
  subagent_started: eventFields('subagent_started')([
    'id',
    'description',
    'subagent_type',
  ]),
  subagent_finished: eventFields('subagent_finished')([
    'id',
    'status',
    'is_error',
    'result',
    'note',
  ]),
  extensions: eventFields('extensions')([
    'mcp_servers',
    'plugins',
    'skills',
    'agents',
  ]),
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
    expect(sorted(rustVariants('UiEvent').keys())).toEqual(
      sorted(UI_EVENT_TYPES),
    )
  })

  test('every EngineCommand variant has a tag here, and no more', () => {
    expect(sorted(rustVariants('EngineCommand').keys())).toEqual(
      sorted(ENGINE_COMMAND_TYPES),
    )
  })

  test('every UiEvent variant has the fields listed here', () => {
    const rust = rustVariants('UiEvent')
    for (const type of UI_EVENT_TYPES) {
      expect({ type, fields: sorted(rust.get(type) ?? []) }).toEqual({
        type,
        fields: sorted(EVENT_FIELDS[type]),
      })
    }
  })

  test('every struct has the fields listed here', () => {
    const rust = rustStructs()
    expect(sorted(rust.keys())).toEqual(sorted(Object.keys(STRUCT_FIELDS)))
    for (const [name, fields] of rust) {
      expect({ name, fields: sorted(fields) }).toEqual({
        name,
        fields: sorted(STRUCT_FIELDS[name] ?? []),
      })
    }
  })

  test('the protocol version matches', () => {
    expect(protocolRs).toContain('pub const PROTOCOL_VERSION: u32 = 1;')
  })

  test('the fake engine samples parse, with only the fields known here', () => {
    const lines = fixtureLines('protocol-v2-events.jsonl')
    expect(lines.length).toBeGreaterThan(0)
    for (const line of lines) {
      const parsed = parseUiEventLine(line)
      expect(parsed.ok).toBe(true)
      if (!parsed.ok) continue
      const known = new Set(['type', ...EVENT_FIELDS[parsed.event.type]])
      expect(
        Object.keys(parsed.event).filter((key) => !known.has(key)),
      ).toEqual([])
    }
    for (const line of fixtureLines('protocol-v2-commands.jsonl')) {
      const message = JSON.parse(line)
      expect(message.kind).toBe('command')
      expect(ENGINE_COMMAND_TYPES).toContain(message.type)
    }
  })
})

// The JSON in these tests is copied from the serde tests in
// rust/crates/neuron-tui/src/protocol.rs: if one side changes, these fail.
describe('client messages (UI -> engine)', () => {
  test('commands are flat lines tagged by kind, then type', () => {
    expect(
      encodeClientMessage(
        commandMessage({ type: 'submit', text: 'hi', attachments: [] }),
      ),
    ).toBe('{"kind":"command","type":"submit","text":"hi","attachments":[]}\n')
    expect(encodeClientMessage(commandMessage({ type: 'interrupt' }))).toBe(
      '{"kind":"command","type":"interrupt"}\n',
    )
    expect(
      JSON.parse(
        encodeClientMessage(
          commandMessage({ type: 'slash', command: 'model', args: 'opus' }),
        ),
      ),
    ).toEqual({ kind: 'command', type: 'slash', command: 'model', args: 'opus' })
  })

  test('protocol v2 commands are the lines the engine reads', () => {
    expect(
      encodeClientMessage(commandMessage({ type: 'list_sessions', scope: 'all' })),
    ).toBe('{"kind":"command","type":"list_sessions","scope":"all"}\n')
    expect(encodeClientMessage(commandMessage({ type: 'trust_preview' }))).toBe(
      '{"kind":"command","type":"trust_preview"}\n',
    )
    expect(
      encodeClientMessage(
        commandMessage({
          type: 'capabilities',
          features: ['questions', 'reports'],
          client: 'terminal',
        }),
      ),
    ).toBe(
      '{"kind":"command","type":"capabilities","features":["questions","reports"],"client":"terminal"}\n',
    )
  })

  test('attachments are tagged by kind', () => {
    const line = encodeClientMessage(
      commandMessage({
        type: 'submit',
        text: 'fix the test',
        attachments: [
          { kind: 'image', path: 'shot.png', media_type: 'image/png' },
        ],
      }),
    )
    expect(JSON.parse(line).attachments[0]).toEqual({
      kind: 'image',
      path: 'shot.png',
      media_type: 'image/png',
    })
  })

  test('permission replies carry the id and a kind-tagged decision', () => {
    expect(
      encodeClientMessage(
        permissionReplyMessage({ id: 3, decision: { kind: 'allow_once' } }),
      ),
    ).toBe(
      '{"kind":"permission_reply","id":3,"decision":{"kind":"allow_once"}}\n',
    )
    expect(
      JSON.parse(
        encodeClientMessage(
          permissionReplyMessage({
            id: 7,
            decision: { kind: 'deny', note: 'use a dry run' },
          }),
        ),
      ).decision,
    ).toEqual({ kind: 'deny', note: 'use a dry run' })
  })
})

describe('UI events (engine -> UI)', () => {
  test('known events parse, with null for serde None', () => {
    expect(
      parseUiEventLine('{"type":"ready","protocol":1,"version":"0.9.0"}'),
    ).toEqual({
      ok: true,
      event: { type: 'ready', protocol: 1, version: '0.9.0' },
    })
    expect(
      parseUiEventLine(
        '{"type":"tool_call_started","id":"toolu_1","name":"bash","input":"{\\"command\\":\\"cargo test\\"}","cut_off":false}',
      ),
    ).toEqual({
      ok: true,
      event: {
        type: 'tool_call_started',
        id: 'toolu_1',
        name: 'bash',
        input: '{"command":"cargo test"}',
        cut_off: false,
      },
    })
    const reasoning = parseUiEventLine(
      '{"type":"reasoning_started","chars":null,"redacted":true}',
    )
    expect(reasoning.ok && reasoning.event).toEqual({
      type: 'reasoning_started',
      chars: null,
      redacted: true,
    })
    expect(parseUiEventLine('{"type":"text_block_end"}').ok).toBe(true)
  })

  test('an event type this UI does not know is reported, not thrown', () => {
    expect(parseUiEventLine('{"type":"from_the_future","url":"x"}')).toEqual({
      ok: false,
      reason: 'unknown_type',
      type: 'from_the_future',
    })
  })

  test('a line that is not a tagged object is invalid', () => {
    expect(parseUiEventLine('not json')).toMatchObject({
      ok: false,
      reason: 'invalid',
    })
    expect(parseUiEventLine('{"message":"untagged"}')).toMatchObject({
      ok: false,
      reason: 'invalid',
    })
  })

  test('the known-type list covers every UiEvent variant', () => {
    // A compile-time check: adding a variant without listing it fails here.
    const all: Record<UiEventType, true> = {
      ready: true,
      status: true,
      sign_in_code: true,
      session_starting: true,
      models: true,
      text_delta: true,
      text_block_end: true,
      reasoning_started: true,
      reasoning_delta: true,
      tool_call_started: true,
      tool_result: true,
      usage: true,
      permission_requested: true,
      notice: true,
      turn_finished: true,
      error: true,
      sessions: true,
      transcript: true,
      workspace_trusted: true,
      question: true,
      todos: true,
      subagent_started: true,
      subagent_finished: true,
      extensions: true,
      report: true,
      account: true,
      trust_preview: true,
    }
    expect([...UI_EVENT_TYPES].sort() as string[]).toEqual(
      Object.keys(all).sort(),
    )
  })
})

describe('JsonLineSplitter', () => {
  test('joins lines split across chunks and drops CRLF', () => {
    const splitter = new JsonLineSplitter()
    expect(splitter.push('{"type":"notice","mess')).toEqual([])
    expect(splitter.push('age":"hi"}\r\n{"type":"turn_fin')).toEqual([
      '{"type":"notice","message":"hi"}',
    ])
    expect(splitter.push('ished"}\n\n')).toEqual(['{"type":"turn_finished"}'])
    expect(splitter.flush()).toBeNull()
  })

  test('flush returns an unterminated tail', () => {
    const splitter = new JsonLineSplitter()
    splitter.push('{"type":"error","message":"boom"}')
    expect(splitter.flush()).toBe('{"type":"error","message":"boom"}')
  })
})

describe('engine client', () => {
  test('launch arguments mirror the engine flags', () => {
    expect(engineArgs()).toEqual(['engine', '--stdio'])
    expect(
      engineArgs({
        model: 'opus',
        provider: 'zero-x',
        permissionMode: 'workspace-write',
        allowedTools: ['bash', 'read_file'],
        reasoningEffort: 'high',
        resume: 'session-1',
        allowBroadCwd: true,
        dangerouslySkipPermissions: true,
      }),
    ).toEqual([
      'engine',
      '--stdio',
      '--model',
      'opus',
      '--provider',
      'zero-x',
      '--permission-mode',
      'workspace-write',
      '--allowedTools',
      'bash,read_file',
      '--reasoning-effort',
      'high',
      '--resume',
      'session-1',
      '--allow-broad-cwd',
      '--dangerously-skip-permissions',
    ])
  })

  test('a client that is not running refuses commands but stops cleanly', async () => {
    const client = new StdioEngineClient({ resolveBinary: () => null })
    expect(client.connection).toBe('not_started')
    expect(() => client.submit('hello')).toThrow(EngineNotConnectedError)
    await expect(client.start()).rejects.toThrow('Could not find the `neuron` binary')
    await expect(
      runTurn(client, { text: 'hello' }, () => {}),
    ).rejects.toBeInstanceOf(EngineNotConnectedError)
    expect(() => client.interrupt()).not.toThrow()
    expect(await client.shutdown()).toEqual({ code: 0, signal: null })
  })
})
