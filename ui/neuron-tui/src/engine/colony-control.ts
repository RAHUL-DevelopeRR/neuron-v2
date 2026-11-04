/**
 * Which engines have a Colony run going (`/colony <task>`), so `/colony stop`
 * reaches the run at once. Engine slash commands queue behind the exchange in
 * progress (./neuron-engine-client.ts `runExchange`), and the running
 * `/colony` exchange ends only with the run, so a queued `/colony stop` would
 * arrive after the run had finished. While a run goes, the router sends the
 * engine an interrupt instead, as Esc does: it stops the run's agents and
 * starts no more, and nothing else is running then.
 */

import type { NeuronEngineClient } from './neuron-engine-client'

const running = new WeakSet<NeuronEngineClient>()

/** The `/colony` exchange that starts a run was sent to `engine`. */
export function colonyRunStarted(engine: NeuronEngineClient): void {
  running.add(engine)
}

/** That exchange ended (the run finished, failed or was stopped). */
export function colonyRunEnded(engine: NeuronEngineClient): void {
  running.delete(engine)
}

export function isColonyRunning(engine: NeuronEngineClient): boolean {
  return running.has(engine)
}

/** `/colony stop`'s arguments. */
export function isColonyStop(args: string[]): boolean {
  return args.length === 1 && args[0]?.toLowerCase() === 'stop'
}

/** Whether `/colony <args>` starts a run (`stop` and `status` do not). */
export function startsColonyRun(args: string[]): boolean {
  const words = args.filter((word) => word.length > 0)
  if (words.length === 0) return false
  return !(
    words.length === 1 && ['stop', 'status'].includes(words[0]!.toLowerCase())
  )
}
