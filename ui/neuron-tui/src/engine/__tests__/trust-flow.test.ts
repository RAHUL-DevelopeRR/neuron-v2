import { afterEach, describe, expect, test } from 'bun:test'

import { UI_CAPABILITIES, UI_CLIENT } from '../capabilities'
import { getNeuronEngineClient } from '../neuron-engine-client'
import {
  handleEngineEvent,
  setEngineClientOptionsForTesting,
  startEngineSession,
  trustWorkspace,
} from '../engine-session'
import { useChatStore } from '../../state/chat-store'
import { useEngineStore } from '../../state/engine-store'
import { startScreen, useStartGate } from '../../state/start-gate'

import type { SpawnEngine } from '../stdio-engine-client'

const FEATURES = ['capabilities', 'trust_workspace', 'trust_preview', 'reports', 'account']

const status = (trusted: boolean, sessionId = 'session-1') => ({
  type: 'status',
  model: 'auto',
  permission_mode: 'workspace-write',
  session_id: sessionId,
  cwd: '/work/repo',
  workspace_trusted: trusted,
  plan_mode: false,
  context_percent: 0,
  git: null,
  signed_in: true,
  sign_in_required: false,
})

const PREVIEW = {
  type: 'trust_preview',
  cwd: '/work/repo',
  root: '/work/repo',
  is_git_root: true,
  session_only: false,
  trusted: false,
  settings_files: ['/work/repo/.neuron/settings.json'],
  hooks: [{ event: 'PreToolUse', command: 'lint', source: '/work/repo/.neuron/settings.json' }],
  mcp_servers: [{ name: 'postgres-tools', transport: 'stdio', source: '/work/repo/.neuron/settings.json' }],
  permission_rules: [],
  other_settings: [],
}

/**
 * Engines this test starts, like Neuron's: `ready` with the v2 features, then
 * a status; they answer `trust_preview` and `trust_workspace`. An engine whose
 * environment has NEURON_TRUST_WORKSPACE=1, or that is started after trust was
 * recorded (`trusted`), reports the folder trusted.
 */
function fakeEngines(options: { sessionOnly?: boolean; trustFails?: boolean } = {}) {
  const encoder = new TextEncoder()
  let trusted = false
  const started: {
    argv: string[]
    env: Record<string, string | undefined>
    sent: Array<Record<string, unknown>>
  }[] = []
  const spawn: SpawnEngine = (argv, spawnOptions) => {
    let out!: ReadableStreamDefaultController<Uint8Array>
    let err!: ReadableStreamDefaultController<Uint8Array>
    let exitWith!: (code: number) => void
    const exit = (code: number) => {
      try {
        out.close()
        err.close()
      } catch {
        // Already closed.
      }
      exitWith(code)
    }
    const record = { argv, env: spawnOptions.env, sent: [] as Array<Record<string, unknown>> }
    started.push(record)
    const isTrusted = () => trusted || spawnOptions.env.NEURON_TRUST_WORKSPACE === '1'
    const emit = (event: object) => out.enqueue(encoder.encode(JSON.stringify(event) + '\n'))
    const stdout = new ReadableStream<Uint8Array>({
      start: (controller) => {
        out = controller
        queueMicrotask(() => {
          emit({ type: 'ready', protocol: 1, version: 'test', features: FEATURES })
          emit(status(isTrusted()))
        })
      },
    })
    return {
      stdin: {
        write: (data: string) => {
          for (const line of data.split('\n').filter(Boolean)) {
            const message = JSON.parse(line) as Record<string, unknown>
            record.sent.push(message)
            queueMicrotask(() => {
              if (message.type === 'trust_preview') emit(PREVIEW)
              if (message.type === 'trust_workspace') {
                if (options.trustFails) {
                  emit({ type: 'error', message: 'could not trust /work/repo: access denied' })
                  return
                }
                if (!options.sessionOnly) trusted = true
                emit({
                  type: 'workspace_trusted',
                  cwd: '/work/repo',
                  restart_required: true,
                  root: '/work/repo',
                  session_only: Boolean(options.sessionOnly),
                })
                emit({ type: 'notice', message: 'Trusted /work/repo. Its settings, hooks and MCP servers load when the session restarts.' })
                emit(status(isTrusted()))
              }
            })
          }
        },
        flush: () => undefined,
        end: () => exit(0),
      },
      stdout,
      stderr: new ReadableStream<Uint8Array>({ start: (controller) => (err = controller) }),
      exited: new Promise<number>((resolve) => (exitWith = resolve)),
      signalCode: null,
      kill: () => exit(137),
    } as unknown as ReturnType<SpawnEngine>
  }
  return { spawn, started }
}

const waitUntil = async (predicate: () => boolean) => {
  const deadline = Date.now() + 2_000
  while (!predicate() && Date.now() < deadline) await new Promise((r) => setTimeout(r, 5))
  expect(predicate()).toBe(true)
}

const screenNow = () =>
  startScreen({
    ...useEngineStore.getState(),
    ...useStartGate.getState(),
  })

