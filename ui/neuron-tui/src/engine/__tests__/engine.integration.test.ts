/**
 * The real `neuron engine --stdio`, driven through {@link StdioEngineClient}:
 * turns against the mock Anthropic service, and the Zero-X sign-in against a
 * local fake gateway (scripts/fake-zero-x-gateway.ts). Needs both binaries
 * built (`cargo build -p rusty-claude-cli -p mock-anthropic-service` in rust/),
 * or their paths in NEURON_BIN and NEURON_MOCK_BIN; skipped when they are
 * missing. Run with `bun test src/engine/__tests__/engine.integration.test.ts`.
 *
 * Every engine gets a throwaway home, file-based credentials and no browser,
 * so nothing here touches the real keychain, config or gateway.
 */
import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import fs from 'fs'
import os from 'os'
import path from 'path'

import {
  FAKE_USER_CODE,
  startFakeGateway,
  type FakeGateway,
} from '../../../scripts/fake-zero-x-gateway'
import { modelRows } from '../../components/model-picker'
import { needsSignIn } from '../../state/engine-store'
import { runExchange, runTurn } from '../neuron-engine-client'
import { StdioEngineClient, type SpawnEngine } from '../stdio-engine-client'
import { mapToolCall, mapToolResult } from '../tool-mapping'

import type { ModelsEvent, SignInCodeEvent, StatusEvent, UiEvent } from '../protocol'

const exe = (name: string) => (process.platform === 'win32' ? `${name}.exe` : name)
const debugDir = path.resolve(import.meta.dir, '../../../../../rust/target/debug')
const neuronBin = process.env.NEURON_BIN || path.join(debugDir, exe('neuron'))
const mockBin = process.env.NEURON_MOCK_BIN || path.join(debugDir, exe('mock-anthropic-service'))
const haveBinaries = fs.existsSync(neuronBin) && fs.existsSync(mockBin)

let mock: ReturnType<typeof Bun.spawn> | null = null
let gateway: FakeGateway | null = null
let mockUrl = ''
let root = ''

async function startMock(): Promise<void> {
  const child = Bun.spawn([mockBin], { stdout: 'pipe', stderr: 'ignore' })
  mock = child
  const reader = (child.stdout as ReadableStream<Uint8Array>).getReader()
  const decoder = new TextDecoder()
  let seen = ''
  while (!mockUrl) {
    const { value, done } = await reader.read()
    if (done) throw new Error('the mock Anthropic service exited before printing its URL')
    seen += decoder.decode(value)
    mockUrl = /MOCK_ANTHROPIC_BASE_URL=(\S+)/.exec(seen)?.[1] ?? ''
  }
  reader.releaseLock()
}

/** A spawn with a fresh home and only the environment given (plus what the OS needs). */
function isolatedSpawn(name: string, env: Record<string, string>): SpawnEngine {
  const home = fs.mkdtempSync(path.join(root, `home-${name}-`))
  return (argv, options) =>
    Bun.spawn(argv, {
      cwd: options.cwd,
      env: {
        NEURON_CONFIG_HOME: path.join(home, 'config'),
        NEURON_HOME: path.join(home, 'neuron'),
        NEURON_CREDENTIAL_STORE: 'file',
        NEURON_NO_BROWSER: '1',
        HOME: home,
        USERPROFILE: home,
        NO_COLOR: '1',
        PATH: process.env.PATH ?? '',
        SystemRoot: process.env.SystemRoot ?? '',
        ...env,
      },
      stdin: 'pipe',
      stdout: 'pipe',
      stderr: 'pipe',
    }) as unknown as ReturnType<SpawnEngine>
}

const workspaceFor = (name: string) => fs.mkdtempSync(path.join(root, `ws-${name}-`))

