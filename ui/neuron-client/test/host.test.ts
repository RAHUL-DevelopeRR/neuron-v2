import { afterEach, describe, expect, test } from 'bun:test'
import fs from 'fs'
import os from 'os'
import path from 'path'

import { SessionController, type EngineTransport, type TransportClose } from '../src/controller'
import { NeuronHost } from '../src/node/host'
import type { ClientMessage, UiEvent } from '../src/protocol'
import { WebSocketTransport, type HostFrame } from '../src/remote'
import type { SessionAction } from '../src/session'

/** An engine in memory: `ready` and a `status` on start, and whatever the test emits. */
class FakeEngine implements EngineTransport {
  readonly echoesActions = false
  sent: ClientMessage[] = []
  private listeners = new Set<(action: SessionAction) => void>()
  private closeListeners = new Set<(close: TransportClose) => void>()

  constructor(
    private readonly features: string[] = [],
    private readonly trusted = true,
  ) {}

  async start() {
    this.emit({ type: 'ready', protocol: 1, version: '6.3.0', features: this.features })
    this.emit({
      type: 'status',
      model: 'm',
      permission_mode: 'workspace-write',
      session_id: 's',
      cwd: '/ws',
      workspace_trusted: this.trusted,
      plan_mode: false,
      context_percent: 0,
      git: null,
    })
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
  onClose(listener: (close: TransportClose) => void) {
    this.closeListeners.add(listener)
    return () => this.closeListeners.delete(listener)
  }
  async close() {
    for (const listener of [...this.closeListeners]) listener({ code: 0, error: null })
  }
}

const TOKEN = 'test-pairing-token-0123456789'
let host: NeuronHost | null = null
let engine: FakeEngine
let root = ''
const controllers: SessionController[] = []

/** Engines the host's restart made, with the session each was asked to continue. */
let restarted: Array<{ engine: FakeEngine; resume: string | null }> = []

async function startHost(options: { remoteOnly?: boolean; features?: string[]; trusted?: boolean } = {}) {
  root = fs.mkdtempSync(path.join(os.tmpdir(), 'neuron-host-test-'))
  fs.mkdirSync(path.join(root, 'src'))
  fs.writeFileSync(path.join(root, 'src', 'main.rs'), 'fn main() {}\n')
  fs.writeFileSync(path.join(root, 'README.md'), '# hi\n')
  engine = new FakeEngine(options.features, options.trusted)
  restarted = []
  host = new NeuronHost({
    transport: engine,
    token: TOKEN,
    root,
    name: 'test host',
    ...(options.remoteOnly && { isLocal: () => false }),
    restart: (resume) => {
      const next = new FakeEngine(options.features, true)
      restarted.push({ engine: next, resume })
      return next
    },
  })
  await host.start()
  return host
}

async function attach(kind: 'desktop' | 'mobile', token = TOKEN) {
  const transport = new WebSocketTransport({ url: host!.url, token, client: { name: kind, kind }, timeoutMs: 5_000 })
  const controller = new SessionController(transport)
  controllers.push(controller)
  await controller.start()
  return { transport, controller }
}

const until = async (label: string, predicate: () => boolean) => {
  const deadline = Date.now() + 5_000
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`)
    await Bun.sleep(10)
  }
}

afterEach(async () => {
  for (const controller of controllers.splice(0)) await controller.close().catch(() => {})
  await host?.stop()
  host = null
  fs.rmSync(root, { recursive: true, force: true })
})

describe('NeuronHost', () => {
  test('a client attaches with the token and gets the session as it stands', async () => {
    await startHost()
    engine.emit({ type: 'text_delta', text: 'earlier output' })
    const { transport, controller } = await attach('mobile')
    expect(transport.host?.name).toBe('test host')
    expect(controller.getState().connection).toBe('ready')
    expect(controller.getState().status?.model).toBe('m')
    expect(controller.getState().items.map((item) => item.kind)).toEqual(['assistant'])
  })

  test('a wrong token is refused and the socket closed', async () => {
    await startHost()
    await expect(attach('mobile', 'wrong-token-wrong-token-wrong')).rejects.toThrow('pairing token')
  })

  test('frames before hello close the socket', async () => {
    await startHost()
    const socket = new WebSocket(host!.url)
    const closed = new Promise<number>((resolve) => (socket.onclose = (event) => resolve(event.code)))
    socket.onopen = () => socket.send(JSON.stringify({ t: 'send', message: { kind: 'command', type: 'interrupt' } }))
    expect(await closed).toBe(4400)
    expect(engine.sent).toEqual([])
  })

  test('a browser page from another origin cannot connect', async () => {
    await startHost()
    const response = await fetch(host!.url.replace('ws://', 'http://'), {
      headers: { origin: 'https://evil.example', upgrade: 'websocket', connection: 'Upgrade', 'sec-websocket-key': 'dGhlIHNhbXBsZSBub25jZQ==', 'sec-websocket-version': '13' },
    })
    expect(response.status).toBe(403)
  })

  test('a prompt from one client reaches the engine and every client sees it', async () => {
    await startHost()
    const desktop = await attach('desktop')
    const mobile = await attach('mobile')
    mobile.controller.send('fix the build')
    await until('the engine got the prompt', () => engine.sent.length === 1)
    expect(engine.sent[0]).toEqual({ kind: 'command', type: 'submit', text: 'fix the build', attachments: [] })
    for (const client of [desktop, mobile]) {
      await until('the prompt in the transcript', () => client.controller.getState().items.some((item) => item.kind === 'user'))
      // Once each: the sender does not add its own copy.
      expect(client.controller.getState().items.filter((item) => item.kind === 'user')).toHaveLength(1)
      expect(client.controller.getState().busy).toBe(true)
    }
    mobile.controller.send('/compact')
    await until('the slash command', () => engine.sent.length === 2)
    expect(engine.sent[1]).toEqual({ kind: 'command', type: 'slash', command: 'compact', args: '' })
  })

  test('the first answer to a permission request wins', async () => {
    await startHost()
    const desktop = await attach('desktop')
    const mobile = await attach('mobile')
    const input = '{"command":"rm -rf target"}'
    engine.emit({ type: 'tool_call_started', id: 't1', name: 'bash', input, cut_off: false })
    engine.emit({
      type: 'permission_requested',
      id: 7,
      request: { tool: 'bash', current_mode: 'workspace-write', required_mode: 'danger-full-access', reason: null, input, session_prefixes: [] },
    })
    await until('both see the request', () => [desktop, mobile].every((c) => c.controller.getState().permissions.length === 1))
    mobile.controller.answerPermission(7, { kind: 'deny', note: 'not from the phone' })
    desktop.controller.answerPermission(7, { kind: 'allow_once' })
    await until('both see it answered', () => [desktop, mobile].every((c) => c.controller.getState().permissions.length === 0))
    await Bun.sleep(50)
    expect(engine.sent.filter((message) => message.kind === 'permission_reply')).toEqual([
      { kind: 'permission_reply', id: 7, decision: { kind: 'deny', note: 'not from the phone' } },
    ])
    const call = desktop.controller.getState().items.find((item) => item.kind === 'tool')
    expect(call && call.kind === 'tool' && call.state).toBe('denied')
  })

  test('remote clients cannot shut the engine down', async () => {
    await startHost()
    const { transport } = await attach('mobile')
    const errors: string[] = []
    const socket = new WebSocket(host!.url)
    socket.onopen = () => {
      socket.send(JSON.stringify({ t: 'hello', protocol: 1, token: TOKEN, client: { name: 'raw', kind: 'cli' } }))
      socket.send(JSON.stringify({ t: 'send', message: { kind: 'command', type: 'shutdown' } }))
    }
    socket.onmessage = (event) => {
      const frame = JSON.parse(String(event.data)) as HostFrame
      if (frame.t === 'error') errors.push(frame.message)
    }
    await until('the refusal', () => errors.length > 0)
    socket.close()
    expect(errors[0]).toContain('Only the machine running Neuron')
    expect(engine.sent).toEqual([])
    expect(transport.clients).toBeGreaterThan(0)
  })

  test('a client that reconnects catches up from where it was', async () => {
    await startHost()
    const { transport, controller } = await attach('mobile')
    await transport.close()
    await until('closed', () => controller.getState().connection === 'exited')
    engine.emit({ type: 'text_delta', text: 'while you were away' })
    engine.emit({ type: 'turn_finished' })
    await transport.start()
    const state = controller.getState()
    expect(state.connection).toBe('ready')
    expect(state.items).toMatchObject([{ kind: 'assistant', text: 'while you were away', streaming: false }])
    // Live again after the snapshot.
    engine.emit({ type: 'notice', message: 'back' })
    await until('the live notice', () => controller.getState().items.length === 2)
  })

  test('fs.list and fs.read stay inside the workspace', async () => {
    await startHost()
    const { transport } = await attach('desktop')
    const listing = await transport.rpc<{ entries: Array<{ name: string; dir: boolean; path: string }> }>('fs.list', { path: '.' })
    expect(listing.entries.map((entry) => [entry.name, entry.dir])).toEqual([
      ['src', true],
      ['README.md', false],
    ])
    const file = await transport.rpc<{ content: string; path: string }>('fs.read', { path: 'src/main.rs' })
    expect(file).toMatchObject({ content: 'fn main() {}\n', path: 'src/main.rs' })
    const failure = (call: Promise<unknown>) => call.then(() => 'resolved', (error: Error) => error.message)
    expect(await failure(transport.rpc('fs.read', { path: '../outside.txt' }))).toMatch(/No such file|outside the workspace/)
    expect(await failure(transport.rpc('fs.list', { path: os.tmpdir() }))).toBe('That path is outside the workspace.')
    expect(await failure(transport.rpc('shell.run', { command: 'whoami' }))).toBe('Unknown method shell.run.')
  })

  test('a client on this machine gets a real terminal in the workspace', async () => {
    await startHost()
    const { transport } = await attach('desktop')
    let output = ''
    let exited: number | null | undefined
    transport.onTerminal((frame) => {
      if (frame.t === 'term_output') output += frame.data
      else exited = frame.code
    })
    const { id } = await transport.rpc<{ id: number }>('term.open', { cols: 100, rows: 30 })
    transport.terminalResize(id, 120, 40)
    transport.terminalInput(id, 'echo neuron-term-$(1+1)\r')
    await until('the echo', () => output.includes('neuron-term-2'))
    transport.terminalInput(id, 'exit\r')
    await until('the shell to exit', () => exited !== undefined)
  }, 20_000)

  test('a client from another machine gets no terminal', async () => {
    await startHost({ remoteOnly: true })
    const { transport } = await attach('mobile')
    const failure = await transport.rpc('term.open', {}).then(() => 'opened', (error: Error) => error.message)
    expect(failure).toBe('Terminals are only available on the machine running Neuron.')
  })

  test('the host tells an engine that takes them: its clients answer questions', async () => {
    await startHost({ features: ['capabilities', 'sessions', 'trust_workspace'] })
    expect(engine.sent).toEqual([{ kind: 'command', type: 'capabilities', features: ['questions'] }])
    const { controller } = await attach('mobile')
    expect(controller.getState().features).toContain('sessions')
    // Clients do not set the engine's capabilities: the host speaks for all of them.
    expect(engine.sent).toHaveLength(1)
  })

  test('an answer reaches the engine, and every client sees the question answered', async () => {
    await startHost({ features: ['capabilities'] })
    const desktop = await attach('desktop')
    const phone = await attach('mobile')
    engine.emit({ type: 'question', id: 4, question: 'Which crate?', options: ['api', 'tools'] })
    await until('the question', () => desktop.controller.getState().items.some((item) => item.kind === 'question'))
    phone.controller.send('tools')
    await until('the answer', () => engine.sent.some((message) => message.kind === 'command' && message.type === 'answer'))
    expect(engine.sent.at(-1)).toEqual({ kind: 'command', type: 'answer', id: 4, text: 'tools' })
    await until('answered everywhere', () =>
      [desktop, phone].every((client) => client.controller.getState().items.some((item) => item.kind === 'question' && item.answered)),
    )
  })

  test('only clients on this machine may trust the workspace or restart the engine', async () => {
    await startHost({ remoteOnly: true, features: ['trust_workspace'], trusted: false })
    const { transport, controller } = await attach('mobile')
    const errors: string[] = []
    const socket = new WebSocket(host!.url)
    socket.onopen = () => {
      socket.send(JSON.stringify({ t: 'hello', protocol: 1, token: TOKEN, client: { name: 'raw', kind: 'cli' } }))
      socket.send(JSON.stringify({ t: 'send', message: { kind: 'command', type: 'trust_workspace' } }))
    }
    socket.onmessage = (event) => {
      const frame = JSON.parse(String(event.data)) as HostFrame
      if (frame.t === 'error') errors.push(frame.message)
    }
    await until('the refusal', () => errors.length > 0)
    socket.close()
    expect(errors[0]).toContain('Only the machine running Neuron can trust')
    expect(engine.sent.filter((message) => message.kind === 'command' && message.type === 'trust_workspace')).toEqual([])
    const failure = await transport.rpc('engine.restart', {}).then(() => 'restarted', (error: Error) => error.message)
    expect(failure).toBe('Only the machine running Neuron can restart its engine.')
    expect(controller.getState().connection).toBe('ready')
  })

  test('a restart from this machine starts a new engine on the same chat', async () => {
    await startHost({ features: ['trust_workspace'], trusted: false })
    const { transport, controller } = await attach('desktop')
    controller.trustWorkspace()
    await until('the trust command', () => engine.sent.some((message) => message.kind === 'command' && message.type === 'trust_workspace'))
    engine.emit({ type: 'workspace_trusted', cwd: '/ws', restart_required: true })
    await until('the restart prompt', () => controller.getState().restartRequired)
    await transport.rpc('engine.restart', { resume: 's' })
    expect(restarted.map((entry) => entry.resume)).toEqual(['s'])
    await until('the new engine', () => !controller.getState().restartRequired && controller.getState().connection === 'ready')
    expect(controller.getState().status?.workspace_trusted).toBe(true)
    // Commands go to the new engine now.
    controller.send('after restart')
    await until('the prompt', () => restarted[0].engine.sent.some((message) => message.kind === 'command' && message.type === 'submit'))
  })

  test('fs.search finds workspace files by name', async () => {
    await startHost()
    fs.mkdirSync(path.join(root, 'node_modules', 'dep'), { recursive: true })
    fs.writeFileSync(path.join(root, 'node_modules', 'dep', 'main.rs'), '')
    const { transport } = await attach('desktop')
    expect(await transport.rpc<string[]>('fs.search', { query: 'main' })).toEqual(['src/main.rs'])
    expect(await transport.rpc<string[]>('fs.search', { query: 'READ' })).toEqual(['README.md'])
  })

  test('git.status and git.diff show the working tree changes', async () => {
    await startHost()
    const git = (...args: string[]) =>
      Bun.spawnSync(['git', '-c', 'user.email=t@example.com', '-c', 'user.name=t', ...args], { cwd: root, stdout: 'pipe', stderr: 'pipe' })
    git('init', '-q', '-b', 'main')
    git('add', '.')
    git('commit', '-q', '-m', 'init')
    fs.writeFileSync(path.join(root, 'src', 'main.rs'), 'fn main() { println!("hi"); }\n')
    fs.writeFileSync(path.join(root, 'new.txt'), 'new\n')
    const { transport } = await attach('desktop')
    const status = await transport.rpc<{ branch: string; changes: Array<{ path: string; index: string; worktree: string }> }>('git.status')
    expect(status.branch).toBe('main')
    expect(status.changes).toEqual([
      { path: 'src/main.rs', index: ' ', worktree: 'M' },
      { path: 'new.txt', index: '?', worktree: '?' },
    ])
    const diff = await transport.rpc<string>('git.diff', { path: 'src/main.rs' })
    expect(diff).toContain('+fn main() { println!("hi"); }')
    const outside = await transport.rpc('git.diff', { path: '../x' }).then(() => 'ran', (error: Error) => error.message)
    expect(outside).toBe('That path is outside the workspace.')
  })

  test('git.status refuses an untrusted repository whose config runs programs', async () => {
    await startHost({ trusted: false })
    Bun.spawnSync(['git', 'init', '-q'], { cwd: root })
    Bun.spawnSync(['git', 'config', 'core.fsmonitor', 'echo pwned'], { cwd: root })
    const { transport } = await attach('desktop')
    const failure = await transport.rpc('git.status').then(() => 'ran', (error: Error) => error.message)
    expect(failure).toContain('trust the workspace')
  })
})
