/**
 * The UI's handle on the Neuron engine: `neuron engine --stdio`, spoken over
 * JSON lines (see ./protocol.ts and rust/crates/neuron-tui/src/protocol.rs).
 *
 * P1 (this file today): the interface the chat codes against, the launch
 * arguments, and a stub that is never connected, so the UI builds and starts
 * without an engine. P2 replaces the stub with a client that spawns the
 * engine, writes {@link ClientMessage} lines to its stdin and reads
 * {@link UiEvent} lines from its stdout (logs arrive on stderr).
 *
 * Tokens stay in the engine: nothing here reads or forwards credentials.
 */

import type {
  Attachment,
  EngineCommand,
  PermissionReply,
  ReadyEvent,
  UiEvent,
} from './protocol'

/** How to launch `neuron engine --stdio`. Mirrors the engine's own flags. */
export interface EngineLaunchOptions {
  /** The `neuron` executable. P2 resolves a default (sibling binary, then PATH). */
  binary?: string
  /** Working directory for the engine; defaults to the project root. */
  cwd?: string
  model?: string
  provider?: string
  permissionMode?: string
  allowedTools?: string[]
  reasoningEffort?: string
  /** Session id to resume. */
  resume?: string
  allowBroadCwd?: boolean
  dangerouslySkipPermissions?: boolean
}

/** The argv after the binary for {@link EngineLaunchOptions}. */
export function engineArgs(options: EngineLaunchOptions = {}): string[] {
  const args = ['engine', '--stdio']
  if (options.model) args.push('--model', options.model)
  if (options.provider) args.push('--provider', options.provider)
  if (options.permissionMode)
    args.push('--permission-mode', options.permissionMode)
  if (options.allowedTools?.length)
    args.push('--allowedTools', options.allowedTools.join(','))
  if (options.reasoningEffort)
    args.push('--reasoning-effort', options.reasoningEffort)
  if (options.resume) args.push('--resume', options.resume)
  if (options.allowBroadCwd) args.push('--allow-broad-cwd')
  if (options.dangerouslySkipPermissions)
    args.push('--dangerously-skip-permissions')
  return args
}

/**
 * How the engine process ended. Exit 0 follows `shutdown` or end of input;
 * 130 follows a forced quit (two interrupts during a turn); 1 is a failed
 * launch, which sends a single `error` event first.
 */
export interface EngineExit {
  code: number | null
  signal: string | null
}

export type EngineConnection = 'not_started' | 'starting' | 'ready' | 'exited'

export interface NeuronEngineClient {
  readonly connection: EngineConnection
  /** Spawns the engine and resolves with its `ready` event. */
  start(options?: EngineLaunchOptions): Promise<ReadyEvent>
  send(command: EngineCommand): void
  /** Runs a turn. Allowed while a turn runs: an `AskUserQuestion` notice expects the next submit as its answer. */
  submit(text: string, attachments?: Attachment[]): void
  /** A slash command without its leading slash, e.g. `slash('model', 'opus')`. */
  slash(command: string, args?: string): void
  setModel(model: string): void
  listModels(): void
  /** Interrupts the running turn; ignored by the engine when none runs. */
  interrupt(): void
  replyToPermission(reply: PermissionReply): void
  /** Every event the engine sends, in order. Returns an unsubscribe function. */
  subscribe(listener: (event: UiEvent) => void): () => void
  onExit(listener: (exit: EngineExit) => void): () => void
  /** Sends `shutdown` and resolves when the process has exited. */
  shutdown(): Promise<EngineExit>
}

export class EngineNotConnectedError extends Error {
  constructor() {
    super(
      'The Neuron engine is not connected yet: this UI build has no engine bridge.',
    )
    this.name = 'EngineNotConnectedError'
  }
}

/** The P1 client: never connects, so every command fails the same way. */
class UnconnectedEngineClient implements NeuronEngineClient {
  readonly connection: EngineConnection = 'not_started'

  start(): Promise<ReadyEvent> {
    return Promise.reject(new EngineNotConnectedError())
  }
  send(_command: EngineCommand): void {
    throw new EngineNotConnectedError()
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
    // Nothing is running without an engine.
  }
  replyToPermission(_reply: PermissionReply): void {
    throw new EngineNotConnectedError()
  }
  subscribe(_listener: (event: UiEvent) => void): () => void {
    return () => {}
  }
  onExit(_listener: (exit: EngineExit) => void): () => void {
    return () => {}
  }
  shutdown(): Promise<EngineExit> {
    return Promise.resolve({ code: 0, signal: null })
  }
}

/** How a turn ended: `error` carries the engine's `error` message, if any. */
export interface TurnOutcome {
  error: string | null
}

export class EngineExitedError extends Error {
  constructor(readonly exit: EngineExit) {
    super(
      `The Neuron engine exited during the turn (code ${exit.code ?? 'none'}${
        exit.signal ? `, signal ${exit.signal}` : ''
      }).`,
    )
    this.name = 'EngineExitedError'
  }
}

/**
 * Submits a prompt and resolves at the turn's `turn_finished`, passing every
 * event before it to `onEvent`. Rejects when the submit fails (with the P1
 * stub: {@link EngineNotConnectedError}) or the engine exits mid-turn.
 */
export function runTurn(
  engine: NeuronEngineClient,
  turn: { text: string; attachments?: Attachment[] },
  onEvent: (event: UiEvent) => void,
): Promise<TurnOutcome> {
  return new Promise((resolve, reject) => {
    let error: string | null = null
    const cleanup = () => {
      unsubscribe()
      unsubscribeExit()
    }
    const unsubscribe = engine.subscribe((event) => {
      onEvent(event)
      if (event.type === 'error') error = event.message
      if (event.type === 'turn_finished') {
        cleanup()
        resolve({ error })
      }
    })
    const unsubscribeExit = engine.onExit((exit) => {
      cleanup()
      reject(new EngineExitedError(exit))
    })
    try {
      engine.submit(turn.text, turn.attachments ?? [])
    } catch (submitError) {
      cleanup()
      reject(submitError)
    }
  })
}

let client: NeuronEngineClient | null = null

/** The process-wide engine client. */
export function getNeuronEngineClient(): NeuronEngineClient {
  client ??= new UnconnectedEngineClient()
  return client
}

/** Test seam: install a fake client (or `null` to go back to the default). */
export function setNeuronEngineClientForTesting(
  next: NeuronEngineClient | null,
): void {
  client = next
}
