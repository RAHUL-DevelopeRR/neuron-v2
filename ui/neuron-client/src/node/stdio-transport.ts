/**
 * `neuron engine --stdio` in a child process (Bun): client messages on its
 * stdin, events on its stdout, logs on its stderr.
 */

import type { EngineTransport, TransportClose } from '../controller'
import {
  JsonLineSplitter,
  PROTOCOL_VERSION,
  commandMessage,
  encodeClientMessage,
  parseUiEventLine,
  type ClientMessage,
  type UiEvent,
} from '../protocol'
import type { SessionAction } from '../session'

export interface EngineLaunch {
  /** The `neuron` binary. */
  binary: string
  cwd: string
  model?: string
  permissionMode?: string
  resume?: string
  allowBroadCwd?: boolean
  /** The engine's whole environment (nothing is inherited). */
  env: Record<string, string | undefined>
}

export function engineArgs(launch: EngineLaunch): string[] {
  const args = ['engine', '--stdio']
  if (launch.model) args.push('--model', launch.model)
  if (launch.permissionMode) args.push('--permission-mode', launch.permissionMode)
  if (launch.resume) args.push('--resume', launch.resume)
  if (launch.allowBroadCwd) args.push('--allow-broad-cwd')
  return args
}

/** The message of an `error: ...` stderr line: some launch checks exit with only that. */
export function stderrErrorMessage(line: string): string | null {
  const match = /^error:\s*(.+)$/i.exec(line.trim())
  return match ? match[1].trim() : null
}

export interface StdioTransportOptions {
  readyTimeoutMs?: number
  shutdownGraceMs?: number
  onStderr?: (line: string) => void
  onUnknownLine?: (line: string, reason: string) => void
}

type Child = ReturnType<typeof Bun.spawn<'pipe', 'pipe', 'pipe'>>

export class StdioTransport implements EngineTransport {
  readonly echoesActions = false
  /** The engine's last `error: ...` on stderr, for a launch that sent no event. */
  stderrError: string | null = null

  private child: Child | null = null
  private ready = false
  private listeners = new Set<(action: SessionAction) => void>()
  private closeListeners = new Set<(close: TransportClose) => void>()
  private exited: Promise<TransportClose> | null = null

  constructor(
    private readonly launch: EngineLaunch,
    private readonly options: StdioTransportOptions = {},
  ) {}

  start(): Promise<void> {
    let child: Child
    try {
      child = Bun.spawn([this.launch.binary, ...engineArgs(this.launch)], {
        cwd: this.launch.cwd,
        env: this.launch.env,
        stdin: 'pipe',
        stdout: 'pipe',
        stderr: 'pipe',
      })
    } catch (error) {
      return Promise.reject(new Error(`Could not start ${this.launch.binary}: ${error instanceof Error ? error.message : error}`))
    }
    this.child = child
    return new Promise<void>((resolve, reject) => {
      let settled = false
      let launchError: string | null = null
      const settle = (error?: Error) => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        if (error) reject(error)
        else resolve()
      }
      const timer = setTimeout(() => {
        settle(new Error('The Neuron engine did not get ready in time.'))
        child.kill()
      }, this.options.readyTimeoutMs ?? 60_000)

      const onEvent = (event: UiEvent) => {
        if (event.type === 'ready') {
          this.ready = true
          if (event.protocol > PROTOCOL_VERSION) {
            this.options.onUnknownLine?.(JSON.stringify(event), `engine protocol ${event.protocol} is newer than ${PROTOCOL_VERSION}`)
          }
          settle()
        } else if (event.type === 'error' && !this.ready) {
          launchError = event.message
        }
        for (const listener of [...this.listeners]) listener(event)
      }
      const stdoutDone = pumpLines(child.stdout, (line) => {
        const parsed = parseUiEventLine(line)
        if (parsed.ok) onEvent(parsed.event)
        else this.options.onUnknownLine?.(line, parsed.reason === 'unknown_type' ? `unknown event ${parsed.type}` : parsed.error)
      })
      const stderrDone = pumpLines(child.stderr, (line) => {
        this.stderrError = stderrErrorMessage(line) ?? this.stderrError
        this.options.onStderr?.(line)
      })
      this.exited = Promise.all([child.exited, stdoutDone]).then(async ([code]) => {
        // A process the engine started can hold stderr open; don't wait long for it.
        await Promise.race([stderrDone, Bun.sleep(250)])
        const wasReady = this.ready
        this.ready = false
        this.child = null
        const error =
          launchError ?? (code !== 0 || !wasReady ? (this.stderrError ?? `The Neuron engine stopped (exit code ${code}).`) : null)
        settle(new Error(error ?? 'The Neuron engine stopped before it was ready.'))
        const close = { code, error }
        for (const listener of [...this.closeListeners]) listener(close)
        return close
      })
    })
  }

  send(message: ClientMessage): void {
    if (!this.ready || !this.child) throw new Error('The Neuron engine is not running.')
    this.child.stdin.write(encodeClientMessage(message))
    this.child.stdin.flush()
  }

  subscribe(listener: (action: SessionAction) => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  onClose(listener: (close: TransportClose) => void): () => void {
    this.closeListeners.add(listener)
    return () => this.closeListeners.delete(listener)
  }

  async close(): Promise<void> {
    const child = this.child
    if (!child || !this.exited) return
    try {
      if (this.ready) child.stdin.write(encodeClientMessage(commandMessage({ type: 'shutdown' })))
      // End of input shuts the engine down too.
      child.stdin.end()
    } catch {
      // The pipe is already closed.
    }
    const exited = this.exited
    const outcome = await Promise.race([exited, Bun.sleep(this.options.shutdownGraceMs ?? 5_000).then(() => null)])
    if (outcome === null) {
      child.kill()
      await exited
    }
  }
}

async function pumpLines(stream: ReadableStream<Uint8Array>, onLine: (line: string) => void): Promise<void> {
  const splitter = new JsonLineSplitter()
  const decoder = new TextDecoder()
  try {
    for await (const chunk of stream) {
      for (const line of splitter.push(decoder.decode(chunk, { stream: true }))) onLine(line)
    }
  } catch {
    // The pipe closed; the exit code says how.
  }
  const tail = splitter.flush()
  if (tail) onLine(tail)
}
