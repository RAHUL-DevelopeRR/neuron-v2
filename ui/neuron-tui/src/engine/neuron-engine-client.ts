/**
 * The UI's handle on the Neuron engine: `neuron engine --stdio`, spoken over
 * JSON lines (see ./protocol.ts and rust/crates/neuron-tui/src/protocol.rs).
 * The chat codes against {@link NeuronEngineClient}; the process-wide client
 * is a {@link StdioEngineClient} (./stdio-engine-client.ts).
 *
 * Tokens stay in the engine: nothing here reads or forwards credentials.
 */

import { StdioEngineClient } from './stdio-engine-client'

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
  constructor(readonly reason?: string) {
    super(
      reason
        ? `The Neuron engine is not running: ${reason}`
        : 'The Neuron engine is not running.',
    )
    this.name = 'EngineNotConnectedError'
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

// Each engine runs one exchange that ends in `turn_finished` at a time (a
// prompt's turn, a slash command), so a `turn_finished` always belongs to the
// exchange waiting for it.
const exchangeTails = new WeakMap<NeuronEngineClient, Promise<unknown>>()

/**
 * Sends a command that the engine ends with `turn_finished` (a submit, a
 * slash command) once the exchanges before it are over, passing every event
 * of its own exchange to `onEvent`. Resolves at its `turn_finished`; rejects
 * when sending fails (for example {@link EngineNotConnectedError}) or the
 * engine exits first.
 */
export function runExchange(
  engine: NeuronEngineClient,
  send: () => void,
  onEvent: (event: UiEvent) => void,
): Promise<TurnOutcome> {
  const previous = exchangeTails.get(engine) ?? Promise.resolve()
  const run = previous.then(
    () =>
      new Promise<TurnOutcome>((resolve, reject) => {
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
          send()
        } catch (sendError) {
          cleanup()
          reject(sendError)
        }
      }),
  )
  exchangeTails.set(
    engine,
    run.catch(() => undefined),
  )
  return run
}

/** Runs a prompt's turn: {@link runExchange} with a `submit`. */
export function runTurn(
  engine: NeuronEngineClient,
  turn: { text: string; attachments?: Attachment[] },
  onEvent: (event: UiEvent) => void,
): Promise<TurnOutcome> {
  return runExchange(
    engine,
    () => engine.submit(turn.text, turn.attachments ?? []),
    onEvent,
  )
}

let client: NeuronEngineClient | null = null

/** The process-wide engine client. */
export function getNeuronEngineClient(): NeuronEngineClient {
  client ??= new StdioEngineClient()
  return client
}

/** Replace the process-wide client (a restart in another folder). */
export function setNeuronEngineClient(next: NeuronEngineClient): void {
  client = next
}

/** Test seam: install a fake client (or `null` to go back to the default). */
export function setNeuronEngineClientForTesting(
  next: NeuronEngineClient | null,
): void {
  client = next
}
