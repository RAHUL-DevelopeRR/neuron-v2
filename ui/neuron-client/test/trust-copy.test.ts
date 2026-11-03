import { describe, expect, test } from 'bun:test'
import fs from 'fs'
import path from 'path'

import { trustChoices, trustCopy, trustPreviewLines, trustRootNote } from '../src/copy/trust'
import { parseUiEventLine, type TrustPreviewEvent } from '../src/protocol'

/** The fake engine's `trust_preview` sample (rust/crates/neuron-tui/fixtures). */
function samplePreview(): TrustPreviewEvent {
  const fixture = path.resolve(import.meta.dir, '../../../rust/crates/neuron-tui/fixtures/protocol-v2-events.jsonl')
  for (const line of fs.readFileSync(fixture, 'utf8').split(/\r?\n/).filter(Boolean)) {
    const parsed = parseUiEventLine(line)
    if (parsed.ok && parsed.event.type === 'trust_preview') return parsed.event
  }
  throw new Error('no trust_preview sample')
}

describe('the trust screen copy', () => {
  test('lists what the sample project would turn on', () => {
    const preview = samplePreview()
    expect(trustPreviewLines(preview)).toEqual([
      '2 hooks that run commands around tool calls',
      '1 MCP server: postgres-tools',
      '3 permission rules from .neuron/settings.json',
      'Other settings: env',
    ])
    expect(trustRootNote(preview)).toBe('(git root: this folder)')
  })

  test('counts one of each in the singular, and several files as a number', () => {
    const preview: TrustPreviewEvent = {
      ...samplePreview(),
      cwd: 'C:\\work\\app\\web',
      root: 'C:\\work\\app',
      hooks: [{ event: 'PreToolUse', command: 'lint', source: 'C:\\work\\app\\web\\.neuron\\settings.json' }],
      mcp_servers: [
        { name: 'docs', transport: 'stdio', target: 'node', source: 'a' },
        { name: 'search', transport: 'http', target: 'search.example.com', source: 'a' },
      ],
      permission_rules: [
        { effect: 'allow', rule: 'bash(npm test)', source: 'C:\\work\\app\\web\\.neuron\\settings.json' },
        { effect: 'ask', rule: 'write_file', source: 'C:\\work\\app\\web\\.neuron.json' },
      ],
      other_settings: [],
    }
    expect(trustPreviewLines(preview)).toEqual([
      '1 hook that runs commands around tool calls',
      '2 MCP servers: docs, search',
      '2 permission rules from 2 settings files',
    ])
    expect(trustRootNote(preview)).toBe('(git root: C:\\work\\app)')
    expect(trustRootNote({ ...preview, is_git_root: false })).toBe('')
  })

  test('a project that adds nothing has no lines', () => {
    const preview = { ...samplePreview(), hooks: [], mcp_servers: [], permission_rules: [], other_settings: [] }
    expect(trustPreviewLines(preview)).toEqual([])
    expect(trustCopy.turnsOnNothing).toContain('does not turn on')
  })

  test('the terminal quits; the desktop offers another folder', () => {
    expect(trustChoices('terminal').map((choice) => choice.label)).toEqual(['Allow Neuron in this folder', 'Quit'])
    expect(trustChoices('desktop').map((choice) => choice.id)).toEqual(['allow', 'open_other'])
  })

  test('never names another product', () => {
    const words = JSON.stringify(trustCopy)
    expect(words).not.toMatch(/claude|anthropic/i)
  })
})
