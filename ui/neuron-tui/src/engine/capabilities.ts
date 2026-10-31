/**
 * What this UI tells the engine it can draw (the `capabilities` command).
 * Engines that list {@link FEATURE_CAPABILITIES} in `ready` get it once, right
 * after `ready`; older ones never see it and send none of the events it turns
 * on, so the UI draws their turns as before.
 *
 * This is the only place that sends it: a later `capabilities` command
 * replaces the earlier one, so a second sender would take features away.
 */

import { logger } from '../utils/logger'
import {
  CAPABILITY_GIT_VIEWS,
  CAPABILITY_LIVE_PROGRESS,
  FEATURE_CAPABILITIES,
  type ReadyEvent,
} from './protocol'

import type { NeuronEngineClient } from './neuron-engine-client'

/**
 * Everything this UI handles: tool-input progress, output estimates and
 * per-request usage while a turn streams (live_progress), and the changes,
 * diff, commit and checkpoint views (git_views).
 */
export const UI_CAPABILITIES: readonly string[] = [
  CAPABILITY_LIVE_PROGRESS,
  CAPABILITY_GIT_VIEWS,
]

/** How this UI names itself in the command. */
export const UI_CLIENT = 'terminal'

/**
 * Sends the `capabilities` command when the engine that sent `ready` takes
 * one. Returns whether it was sent. A write that fails (the engine died right
 * after `ready`) is logged: its exit is reported elsewhere.
 */
export function declareUiCapabilities(
  engine: Pick<NeuronEngineClient, 'send'>,
  ready: ReadyEvent,
): boolean {
  if (!ready.features?.includes(FEATURE_CAPABILITIES)) return false
  try {
    engine.send({
      type: 'capabilities',
      features: [...UI_CAPABILITIES],
      client: UI_CLIENT,
    })
    return true
  } catch (error) {
    logger.debug({ error }, '[engine] capabilities not sent')
    return false
  }
}
