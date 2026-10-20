import { describe, expect, test } from 'bun:test'

import {
  EngineExitedError,
  runExchange,
  runTurn,
} from '../neuron-engine-client'
import {
  EngineLaunchError,
  StdioEngineClient,
  resolveNeuronBinary,
  type SpawnEngine,
} from '../stdio-engine-client'

import type { ReadyEvent } from '../protocol'

/** A child process the test drives: lines it "prints", and what it was sent. */
function fakeEngine() {
  const encoder = new TextEncoder()
  let out!: ReadableStreamDefaultController<Uint8Array>
  let err!: ReadableStreamDefaultController<Uint8Array>
  let exit!: (code: number) => void
  const written: string[] = []
  const argv: string[][] = []
  let stdinEnded = false
  const exited = new Promise<number>((resolve) => (exit = resolve))
  const child = {
    stdin: {
      write: (data: string) => written.push(data),
      flush: () => undefined,
      end: () => {
        stdinEnded = true
      },
    },
    stdout: new ReadableStream<Uint8Array>({ start: (c) => (out = c) }),
    stderr: new ReadableStream<Uint8Array>({ start: (c) => (err = c) }),
    exited,
    signalCode: null,
    kill: () => exit(137),
  }
  const spawn: SpawnEngine = (args) => {
    argv.push(args)
    return child
  }
  return {
    spawn,
    argv,
    written: () => written.map((line) => JSON.parse(line)),
    stdinEnded: () => stdinEnded,
    emit: (...events: object[]) =>
      out.enqueue(encoder.encode(events.map((e) => JSON.stringify(e) + '\n').join(''))),
    emitRaw: (text: string) => out.enqueue(encoder.encode(text)),
    log: (text: string) => err.enqueue(encoder.encode(text)),
    exit: (code: number) => {
      out.close()
      err.close()
      exit(code)
    },
  }
}

const READY = { type: 'ready', protocol: 1, version: '6.3.0' }
const tick = () => new Promise((resolve) => setTimeout(resolve, 5))