afterEach(async () => {
  await getNeuronEngineClient().shutdown().catch(() => undefined)
  setEngineClientOptionsForTesting({})
  useEngineStore.getState().reset()
  useStartGate.getState().reset()
  useChatStore.getState().reset()
})

describe('the folder trust flow against the engine', () => {
  test('an untrusted folder shows the trust screen, with what trusting would turn on', async () => {
    const engines = fakeEngines()
    setEngineClientOptionsForTesting({ spawn: engines.spawn, resolveBinary: () => 'neuron' })
    await startEngineSession({ cwd: process.cwd() })
    await waitUntil(() => useEngineStore.getState().trustPreview !== null)

    expect(screenNow()).toBe('trust')
    // One capabilities command, with reports among them; the preview was asked for once.
    const sent = engines.started[0]!.sent
    const capabilities = sent.filter((message) => message.type === 'capabilities')
    expect(capabilities).toEqual([
      { kind: 'command', type: 'capabilities', features: [...UI_CAPABILITIES], client: UI_CLIENT },
    ])
    expect(UI_CAPABILITIES).toContain('reports')
    expect(sent.filter((message) => message.type === 'trust_preview')).toHaveLength(1)
    expect(useEngineStore.getState().trustPreview?.mcp_servers[0]?.name).toBe('postgres-tools')
  })

  test('Allow records trust and restarts the engine with the folder settings; the chat follows', async () => {
    const engines = fakeEngines()
    setEngineClientOptionsForTesting({ spawn: engines.spawn, resolveBinary: () => 'neuron' })
    await startEngineSession({ cwd: process.cwd(), model: 'opus' })
    await waitUntil(() => screenNow() === 'trust')

    trustWorkspace()
    expect(screenNow()).toBe('trust')
    await waitUntil(() => engines.started.length === 2)
    await waitUntil(() => useEngineStore.getState().status?.workspace_trusted === true)

    expect(screenNow()).toBe('chat')
    expect(engines.started[0]!.sent).toContainEqual({ kind: 'command', type: 'trust_workspace' })
    // Same flags; no conversation yet, so no --resume; trust is stored, no env.
    const argv = engines.started[1]!.argv
    expect(argv).toContain('--model')
    expect(argv).not.toContain('--resume')
    expect(engines.started[1]!.env.NEURON_TRUST_WORKSPACE).toBeUndefined()
    // The engine's "Trusted … when the session restarts" is not a chat line.
    expect(useChatStore.getState().messages).toHaveLength(0)
  })

  test('with a prompt in the chat, the restart keeps its conversation', async () => {
    const engines = fakeEngines()
    setEngineClientOptionsForTesting({ spawn: engines.spawn, resolveBinary: () => 'neuron' })
    await startEngineSession({ cwd: process.cwd() })
    await waitUntil(() => screenNow() === 'trust')
    useChatStore.getState().setMessages([{ id: 'user-1', variant: 'user', content: 'hello' } as never])

    trustWorkspace()
    await waitUntil(() => engines.started.length === 2)
    const argv = engines.started[1]!.argv
    expect(argv.slice(argv.indexOf('--resume'), argv.indexOf('--resume') + 2)).toEqual(['--resume', 'session-1'])
  })

  test('the home folder is trusted for the restarted engine only', async () => {
    const engines = fakeEngines({ sessionOnly: true })
    setEngineClientOptionsForTesting({ spawn: engines.spawn, resolveBinary: () => 'neuron' })
    await startEngineSession({ cwd: process.cwd() })
    await waitUntil(() => screenNow() === 'trust')

    trustWorkspace()
    await waitUntil(() => engines.started.length === 2)
    await waitUntil(() => useEngineStore.getState().status?.workspace_trusted === true)
    expect(engines.started[1]!.env.NEURON_TRUST_WORKSPACE).toBe('1')
    expect(screenNow()).toBe('chat')
  })

  test('a failure to record trust stays on the trust screen and says why', async () => {
    const engines = fakeEngines({ trustFails: true })
    setEngineClientOptionsForTesting({ spawn: engines.spawn, resolveBinary: () => 'neuron' })
    await startEngineSession({ cwd: process.cwd() })
    await waitUntil(() => screenNow() === 'trust')

    trustWorkspace()
    await waitUntil(() => useStartGate.getState().trustStep.kind === 'failed')
    const step = useStartGate.getState().trustStep
    expect(step.kind === 'failed' && step.error).toContain('access denied')
    expect(screenNow()).toBe('trust')
    expect(engines.started).toHaveLength(1)
    expect(useChatStore.getState().messages).toHaveLength(0)
  })

  test('a report opens the report panel', () => {
    handleEngineEvent({
      type: 'report',
      command: 'usage',
      title: 'Usage today',
      sections: [{ heading: null, markdown: '**12** of 200 requests' }],
      links: [{ label: 'Dashboard', url: 'https://zero-x.live/dashboard' }],
    })
    expect(useEngineStore.getState().report?.title).toBe('Usage today')
    expect(useChatStore.getState().inputMode).toBe('report')
  })
})
