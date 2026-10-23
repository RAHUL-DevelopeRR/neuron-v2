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
} from '../src/protocol'

const protocolRs = fs.readFileSync(
  path.resolve(import.meta.dir, '../../../rust/crates/neuron-tui/src/protocol.rs'),
  'utf8',
)

/** The variant names of `pub enum <name>` in protocol.rs, as serde's snake_case tags. */
function serdeTags(enumName: string): string[] {
  const start = protocolRs.indexOf(`pub enum ${enumName} {`)
  expect(start).toBeGreaterThan(-1)
  let depth = 0
  let body = ''
  for (let index = protocolRs.indexOf('{', start); index < protocolRs.length; index++) {
    const char = protocolRs[index]
    if (char === '{') depth++
    if (char === '}') depth--
    body += char
    if (depth === 0) break
  }
  // Variants sit at depth 1: a capitalised name at the start of a line, after its doc comments.
  const names: string[] = []
  let level = 0
  for (const line of body.split('\n')) {
    const trimmed = line.trim()
    if (level === 1 && /^[A-Z]\w*\s*(\{|\(|,|$)/.test(trimmed)) names.push(/^[A-Z]\w*/.exec(trimmed)![0])
    for (const char of line) {
      if (char === '{') level++
      if (char === '}') level--
    }
  }
  return names.map((name) => name.replace(/[A-Z]/g, (letter, offset) => (offset ? '_' : '') + letter.toLowerCase()))
}

describe('the protocol mirrors protocol.rs', () => {
  test('every UiEvent variant has a tag here, and no more', () => {
    expect([...serdeTags('UiEvent')].sort()).toEqual([...UI_EVENT_TYPES].sort())
  })

  test('every EngineCommand variant has a tag here, and no more', () => {
    expect([...serdeTags('EngineCommand')].sort()).toEqual([...ENGINE_COMMAND_TYPES].sort())
  })

  test('the protocol version matches', () => {
    expect(protocolRs).toContain('pub const PROTOCOL_VERSION: u32 = 1;')
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
      { kind: 'permission_reply', id: 'one', decision: { kind: 'allow_once' } },
      { kind: 'permission_reply', id: 1, decision: { kind: 'allow_forever' } },
      { kind: 'other' },
    ]) {
      expect(parseClientMessage(bad)).toBeNull()
    }
  })
})
