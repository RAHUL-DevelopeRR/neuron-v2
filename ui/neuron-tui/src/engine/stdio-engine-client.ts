/**
 * The engine client that talks to `neuron engine --stdio` in a child process:
 * {@link ClientMessage} lines on its stdin, {@link UiEvent} lines on its
 * stdout, logs on its stderr (kept in the UI's log file, never shown).
 */

import { existsSync } from 'fs'
import path from 'path'

import {
  EngineNotConnectedError,
  engineArgs,
  type EngineConnection,
  type EngineExit,
  type EngineLaunchOptions,
  type NeuronEngineClient,
} from './neuron-engine-client'
import {
  JsonLineSplitter,
  PROTOCOL_VERSION,
  commandMessage,
  encodeClientMessage,
  parseUiEventLine,
  permissionReplyMessage,
  type Attachment,
  type ClientMessage,
  type EngineCommand,
  type PermissionReply,
  type ReadyEvent,
  type UiEvent,
} from './protocol'

const READY_TIMEOUT_MS = 60_000
const SHUTDOWN_GRACE_MS = 5_000
// How long an exit waits for the last stderr lines. A process the engine
// started (an MCP server) can hold the pipe open after the engine is gone.
const STDERR_DRAIN_MS = 250

/**
 * The message of an `error: ...` line on stderr. Some launch checks (a broad
 * working directory, bad flags) print one there and exit without an `error`
 * event.
 */
export function stderrErrorMessage(line: string): string | null {
  const match = /^error:\s*(.+)$/i.exec(line.trim())
  return match ? match[1].trim() : null
}

/** The engine could not start: its own `error` event, or why it never got ready. */
export class EngineLaunchError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'EngineLaunchError'
  }
}

const exeName = (name: string) =>
  process.platform === 'win32' ? `${name}.exe` : name

/**
 * Where the `neuron` binary is: `NEURON_BIN`, else beside this UI's own
 * executable (the packaged layout), else on PATH.
 */
export function resolveNeuronBinary(
  env: NodeJS.ProcessEnv = process.env,
  execPath: string = process.execPath,
  which: (name: string) => string | null = (name) => Bun.which(name),
): string | null {
  const fromEnv = env.NEURON_BIN?.trim()
  if (fromEnv) return fromEnv
  // Beside the compiled UI binary. When the UI runs from source, execPath is
  // bun itself, whose folder has no neuron in it.
  const sibling = path.join(path.dirname(execPath), exeName('neuron'))
  if (existsSync(sibling)) return sibling
  return which('neuron')
}

type SpawnedEngine = {
  stdin: { write(data: string): unknown; flush(): unknown; end(): unknown }
  stdout: ReadableStream<Uint8Array>
  stderr: ReadableStream<Uint8Array>
  exited: Promise<number>
  signalCode: string | null
  kill(signal?: number | NodeJS.Signals): void
}

async function* readChunks(
  stream: ReadableStream<Uint8Array>,
): AsyncGenerator<Uint8Array> {
  const reader = stream.getReader()
  try {
    while (true) {
      const { value, done } = await reader.read()
      if (done) return
      if (value) yield value
    }
  } finally {
    reader.releaseLock()
  }
}

export type SpawnEngine = (
  argv: string[],
  options: { cwd: string; env: Record<string, string | undefined> },
) => SpawnedEngine

const defaultSpawn: SpawnEngine = (argv, options) =>
  Bun.spawn(argv, {
    cwd: options.cwd,
    env: options.env,
    stdin: 'pipe',
    stdout: 'pipe',
    stderr: 'pipe',
  }) as unknown as SpawnedEngine

export interface StdioEngineClientOptions {
  spawn?: SpawnEngine
  resolveBinary?: () => string | null
  /** Each stderr line of the engine (its logs). */
  onStderr?: (line: string) => void
  /** A stdout line that is not a known event (unknown type or not JSON). */
  onUnknownLine?: (line: string, reason: string) => void
  readyTimeoutMs?: number
}

export class StdioEngineClient implements NeuronEngineClient {
  connection: EngineConnection = 'not_started'
  /** Why the engine is not running, once it failed or stopped. */
  lastError: string | null = null
  ready: ReadyEvent | null = null

  private child: SpawnedEngine | null = null
  private listeners = new Set<(event: UiEvent) => void>()
  private exitListeners = new Set<(exit: EngineExit) => void>()
  private startPromise: Promise<ReadyEvent> | null = null
  private exitPromise: Promise<EngineExit> | null = null
  private stderrError: string | null = null

  constructor(private readonly options: StdioEngineClientOptions = {}) {}

  start(launch: EngineLaunchOptions = {}): Promise<ReadyEvent> {
    if (this.startPromise) return this.startPromise
    this.startPromise = this.launch(launch)
    return this.startPromise
  }

