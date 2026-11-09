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

async function startHost(options: { remoteOnly?: boolean; localOnly?: boolean; features?: string[]; trusted?: boolean } = {}) {
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
    ...(options.localOnly && { isLocal: () => true }),
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

const until = async (label: string, predicate: () => boolean, timeoutMs = 5_000) => {
  const deadline = Date.now() + timeoutMs
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`)
    await Bun.sleep(10)
  }
}

/**
 * Opens a raw socket, says hello with the token, sends each message as a `send` frame and waits until the
 * host has forwarded or refused every one. Returns the messages of the error frames that came back, in order.
 */
async function sendAsPeer(messages: unknown[]): Promise<string[]> {
  const errors: string[] = []
  const socket = new WebSocket(host!.url)
  socket.onopen = () => {
    socket.send(JSON.stringify({ t: 'hello', protocol: 1, token: TOKEN, client: { name: 'raw', kind: 'cli' } }))
    for (const message of messages) socket.send(JSON.stringify({ t: 'send', message }))
  }
  socket.onmessage = (event) => {
    const frame = JSON.parse(String(event.data)) as HostFrame
    if (frame.t === 'error') errors.push(frame.message)
  }
  await until('every message handled', () => errors.length + engine.sent.length >= messages.length)
  socket.close()
  return errors
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
    // A slash command goes out as one. `/status` only reads, so the host passes it to any peer.
    mobile.controller.send('/status')
    await until('the slash command', () => engine.sent.length === 2)
    expect(engine.sent[1]).toEqual({ kind: 'command', type: 'slash', command: 'status', args: '' })
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
    // $((1+1)) is 2 in both PowerShell (Windows) and POSIX shells.
    transport.terminalInput(id, 'echo neuron-term-$((1+1))\r')
    // A cold Windows PowerShell can take several seconds to start on CI runners.
    await until('the echo', () => output.includes('neuron-term-2'), 15_000)
    transport.terminalInput(id, 'exit\r')
    await until('the shell to exit', () => exited !== undefined)
  }, 30_000)

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

  test('a folder no repository tracks is no git repository, even inside one', async () => {
    await startHost()
    // On machines where the temp folder sits in a repository (a home directory kept in
    // git), the workspace is an untracked folder of it; elsewhere it is in none.
    const { transport } = await attach('desktop')
    expect(await transport.rpc('git.status')).toBeNull()
    // A repository inside the workspace's parent: the workspace is a tracked subfolder.
    const parent = fs.mkdtempSync(path.join(os.tmpdir(), 'neuron-host-parent-'))
    const nested = path.join(parent, 'pkg')
    fs.mkdirSync(nested)
    fs.writeFileSync(path.join(nested, 'a.txt'), 'a\n')
    fs.writeFileSync(path.join(parent, 'outside.txt'), 'x\n')
    const git = (...args: string[]) =>
      Bun.spawnSync(['git', '-c', 'user.email=t@example.com', '-c', 'user.name=t', ...args], { cwd: parent, stdout: 'pipe', stderr: 'pipe' })
    git('init', '-q', '-b', 'main')
    git('add', '.')
    git('commit', '-q', '-m', 'init')
    fs.writeFileSync(path.join(nested, 'a.txt'), 'changed\n')
    fs.writeFileSync(path.join(parent, 'outside.txt'), 'changed too\n')
    const { gitStatus } = await import('../src/node/workspace')
    // Only the workspace's own changes, with paths relative to it.
    expect(await gitStatus(nested, true)).toEqual({ branch: 'main', changes: [{ path: 'a.txt', index: ' ', worktree: 'M' }] })
    fs.rmSync(parent, { recursive: true, force: true })
  })
})

describe('NeuronHost policy for peers', () => {
  async function sendRaw(messages: unknown[]) {
    const errors: string[] = []
    const socket = new WebSocket(host!.url)
    socket.onopen = () => {
      socket.send(JSON.stringify({ t: 'hello', protocol: 1, token: TOKEN, client: { name: 'raw', kind: 'cli' } }))
      for (const message of messages) socket.send(JSON.stringify({ t: 'send', message }))
    }
    socket.onmessage = (event) => {
      const frame = JSON.parse(String(event.data)) as HostFrame
      if (frame.t === 'error') errors.push(frame.message)
    }
    await until('the refusals', () => errors.length >= messages.length)
    socket.close()
    return errors
  }

  test('peers cannot commit, rewind for real or switch MCP servers', async () => {
    await startHost()
    const errors = await sendRaw([
      { kind: 'command', type: 'commit', message: 'x' },
      { kind: 'command', type: 'rewind', checkpoint: 1, mode: 'code_only' },
      { kind: 'command', type: 'rewind', checkpoint: 1, mode: 'code_only', dry_run: false },
      { kind: 'command', type: 'set_mcp_server_enabled', name: 'fs', enabled: false },
    ])
    expect(errors).toHaveLength(4)
    expect(engine.sent).toEqual([])
  })

  test('peers may preview a rewind', async () => {
    await startHost()
    const peer = await attach('mobile')
    peer.transport.send({ kind: 'command', type: 'rewind', checkpoint: 1, mode: 'code_only', dry_run: true })
    await until('the preview', () => engine.sent.length === 1)
    expect(engine.sent[0]).toEqual({ kind: 'command', type: 'rewind', checkpoint: 1, mode: 'code_only', dry_run: true })
  })
})

describe('NeuronHost slash policy for remote peers', () => {
  /** Sends messages as a non-local peer, then waits until each was either forwarded or refused. */
  async function remote(messages: unknown[]) {
    const errors: string[] = []
    const socket = new WebSocket(host!.url)
    socket.onopen = () => {
      socket.send(JSON.stringify({ t: 'hello', protocol: 1, token: TOKEN, client: { name: 'raw', kind: 'cli' } }))
      for (const message of messages) socket.send(JSON.stringify({ t: 'send', message }))
    }
    socket.onmessage = (event) => {
      const frame = JSON.parse(String(event.data)) as HostFrame
      if (frame.t === 'error') errors.push(frame.message)
    }
    await until('every message handled', () => errors.length + engine.sent.length >= messages.length)
    socket.close()
    return errors
  }
  const slash = (command: string, args = '') => ({ kind: 'command', type: 'slash', command, args })

  test('read-only slash commands pass', async () => {
    await startHost({ remoteOnly: true })
    const names = ['help', 'status', 'cost', 'context', 'usage', 'doctor', 'changes', 'diff', 'stats', 'memory', 'model']
    const errors = await remote(names.map((name) => slash(name)))
    expect(errors).toEqual([])
    expect(engine.sent).toHaveLength(names.length)
  })

  test('every other slash command is refused, whatever the case or spacing', async () => {
    await startHost({ remoteOnly: true })
    const names = ['commit', 'Commit', ' commit', 'commit ', 'COMMIT\n', '/commit', 'rewind', 'permissions', 'logout', 'login', 'pr', 'issue', 'init', 'clear', 'plan', 'status commit', '']
    const errors = await remote(names.map((name) => slash(name, 'danger-full-access')))
    expect(errors).toHaveLength(names.length)
    expect(errors[0]).toContain('Only the Neuron desktop app')
    expect(engine.sent).toEqual([])
  })

  test('so are the alpha.5 slash commands that do what the refused commands do', async () => {
    await startHost({ remoteOnly: true })
    // /effort and /reasoning set the thinking level, /branch switches or creates a branch, /permissions
    // sets the mode, /mcp adds, approves or removes servers, /hooks changes hooks, /plan toggles plan mode.
    const names = ['effort', 'reasoning', 'branch', 'permissions', 'mcp', 'hooks', 'plan', 'Effort', ' MCP ']
    const errors = await remote(names.map((name) => slash(name, 'danger-full-access')))
    expect(errors).toHaveLength(names.length)
    expect(errors[0]).toContain('Only the Neuron desktop app')
    expect(engine.sent).toEqual([])
  })

  test('remote resume names a session id or latest, never a path', async () => {
    await startHost({ remoteOnly: true })
    const bad = [
      { kind: 'command', type: 'resume', session: '../../etc/passwd' },
      { kind: 'command', type: 'resume', session: 'C:\\Users\\x\\s.json' },
      { kind: 'command', type: 'resume', session: '/tmp/s' },
      { kind: 'command', type: 'resume', session: '-x' },
      { kind: 'command', type: 'resume', session: '.hidden' },
      { kind: 'command', type: 'resume', session: 'a'.repeat(129) },
      slash('resume', '../x'),
      slash('Resume', '/abs/path'),
      slash('resume', ''),
    ]
    expect(await remote(bad)).toHaveLength(bad.length)
    expect(engine.sent).toEqual([])
  })

  test('remote resume with a valid id or latest is forwarded', async () => {
    await startHost({ remoteOnly: true })
    const errors = await remote([
      { kind: 'command', type: 'resume', session: 'session-2026.11_07' },
      { kind: 'command', type: 'resume', session: 'latest' },
      slash('resume', 'abc123'),
    ])
    expect(errors).toEqual([])
    expect(engine.sent).toHaveLength(3)
  })
})

/**
 * A phone behind `neuron engine --share` reaches the host from 127.0.0.1, so the host counts it as local.
 * "Local" says where a socket came from, not who is typing: the slash allow-list and the resume-by-id rule
 * hold for every peer, and `local` only opens terminals, engine restarts and `trust_workspace`.
 */
describe('NeuronHost walls for a peer on this machine', () => {
  const slash = (command: string, args = '') => ({ kind: 'command', type: 'slash', command, args })
  const resume = (session: string) => ({ kind: 'command', type: 'resume', session })
  const OWNER_ONLY = 'Only the Neuron desktop app can change your files, history or settings.'
  const BY_ID = 'A remote client can only resume a session by its id.'

  test('the slash twins of the refused commands, and resume by path, never reach the engine', async () => {
    await startHost({ localOnly: true })
    // The frames sent over a loopback socket that used to reach the engine.
    const twins = [
      slash('permissions', 'danger-full-access'),
      slash('effort', 'max'),
      slash('branch', 'x'),
      slash('commit', 'x'),
      slash('rewind', '1 code'),
      slash('exit'),
      slash('logout'),
      slash('clear', '--confirm'),
      slash('pr', 'create'),
      slash('issue', 'create'),
      // It rewrites the history, and a local peer could send it before.
      slash('compact'),
    ]
    const errors = await sendAsPeer([...twins, resume('/etc/passwd')])
    expect(engine.sent).toEqual([])
    expect(errors).toEqual([...twins.map(() => OWNER_ONLY), BY_ID])
  })

  test('the read-only slash commands and resume by id still pass', async () => {
    await startHost({ localOnly: true })
    const names = ['help', 'status', 'cost', 'context', 'usage', 'doctor', 'changes', 'diff', 'stats', 'memory', 'model']
    const messages = [...names.map((name) => slash(name)), resume('session-2026.11_07'), resume('latest'), slash('resume', 'abc123')]
    expect(await sendAsPeer(messages)).toEqual([])
    expect(engine.sent).toHaveLength(messages.length)
  })

  test('resume names a session id or latest, never a path, whoever sends it', async () => {
    await startHost({ localOnly: true })
    const paths = [
      resume('/tmp/s'),
      resume('C:\\Users\\x\\s.json'),
      resume('../../etc/passwd'),
      resume('..\\x'),
      resume('-x'),
      resume('.hidden'),
      resume('a'.repeat(129)),
      slash('resume', '../x'),
      slash('Resume', '/abs/path'),
      slash('resume', ''),
    ]
    const errors = await sendAsPeer(paths)
    expect(engine.sent).toEqual([])
    expect(errors).toHaveLength(paths.length)
  })
})

describe('NeuronHost policy for the alpha.5 commands', () => {
  /** Sends messages as a peer on another machine, then waits until each was either forwarded or refused. */
  async function fromAnotherMachine(messages: unknown[]) {
    const errors: string[] = []
    const socket = new WebSocket(host!.url)
    socket.onopen = () => {
      socket.send(JSON.stringify({ t: 'hello', protocol: 1, token: TOKEN, client: { name: 'raw', kind: 'cli' } }))
      for (const message of messages) socket.send(JSON.stringify({ t: 'send', message }))
    }
    socket.onmessage = (event) => {
      const frame = JSON.parse(String(event.data)) as HostFrame
      if (frame.t === 'error') errors.push(frame.message)
    }
    await until('every message handled', () => errors.length + engine.sent.length >= messages.length)
    socket.close()
    return errors
  }

  // They only read, but every call costs the engine real work and no phone screen uses the answer.
  // The gate drops them, so the host answers "not a frame" and the engine hears nothing.
  const reads = [
    { kind: 'command', type: 'list_branches' },
    { kind: 'command', type: 'list_connector_catalog', query: 'git' },
  ]
  for (const [where, options] of [
    ['another machine', { remoteOnly: true }],
    ['this machine', { localOnly: true }],
  ] as const) {
    test(`a peer on ${where} cannot list the branches or the connector catalog`, async () => {
      await startHost(options)
      const errors = await sendAsPeer(reads)
      expect(engine.sent).toEqual([])
      expect(errors).toEqual(reads.map(() => 'Not a Neuron remote frame.'))
    })
  }

  /** Commands that change something: a connector to run or remove, the permission mode, the branch, how turns run. */
  const changes = [
    { kind: 'command', type: 'add_mcp_server', id: 'github', name: null, scope: 'user', secrets: { EXAMPLE_TOKEN: 'not-a-real-token' } },
    { kind: 'command', type: 'approve_mcp_server', name: 'postgres-tools', fingerprint: '4f1c0de5' },
    { kind: 'command', type: 'remove_mcp_server', name: 'docs', scope: null },
    { kind: 'command', type: 'set_permission_mode', mode: 'danger-full-access' },
    { kind: 'command', type: 'switch_branch', name: 'main', create: false },
    { kind: 'command', type: 'set_reasoning_effort', effort: 'high' },
  ] as const satisfies readonly ClientMessage[]

  test('a peer on another machine cannot add, approve or remove a server, change the mode, switch branch or set the effort', async () => {
    await startHost({ remoteOnly: true })
    const errors = await fromAnotherMachine([...changes, { kind: 'permission_reply', id: 7, decision: { kind: 'allow_for_project' } }])
    expect(errors).toHaveLength(changes.length + 1)
    expect(engine.sent).toEqual([])
  })

  /** Hands `message` to the host's forwarding step as if the gate before it had let it through, as a bug in the gate would. */
  function forwardPastTheGate(message: ClientMessage, local: boolean): HostFrame[] {
    const frames: HostFrame[] = []
    const peer = {
      data: { authed: true, name: 'raw', kind: 'cli', timer: null, local, terminals: new Map() },
      send: (text: string) => frames.push(JSON.parse(text) as HostFrame),
    }
    ;(host as unknown as { forward(socket: unknown, message: ClientMessage): void }).forward(peer, message)
    return frames
  }

  test('the host refuses those commands itself, to a peer on this machine as well', async () => {
    await startHost()
    for (const local of [false, true]) {
      for (const message of changes) {
        const frames = forwardPastTheGate(message, local)
        expect({ type: message.type, local, frames }).toEqual({
          type: message.type,
          local,
          frames: [{ t: 'error', message: 'Only the Neuron desktop app can change your files, history or settings.', fatal: false }],
        })
      }
    }
    expect(engine.sent).toEqual([])
  })

  test('the host will not save project rules for a peer, and the request stays open for a decision it may take', async () => {
    await startHost()
    const input = '{"command":"cargo test"}'
    engine.emit({ type: 'tool_call_started', id: 't1', name: 'bash', input, cut_off: false })
    engine.emit({
      type: 'permission_requested',
      id: 7,
      request: { tool: 'bash', current_mode: 'workspace-write', required_mode: 'danger-full-access', reason: null, input, session_prefixes: [], project_rules: ['Bash(cargo test:*)'] },
    })
    for (const local of [false, true]) {
      const frames = forwardPastTheGate({ kind: 'permission_reply', id: 7, decision: { kind: 'allow_for_project' } }, local)
      expect(frames).toEqual([{ t: 'error', message: 'Only the Neuron desktop app can change your files, history or settings.', fatal: false }])
    }
    expect(engine.sent).toEqual([])
    expect(host!.state.permissions.map((permission) => permission.id)).toEqual([7])
    // A decision a peer may take still goes through.
    expect(forwardPastTheGate({ kind: 'permission_reply', id: 7, decision: { kind: 'allow_once' } }, false)).toEqual([])
    expect(engine.sent).toEqual([{ kind: 'permission_reply', id: 7, decision: { kind: 'allow_once' } }])
  })
})

describe('NeuronHost origin check', () => {
  const allowed = (origin: string | null, hostHeader: string | null) =>
    (host as unknown as { originAllowed(o: string | null, h: string | null): boolean }).originAllowed(origin, hostHeader)

  test('a same-host origin needs a loopback Host header', async () => {
    await startHost()
    expect(allowed('http://192.168.1.5:3000', '192.168.1.5:3000')).toBe(false)
    expect(allowed('https://evil.example', 'evil.example')).toBe(false)
    expect(allowed('http://evil.example:80', 'localhost:7777')).toBe(false)
  })

  test('local pages and native clients still connect', async () => {
    await startHost()
    expect(allowed(null, 'anything')).toBe(true)
    expect(allowed('http://localhost:5173', 'localhost:7777')).toBe(true)
    expect(allowed('http://127.0.0.1:5173', '127.0.0.1')).toBe(true)
  })
})

