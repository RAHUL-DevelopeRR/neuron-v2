/**
 * The real `neuron engine --stdio` against the mock Anthropic service, driven
 * through {@link StdioEngineClient}. Needs both binaries built
 * (`cargo build -p rusty-claude-cli -p mock-anthropic-service` in rust/), or their paths
 * in NEURON_BIN and NEURON_MOCK_BIN; skipped when they are missing. Run with
 * `bun test src/engine/__tests__/engine.integration.test.ts`.
 */
import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import fs from 'fs'
import os from 'os'
import path from 'path'

import { runTurn } from '../neuron-engine-client'
import { StdioEngineClient, type SpawnEngine } from '../stdio-engine-client'
import { mapToolCall, mapToolResult } from '../tool-mapping'

import type { UiEvent } from '../protocol'

const exe = (name: string) => (process.platform === 'win32' ? `${name}.exe` : name)
const debugDir = path.resolve(import.meta.dir, '../../../../../rust/target/debug')
const neuronBin = process.env.NEURON_BIN || path.join(debugDir, exe('neuron'))
const mockBin = process.env.NEURON_MOCK_BIN || path.join(debugDir, exe('mock-anthropic-service'))
const haveBinaries = fs.existsSync(neuronBin) && fs.existsSync(mockBin)

let mock: ReturnType<typeof Bun.spawn> | null = null
let baseUrl = ''
let home = ''

async function startMock(): Promise<void> {
  const child = Bun.spawn([mockBin], { stdout: 'pipe', stderr: 'ignore' })
  mock = child
  const reader = (child.stdout as ReadableStream<Uint8Array>).getReader()
  const decoder = new TextDecoder()
  let seen = ''
  while (!baseUrl) {
    const { value, done } = await reader.read()
    if (done) throw new Error('the mock Anthropic service exited before printing its URL')
    seen += decoder.decode(value)
    baseUrl = /MOCK_ANTHROPIC_BASE_URL=(\S+)/.exec(seen)?.[1] ?? ''
  }
  reader.releaseLock()
}

/** Spawns the engine with only the environment the mock needs. */
const spawnAgainstMock: SpawnEngine = (argv, options) =>
  Bun.spawn(argv, {
    cwd: options.cwd,
    env: {
      ANTHROPIC_API_KEY: 'integration-test-key',
      ANTHROPIC_BASE_URL: baseUrl,
      NEURON_CONFIG_HOME: path.join(home, 'config'),
      HOME: home,
      USERPROFILE: home,
      NO_COLOR: '1',
      PATH: process.env.PATH ?? '',
      SystemRoot: process.env.SystemRoot ?? '',
    },
    stdin: 'pipe',
    stdout: 'pipe',
    stderr: 'pipe',
  }) as unknown as ReturnType<SpawnEngine>

/** Runs one mock scenario in a fresh workspace; answers permission prompts with `allow`. */
async function runScenario(scenario: string, allow: boolean) {
  const workspace = fs.mkdtempSync(path.join(home, `ws-${scenario}-`))
  const client = new StdioEngineClient({
    spawn: spawnAgainstMock,
    resolveBinary: () => neuronBin,
  })
  const events: UiEvent[] = []
  const unsubscribe = client.subscribe((event) => {
    if (event.type === 'permission_requested') {
      client.replyToPermission({
        id: event.id,
        decision: allow ? { kind: 'allow_once' } : { kind: 'deny', note: 'integration test denies' },
      })
    }
  })
  try {
    const ready = await client.start({
      cwd: workspace,
      model: 'sonnet',
      permissionMode: 'workspace-write',
    })
    expect(ready.protocol).toBe(1)
    const outcome = await runTurn(client, { text: `PARITY_SCENARIO:${scenario}` }, (event) =>
      events.push(event),
    )
    return { outcome, events, workspace }
  } finally {
    unsubscribe()
    const exit = await client.shutdown()
    expect(exit.code).toBe(0)
  }
}

const text = (events: UiEvent[]) =>
  events
    .filter((e): e is Extract<UiEvent, { type: 'text_delta' }> => e.type === 'text_delta')
    .map((e) => e.text)
    .join('')

describe.skipIf(!haveBinaries)('neuron engine --stdio against the mock service', () => {
  beforeAll(async () => {
    home = fs.mkdtempSync(path.join(os.tmpdir(), 'neuron-tui-integration-'))
    await startMock()
  })

  afterAll(() => {
    mock?.kill()
    if (home) fs.rmSync(home, { recursive: true, force: true })
  })

  test('streaming text arrives as deltas and the turn ends cleanly', async () => {
    const { outcome, events } = await runScenario('streaming_text', true)
    expect(outcome.error).toBeNull()
    expect(text(events).length).toBeGreaterThan(0)
    expect(events.at(-1)?.type).toBe('turn_finished')
  }, 60_000)

  test('an approved bash call runs and maps onto the terminal renderer', async () => {
    const { outcome, events, workspace } = await runScenario('bash_permission_prompt_approved', true)
    expect(outcome.error).toBeNull()
    const call = events.find((e) => e.type === 'tool_call_started')
    const result = events.find((e) => e.type === 'tool_result')
    if (call?.type !== 'tool_call_started' || result?.type !== 'tool_result') {
      throw new Error(`no tool call and result in ${events.map((e) => e.type).join(', ')}`)
    }
    expect(mapToolCall(call.name, call.input).toolName).toBe('run_terminal_command')
    const [shown] = mapToolResult(result.name, result.output, result.is_error)
    expect(shown).toEqual({
      type: 'json',
      value: expect.objectContaining({ stdout: expect.stringContaining('approved via prompt') }),
    })
    expect(fs.existsSync(path.join(workspace, 'approved.txt'))).toBe(true)
  }, 60_000)

  test('a denied bash call gets no result and writes nothing', async () => {
    const { outcome, events, workspace } = await runScenario('bash_permission_prompt_denied', false)
    expect(outcome.error).toBeNull()
    expect(events.some((e) => e.type === 'tool_call_started')).toBe(true)
    expect(events.some((e) => e.type === 'tool_result')).toBe(false)
    expect(fs.readdirSync(workspace).filter((f) => f.endsWith('.txt'))).toEqual([])
  }, 60_000)

  test('usage and status follow a turn', async () => {
    const { events } = await runScenario('token_cost_reporting', true)
    const usage = events.find((e) => e.type === 'usage')
    if (usage?.type !== 'usage') throw new Error('no usage event')
    expect(usage.cumulative.input + usage.cumulative.output).toBeGreaterThan(0)
  }, 60_000)
})