  private launch(launch: EngineLaunchOptions): Promise<ReadyEvent> {
    const binary =
      launch.binary ?? (this.options.resolveBinary ?? resolveNeuronBinary)()
    if (!binary) {
      return this.fail(
        'Could not find the `neuron` binary. Install Neuron, or set NEURON_BIN to its path.',
      )
    }
    let child: SpawnedEngine
    try {
      child = (this.options.spawn ?? defaultSpawn)(
        [binary, ...engineArgs(launch)],
        { cwd: launch.cwd ?? process.cwd(), env: { ...process.env } },
      )
    } catch (error) {
      return this.fail(
        `Could not start ${binary}: ${error instanceof Error ? error.message : String(error)}`,
      )
    }
    this.child = child
    this.connection = 'starting'

    return new Promise<ReadyEvent>((resolve, reject) => {
      let settled = false
      const settle = (outcome: () => void) => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        outcome()
      }
      const timer = setTimeout(() => {
        settle(() => {
          this.lastError = 'The Neuron engine did not get ready in time.'
          reject(new EngineLaunchError(this.lastError))
        })
        child.kill()
      }, this.options.readyTimeoutMs ?? READY_TIMEOUT_MS)

      const onEvent = (event: UiEvent) => {
        if (event.type === 'ready') {
          this.ready = event
          this.connection = 'ready'
          if (event.protocol > PROTOCOL_VERSION) {
            this.options.onUnknownLine?.(
              JSON.stringify(event),
              `engine speaks protocol ${event.protocol}; this UI knows ${PROTOCOL_VERSION}`,
            )
          }
          settle(() => resolve(event))
        } else if (event.type === 'error' && this.connection === 'starting') {
          // A failed launch sends one error and exits.
          this.lastError = event.message
          settle(() => reject(new EngineLaunchError(event.message)))
        }
        for (const listener of [...this.listeners]) listener(event)
      }

      const stdoutDone = this.pumpStdout(child.stdout, onEvent)
      const stderrDone = this.pumpStderr(child.stderr)

      // Its last lines (a launch error) are read before the exit is handled.
      this.exitPromise = Promise.all([child.exited, stdoutDone]).then(async ([code]) => {
        await Promise.race([
          stderrDone,
          new Promise((resolve) => setTimeout(resolve, STDERR_DRAIN_MS)),
        ])
        const exit: EngineExit = { code, signal: child.signalCode }
        const wasReady = this.connection === 'ready'
        this.connection = 'exited'
        this.child = null
        if (!this.lastError && (code !== 0 || !wasReady)) {
          this.lastError =
            this.stderrError ?? `The Neuron engine stopped (exit code ${code}).`
        }
        settle(() =>
          reject(
            new EngineLaunchError(
              this.lastError ?? `The Neuron engine stopped (exit code ${code}).`,
            ),
          ),
        )
        for (const listener of [...this.exitListeners]) listener(exit)
        return exit
      })
    })
  }

  private fail(message: string): Promise<ReadyEvent> {
    this.connection = 'exited'
    this.lastError = message
    return Promise.reject(new EngineLaunchError(message))
  }

  private async pumpStdout(
    stream: ReadableStream<Uint8Array>,
    onEvent: (event: UiEvent) => void,
  ): Promise<void> {
    const splitter = new JsonLineSplitter()
    const decoder = new TextDecoder()
    const handle = (line: string) => {
      const parsed = parseUiEventLine(line)
      if (parsed.ok) onEvent(parsed.event)
      else
        this.options.onUnknownLine?.(
          line,
          parsed.reason === 'unknown_type' ? `unknown event ${parsed.type}` : parsed.error,
        )
    }
    try {
      for await (const chunk of readChunks(stream)) {
        for (const line of splitter.push(decoder.decode(chunk, { stream: true })))
          handle(line)
      }
    } catch {
      // The pipe closed; `exited` reports how.
    }
    const tail = splitter.flush()
    if (tail) handle(tail)
  }

  private async pumpStderr(stream: ReadableStream<Uint8Array>): Promise<void> {
    const splitter = new JsonLineSplitter()
    const decoder = new TextDecoder()
    try {
      for await (const chunk of readChunks(stream)) {
        for (const line of splitter.push(decoder.decode(chunk, { stream: true }))) {
          this.stderrError = stderrErrorMessage(line) ?? this.stderrError
          this.options.onStderr?.(line)
        }
      }
    } catch {
      // Logs are best-effort.
    }
    const tail = splitter.flush()
    if (tail) {
      this.stderrError = stderrErrorMessage(tail) ?? this.stderrError
      this.options.onStderr?.(tail)
    }
  }

  private write(message: ClientMessage): void {
    if (this.connection !== 'ready' || !this.child) {
      throw new EngineNotConnectedError(this.lastError ?? undefined)
    }
    this.child.stdin.write(encodeClientMessage(message))
    this.child.stdin.flush()
  }

  send(command: EngineCommand): void {
    this.write(commandMessage(command))
  }
  submit(text: string, attachments: Attachment[] = []): void {
    this.send({ type: 'submit', text, attachments })
  }
  slash(command: string, args = ''): void {
    this.send({ type: 'slash', command, args })
  }
  setModel(model: string): void {
    this.send({ type: 'set_model', model })
  }
  listModels(): void {
    this.send({ type: 'list_models' })
  }
  interrupt(): void {
    // Ignored by the engine when no turn runs; a stopped engine has none.
    if (this.connection !== 'ready') return
    this.send({ type: 'interrupt' })
  }
  replyToPermission(reply: PermissionReply): void {
    this.write(permissionReplyMessage(reply))
  }
  subscribe(listener: (event: UiEvent) => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }
  onExit(listener: (exit: EngineExit) => void): () => void {
    this.exitListeners.add(listener)
    return () => this.exitListeners.delete(listener)
  }

  async shutdown(): Promise<EngineExit> {
    const child = this.child
    if (!child || !this.exitPromise) {
      return { code: 0, signal: null }
    }
    try {
      if (this.connection === 'ready') {
        child.stdin.write(encodeClientMessage(commandMessage({ type: 'shutdown' })))
        child.stdin.flush()
      }
      // End of input also shuts the engine down.
      child.stdin.end()
    } catch {
      // The pipe is already gone.
    }
    const exited = this.exitPromise
    const timeout = new Promise<'timeout'>((resolve) =>
      setTimeout(() => resolve('timeout'), SHUTDOWN_GRACE_MS),
    )
    const outcome = await Promise.race([exited, timeout])
    if (outcome === 'timeout') {
      child.kill()
      return exited
    }
    return outcome
  }
}
