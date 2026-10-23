/**
 * The phone's pairing and session code against a real Neuron host. The host
 * runs a fake engine here; ui/neuron-client's integration tests cover the same
 * host with the real engine.
 */
import { afterEach, describe, expect, test } from 'bun:test'
import fs from 'fs'
import os from 'os'
import path from 'path'

import { SessionController, type ClientMessage, type EngineTransport, type SessionAction, type TransportClose, type UiEvent } from '@neuron/client'
import { NeuronHost } from '../../neuron-client/src/node/host'
import { connect, normalizeHostUrl } from '../src/connection'

class FakeEngine implements EngineTransport {
  readonly echoesActions = false
  sent: ClientMessage[] = []
  private listeners = new Set<(action: SessionAction) => void>()
  async start() {
    this.emit({ type: 'ready', protocol: 1, version: '6.3.0' })
    this.emit({ type: 'status', model: 'm', permission_mode: 'workspace-write', session_id: 's', cwd: '/work/app', workspace_trusted: true, plan_mode: false, context_percent: 0, git: null })
  }
  emit(event: UiEvent) {
    for (const listener of [...this.listeners]) listener(event)
  }
  send(message: ClientMessage) {
    this.sent.push(message)
  }
  subscribe(listener: (action: SessionAction) => void) {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }
  onClose(_listener: (close: TransportClose) => void) {
    return () => {}
  }
  async close() {}
}

describe('normalizeHostUrl', () => {
  test('reads the address forms people type', () => {
    expect(normalizeHostUrl('192.168.1.5:51967')).toBe('ws://192.168.1.5:51967/neuron')
    expect(normalizeHostUrl(' ws://10.0.0.2:4000/neuron ')).toBe('ws://10.0.0.2:4000/neuron')
    expect(normalizeHostUrl('http://laptop.local:4000')).toBe('ws://laptop.local:4000/neuron')
    expect(normalizeHostUrl('https://relay.example/neuron')).toBe('wss://relay.example/neuron')
  })

  test('refuses what is not an address, and tokens in the URL', () => {
    expect(() => normalizeHostUrl('')).toThrow('Enter the address')
    expect(() => normalizeHostUrl('ws://host:1/neuron?token=abc')).toThrow('own field')
    expect(() => normalizeHostUrl('ws://[bad')).toThrow('not an address')
  })
})

describe('the phone on a host', () => {
  let host: NeuronHost | null = null
  let root = ''
  const controllers: SessionController[] = []
  afterEach(async () => {
    for (const controller of controllers.splice(0)) await controller.close()
    await host?.stop()
    host = null
    fs.rmSync(root, { recursive: true, force: true })
  })

  async function startHost() {
    root = fs.mkdtempSync(path.join(os.tmpdir(), 'neuron-mobile-test-'))
    const engine = new FakeEngine()
    host = new NeuronHost({ transport: engine, token: 'phone-pairing-token', root, name: 'laptop' })
    await host.start()
    return engine
  }

  test('pairs, sees the session, steers it and approves a call', async () => {
    const engine = await startHost()
    const address = host!.url.replace('ws://', '').replace('/neuron', '')
    const { controller, transport } = await connect(address, ' phone-pairing-token ')
    controllers.push(controller)
    expect(transport.host?.name).toBe('laptop')
    expect(controller.getState().status?.cwd).toBe('/work/app')

    controller.send('run the tests')
    const input = '{"command":"cargo test"}'
    engine.emit({ type: 'tool_call_started', id: 't', name: 'bash', input, cut_off: false })
    engine.emit({
      type: 'permission_requested',
      id: 1,
      request: { tool: 'bash', current_mode: 'workspace-write', required_mode: 'danger-full-access', reason: null, input, session_prefixes: ['cargo test'] },
    })
    const deadline = Date.now() + 5_000
    while (controller.getState().permissions.length === 0) {
      if (Date.now() > deadline) throw new Error('no permission request')
      await Bun.sleep(10)
    }
    controller.answerPermission(1, { kind: 'allow_for_session' })
    while (engine.sent.length < 2) {
      if (Date.now() > deadline) throw new Error('the engine got no reply')
      await Bun.sleep(10)
    }
    expect(engine.sent).toEqual([
      { kind: 'command', type: 'submit', text: 'run the tests', attachments: [] },
      { kind: 'permission_reply', id: 1, decision: { kind: 'allow_for_session' } },
    ])
  })

  test('a wrong token is refused with a message the screen can show', async () => {
    await startHost()
    const failure = await connect(host!.url, 'not-the-token').then(
      () => 'connected',
      (error: Error) => error.message,
    )
    expect(failure).toBe('The pairing token is not right.')
    expect(await connect(host!.url, '  ').catch((error: Error) => error.message)).toBe('Enter the pairing token from the Neuron host.')
  })

  test("answers the agent's question and switches the computer's chat", async () => {
    const engine = await startHost()
    const { controller } = await connect(host!.url, 'phone-pairing-token')
    controllers.push(controller)
    engine.emit({ type: 'question', id: 5, question: 'Which branch?', options: ['main', 'dev'] })
    const deadline = Date.now() + 5_000
    const waitFor = async (label: string, ok: () => boolean) => {
      while (!ok()) {
        if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`)
        await Bun.sleep(10)
      }
    }
    await waitFor('the question', () => controller.getState().items.some((item) => item.kind === 'question'))
    controller.answer(5, 'dev')
    await waitFor('the answer', () => engine.sent.some((message) => message.kind === 'command' && message.type === 'answer'))
    expect(engine.sent.at(-1)).toEqual({ kind: 'command', type: 'answer', id: 5, text: 'dev' })

    controller.listSessions()
    await waitFor('the chat list request', () => engine.sent.some((message) => message.kind === 'command' && message.type === 'list_sessions'))
    engine.emit({
      type: 'sessions',
      current: 's',
      sessions: [{ id: 'old', title: 'Fix the build', model: null, message_count: 3, updated_at_ms: 1, current: false, branch: null }],
    })
    await waitFor('the chats', () => controller.getState().sessions?.length === 1)
    controller.resume('old')
    await waitFor('the resume', () => engine.sent.some((message) => message.kind === 'command' && message.type === 'resume'))
    expect(engine.sent.at(-1)).toEqual({ kind: 'command', type: 'resume', session: 'old' })
  })
})