describe('StdioEngineClient', () => {
  test('starts the engine with the launch flags and resolves at ready', async () => {
    const engine = fakeEngine()
    const client = new StdioEngineClient({
      spawn: engine.spawn,
      resolveBinary: () => '/opt/neuron',
    })
    const started = client.start({ model: 'opus', cwd: '/work' })
    expect(client.connection).toBe('starting')
    engine.emit(READY)
    expect(await started).toEqual(READY as ReadyEvent)
    expect(client.connection).toBe('ready')
    expect(engine.argv[0]).toEqual(['/opt/neuron', 'engine', '--stdio', '--model', 'opus'])
  })

  test('commands go out as one JSON line each, in the protocol shape', async () => {
    const engine = fakeEngine()
    const client = new StdioEngineClient({ spawn: engine.spawn, resolveBinary: () => 'neuron' })
    const started = client.start()
    engine.emit(READY)
    await started
    client.submit('fix it', [{ kind: 'file', path: 'a.rs' }])
    client.interrupt()
    client.replyToPermission({ id: 4, decision: { kind: 'deny', note: 'no' } })
    expect(engine.written()).toEqual([
      { kind: 'command', type: 'submit', text: 'fix it', attachments: [{ kind: 'file', path: 'a.rs' }] },
      { kind: 'command', type: 'interrupt' },
      { kind: 'permission_reply', id: 4, decision: { kind: 'deny', note: 'no' } },
    ])
  })

  test('a launch error before ready rejects with the engine message', async () => {
    const engine = fakeEngine()
    const client = new StdioEngineClient({ spawn: engine.spawn, resolveBinary: () => 'neuron' })
    const started = client.start()
    engine.emit({ type: 'error', message: 'Sign in with `neuron auth login`' })
    engine.exit(1)
    await expect(started).rejects.toBeInstanceOf(EngineLaunchError)
    await expect(started).rejects.toThrow('Sign in with `neuron auth login`')
    expect(client.connection).toBe('exited')
    expect(client.lastError).toBe('Sign in with `neuron auth login`')
    expect(() => client.submit('hi')).toThrow('Sign in with `neuron auth login`')
  })

  test('an exit before ready with only a stderr error shows that error', async () => {
    const engine = fakeEngine()
    const client = new StdioEngineClient({ spawn: engine.spawn, resolveBinary: () => 'neuron' })
    const started = client.start()
    // The engine's broad-directory check prints this and exits, no event.
    engine.log(
      'error: neuron is running from a very broad directory (C:\\Users\\me). Pass --allow-broad-cwd to proceed anyway.\n\nRun `neuron --help` for usage.\n',
    )
    engine.exit(1)
    await expect(started).rejects.toThrow('neuron is running from a very broad directory')
    expect(client.lastError).toStartWith('neuron is running from a very broad directory')
  })

  test('an exit before ready with nothing said reports the exit code', async () => {
    const engine = fakeEngine()
    const client = new StdioEngineClient({ spawn: engine.spawn, resolveBinary: () => 'neuron' })
    const started = client.start()
    engine.log('panicked at src/main.rs\n')
    engine.exit(101)
    await expect(started).rejects.toThrow('The Neuron engine stopped (exit code 101).')
  })

  test('events split across chunks reach subscribers in order; junk is reported', async () => {
    const engine = fakeEngine()
    const unknown: string[] = []
    const client = new StdioEngineClient({
      spawn: engine.spawn,
      resolveBinary: () => 'neuron',
      onUnknownLine: (_line, reason) => unknown.push(reason),
    })
    const seen: string[] = []
    client.subscribe((event) => seen.push(event.type))
    const started = client.start()
    engine.emitRaw('{"type":"ready","protocol":1,"version":"x"}\n{"type":"text_')
    await started
    engine.emitRaw('delta","text":"hi"}\r\nnot json\n{"type":"brand_new"}\n')
    await tick()
    expect(seen).toEqual(['ready', 'text_delta'])
    expect(unknown).toHaveLength(2)
    expect(unknown[1]).toBe('unknown event brand_new')
  })

  test('stderr lines go to the log callback, never to subscribers', async () => {
    const engine = fakeEngine()
    const logged: string[] = []
    const client = new StdioEngineClient({
      spawn: engine.spawn,
      resolveBinary: () => 'neuron',
      onStderr: (line) => logged.push(line),
    })
    const started = client.start()
    engine.log('provider: zero-x\nwarming up\n')
    engine.emit(READY)
    await started
    await tick()
    expect(logged).toEqual(['provider: zero-x', 'warming up'])
  })

  test('shutdown sends shutdown, closes stdin and waits for the exit', async () => {
    const engine = fakeEngine()
    const client = new StdioEngineClient({ spawn: engine.spawn, resolveBinary: () => 'neuron' })
    const exits: number[] = []
    client.onExit((exit) => exits.push(exit.code ?? -1))
    const started = client.start()
    engine.emit(READY)
    await started
    const stopping = client.shutdown()
    expect(engine.written().at(-1)).toEqual({ kind: 'command', type: 'shutdown' })
    expect(engine.stdinEnded()).toBe(true)
    engine.exit(0)
    expect(await stopping).toEqual({ code: 0, signal: null })
    expect(exits).toEqual([0])
    expect(client.connection).toBe('exited')
  })

  test('a turn that loses its engine rejects with EngineExitedError', async () => {
    const engine = fakeEngine()
    const client = new StdioEngineClient({ spawn: engine.spawn, resolveBinary: () => 'neuron' })
    const started = client.start()
    engine.emit(READY)
    await started
    const turn = runTurn(client, { text: 'go' }, () => {})
    await tick()
    engine.exit(101)
    await expect(turn).rejects.toBeInstanceOf(EngineExitedError)
  })
})

describe('runExchange', () => {
  test('exchanges run one at a time, each ended by its own turn_finished', async () => {
    const engine = fakeEngine()
    const client = new StdioEngineClient({ spawn: engine.spawn, resolveBinary: () => 'neuron' })
    const started = client.start()
    engine.emit(READY)
    await started

    const slash = runExchange(client, () => client.slash('compact'), () => {})
    const turnEvents: string[] = []
    const turn = runTurn(client, { text: 'next' }, (event) => turnEvents.push(event.type))
    await tick()
    // The prompt waits until the slash command's exchange is over.
    expect(engine.written().map((m) => m.type)).toEqual(['slash'])

    engine.emit({ type: 'notice', message: 'Compacted.' }, { type: 'turn_finished' })
    expect(await slash).toEqual({ error: null })
    await tick()
    expect(engine.written().map((m) => m.type)).toEqual(['slash', 'submit'])
    expect(turnEvents).toEqual([])

    engine.emit(
      { type: 'text_delta', text: 'done' },
      { type: 'error', message: 'rate limited' },
      { type: 'turn_finished' },
    )
    expect(await turn).toEqual({ error: 'rate limited' })
    expect(turnEvents).toEqual(['text_delta', 'error', 'turn_finished'])
  })
})

describe('resolveNeuronBinary', () => {
  test('NEURON_BIN wins, then PATH', () => {
    expect(resolveNeuronBinary({ NEURON_BIN: '/x/neuron' }, '/nowhere/bun', () => '/usr/bin/neuron')).toBe('/x/neuron')
    expect(resolveNeuronBinary({}, '/nowhere/bun', () => '/usr/bin/neuron')).toBe('/usr/bin/neuron')
    expect(resolveNeuronBinary({}, '/nowhere/bun', () => null)).toBeNull()
  })
})
