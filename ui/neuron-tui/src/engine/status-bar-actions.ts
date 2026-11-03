/**
 * What the prompt status bar asks of the engine: a thinking level, a
 * permission mode, the branches, a branch switch, the account. Engines with
 * feature `status_bar` take them as commands that answer with a `status` and
 * add nothing to the chat; older ones get the slash commands the chat would
 * send (`/permissions <mode>`, `/plan on|off`), which answer with a notice.
 */

import { getNeuronEngineClient, runExchange, type NeuronEngineClient } from './neuron-engine-client'
import { FEATURE_STATUS_BAR, type StatusEvent } from './protocol'
import { hasEngineFeature, useEngineStore } from '../state/engine-store'
import { logger } from '../utils/logger'

/** A permission-mode choice: a mode (plan mode off), or plan mode. */
export type PermissionChoice = 'read-only' | 'workspace-write' | 'danger-full-access' | 'plan'

export interface PermissionOption {
  id: PermissionChoice
  label: string
  description: string
}

export const PERMISSION_OPTIONS: readonly PermissionOption[] = [
  { id: 'read-only', label: 'Read-only', description: 'Reads and searches; no edits' },
  { id: 'workspace-write', label: 'Edit workspace', description: 'Edits files in this folder' },
  { id: 'danger-full-access', label: 'Full access', description: 'No limits (enabled at launch)' },
  { id: 'plan', label: 'Plan', description: 'Plans the work first; changes nothing until you say so' },
]

/** The choice the status shows now: plan mode, else the mode. */
export function currentPermissionChoice(status: StatusEvent | null): PermissionChoice | null {
  if (!status) return null
  if (status.plan_mode) return 'plan'
  const mode = status.permission_mode
  return PERMISSION_OPTIONS.some((option) => option.id === mode) ? (mode as PermissionChoice) : null
}

/**
 * The choices Shift+Tab cycles and the picker lists. Full access only when
 * the engine started with it (`full_access_allowed`), or when it is the mode
 * already (an older engine that does not say).
 */
export function permissionChoices(status: StatusEvent | null): PermissionOption[] {
  const allowed =
    status?.full_access_allowed === true ||
    (status?.permission_mode === 'danger-full-access' && status.full_access_allowed === undefined)
  return PERMISSION_OPTIONS.filter((option) => option.id !== 'danger-full-access' || allowed)
}

/** Shift+Tab: Read-only → Edit workspace → (Full access) → Plan → Read-only. */
export function nextPermissionChoice(status: StatusEvent | null): PermissionChoice | null {
  if (!status) return null
  const choices = permissionChoices(status)
  const current = currentPermissionChoice(status)
  const index = choices.findIndex((option) => option.id === current)
  return choices[(index + 1) % choices.length]!.id
}

function readyEngine(): NeuronEngineClient | null {
  const engine = getNeuronEngineClient()
  return engine.connection === 'ready' ? engine : null
}

function supportsStatusBar(): boolean {
  return hasEngineFeature(useEngineStore.getState(), FEATURE_STATUS_BAR)
}

/** Runs a slash command for an older engine; its notice says how it went. */
function legacySlash(engine: NeuronEngineClient, command: string, args: string): Promise<void> {
  return runExchange(engine, () => engine.slash(command, args), () => {}).then(
    () => undefined,
    (error: unknown) => logger.warn({ error }, `[status-bar] /${command} failed`),
  )
}

/** Sets the permission mode (or plan mode). Returns false when there is no engine to ask. */
export function setPermissionChoice(choice: PermissionChoice): boolean {
  const engine = readyEngine()
  if (!engine) return false
  if (supportsStatusBar()) {
    engine.send({ type: 'set_permission_mode', mode: choice })
    return true
  }
  const status = useEngineStore.getState().status
  if (choice === 'plan') {
    void legacySlash(engine, 'plan', 'on')
    return true
  }
  const leavePlan = status?.plan_mode ? legacySlash(engine, 'plan', 'off') : Promise.resolve()
  void leavePlan.then(() => legacySlash(engine, 'permissions', choice))
  return true
}

/** Sets the thinking level; `null` for the model's default. */
export function setReasoningEffort(effort: string | null): boolean {
  const engine = readyEngine()
  if (!engine) return false
  if (supportsStatusBar()) {
    engine.send({ type: 'set_reasoning_effort', effort })
  } else {
    void legacySlash(engine, 'effort', effort ?? 'default')
  }
  return true
}

/** Asks for the branches (`branches` event); engines without the feature have none. */
export function requestBranches(): boolean {
  const engine = readyEngine()
  if (!engine || !supportsStatusBar()) return false
  engine.send({ type: 'list_branches' })
  return true
}

/** Switches (or creates) a branch; the engine refuses with uncommitted changes. */
export function switchBranch(name: string, create: boolean): boolean {
  const engine = readyEngine()
  if (!engine || !supportsStatusBar()) return false
  engine.send({ type: 'switch_branch', name, create })
  return true
}

/** Reads the plan and usage again (an `account` event follows). */
export function refreshAccount(): void {
  const engine = readyEngine()
  if (!engine) return
  try {
    engine.send({ type: 'refresh_account' })
  } catch (error) {
    logger.debug({ error }, '[status-bar] refresh_account failed')
  }
}