/** Runs one mock scenario in a fresh workspace; answers permission prompts with `allow`. */
async function runScenario(scenario: string, allow: boolean) {
  const workspace = workspaceFor(scenario)
  const client = new StdioEngineClient({
    spawn: isolatedSpawn(scenario, {
      ANTHROPIC_API_KEY: 'integration-test-key',
      ANTHROPIC_BASE_URL: mockUrl,
    }),
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

async function until(label: string, predicate: () => boolean, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`)
    await new Promise((resolve) => setTimeout(resolve, 20))
  }
}

describe.skipIf(!haveBinaries)('neuron engine --stdio', () => {
  beforeAll(async () => {
    root = fs.mkdtempSync(path.join(os.tmpdir(), 'neuron-tui-integration-'))
    gateway = startFakeGateway()
    await startMock()
  })

  afterAll(() => {
    mock?.kill()
    gateway?.stop()
    if (root) fs.rmSync(root, { recursive: true, force: true })
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
    expect(result.id).toBe(call.id)
    expect(mapToolCall(call.name, call.input).toolName).toBe('run_terminal_command')
    const [shown] = mapToolResult(result.name, result.output, result.is_error)
    expect(shown).toEqual({
      type: 'json',
      value: expect.objectContaining({ stdout: expect.stringContaining('approved via prompt') }),
    })
    expect(fs.existsSync(path.join(workspace, 'approved.txt'))).toBe(true)
  }, 60_000)

  test('a denied bash call gets an error result for its id and writes nothing', async () => {
    const { outcome, events, workspace } = await runScenario('bash_permission_prompt_denied', false)
    expect(outcome.error).toBeNull()
    const call = events.find((e) => e.type === 'tool_call_started')
    const result = events.find((e) => e.type === 'tool_result')
    if (call?.type !== 'tool_call_started' || result?.type !== 'tool_result') {
      throw new Error(`no tool call and result in ${events.map((e) => e.type).join(', ')}`)
    }
    expect(result.id).toBe(call.id)
    expect(result.is_error).toBe(true)
    expect(result.output).toContain('integration test denies')
    expect(fs.readdirSync(workspace).filter((f) => f.endsWith('.txt'))).toEqual([])
  }, 60_000)

  test('usage follows a turn', async () => {
    const { events } = await runScenario('token_cost_reporting', true)
    const usage = events.find((e) => e.type === 'usage')
    if (usage?.type !== 'usage') throw new Error('no usage event')
    expect(usage.cumulative.input + usage.cumulative.output).toBeGreaterThan(0)
  }, 60_000)

  test('signed out: prompts wait for /login, which signs in with a device code', async () => {
    const client = new StdioEngineClient({
      spawn: isolatedSpawn('sign-in', { NEURON_API_BASE: gateway!.base }),
      resolveBinary: () => neuronBin,
    })
    const statuses: StatusEvent[] = []
    client.subscribe((event) => {
      if (event.type === 'status') statuses.push(event)
    })
    try {
      await client.start({ cwd: workspaceFor('sign-in') })
      await until('the first status', () => statuses.length > 0)
      expect(needsSignIn(statuses[0]!)).toBe(true)

      const refused = await runTurn(client, { text: 'hello' }, () => {})
      expect(refused.error).toContain('/login')

      const codes: SignInCodeEvent[] = []
      const login = await runExchange(
        client,
        () => client.slash('login'),
        (event) => {
          if (event.type === 'sign_in_code') codes.push(event)
        },
      )
      expect(login.error).toBeNull()
      expect(codes).toHaveLength(1)
      expect(codes[0]!.user_code).toBe(FAKE_USER_CODE)
      expect(codes[0]!.browser_opened).toBe(false)

      await until('a signed-in status', () => statuses.at(-1)?.signed_in === true)
      const signedIn = statuses.at(-1)!
      expect(needsSignIn(signedIn)).toBe(false)
      expect(signedIn.account?.plan).toBe('pro')

      const models = await new Promise<ModelsEvent>((resolve) => {
        const off = client.subscribe((event) => {
          if (event.type === 'models') {
            off()
            resolve(event)
          }
        })
        client.listModels()
      })
      expect(modelRows(models).map((row) => [row.id, row.available])).toEqual([
        ['zx-fast', true],
        ['zx-big', false],
      ])

      await runExchange(client, () => client.slash('logout'), () => {})
      await until('a signed-out status', () => statuses.at(-1)?.signed_in === false)
      expect(gateway!.requests).toContain('DELETE /auth/session (signed)')
    } finally {
      await client.shutdown()
    }
  }, 90_000)
})
