import { describe, expect, test } from 'bun:test'

import {
  EngineNotConnectedError,
  engineArgs,
  runTurn,
} from '../neuron-engine-client'
import { StdioEngineClient } from '../stdio-engine-client'
import {
  JsonLineSplitter,
  UI_EVENT_TYPES,
  commandMessage,
  encodeClientMessage,
  parseUiEventLine,
  permissionReplyMessage,
  type UiEventType,
} from '../protocol'

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
    expect(parseUiEventLine('{"type":"sign_in_code","url":"x"}')).toEqual({
      ok: false,
      reason: 'unknown_type',
      type: 'sign_in_code',
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
