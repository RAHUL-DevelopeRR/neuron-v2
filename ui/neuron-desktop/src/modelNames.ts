import { cleanModelName, distinctModelName, findCatalogModel, type ModelInfo } from '@neuron/client'

/**
 * Neuron never shows the provider a model is routed through (Groq, OpenRouter,
 * NVIDIA, Z.ai, ...). The gateway's catalog carries it in its display names
 * ("GPT-OSS 120B (Groq)"), so every place the desktop names a model goes
 * through here rather than showing a `display_name` as it comes. The raw id
 * stays where it has to be one: what a pick sends and what Settings saves.
 */

/** What the engine's `status` says about the name of the model it is on, or of the one that answered. */
export interface EngineModelName {
  /**
   * `model_display_name` or `served_model_display_name`. An engine from 7.0.0-alpha.5 sends it cleaned;
   * an older one sends the catalog's name as it is, route and all, so it is cleaned here as well.
   */
  name?: string | null
  /** `provider`: who serves the model. A trailing "(provider)" in a name is a route too. */
  provider?: string | null
}

/**
 * Model `id` as people read it: the engine's own name for it when it sent one, else the catalog's
 * (the entry is found by the id the engine asks with, which is not always the catalog's:
 * the engine strips `openai/` and a saved session can hold a route id), else the model part of the id.
 * Without its route either way, and numbered when another catalog model has the same name, as in the picker.
 */
export function modelLabel(id: string, models: readonly ModelInfo[], engine: EngineModelName = {}): string {
  const entry = findCatalogModel(models, id)
  const name = engine.name?.trim() || entry?.display_name
  return distinctModelName(models, entry?.id, cleanModelName(id, name, engine.provider ?? entry?.provider))
}

/** Catalog model `model` as the picker and Settings list it. */
export function modelRowName(models: readonly ModelInfo[], model: ModelInfo): string {
  return distinctModelName(models, model.id, cleanModelName(model.id, model.display_name, model.provider))
}

/**
 * The gateway's `unavailable_reason` codes, in words: the terminal UI's own (components/model-picker.tsx there).
 * Any other text is shown as sent: it is free text, and the model-name rules, which work on the end of a name,
 * cannot scrub a provider's name out of a sentence.
 */
const UNAVAILABLE_REASONS: ReadonlyMap<string, string> = new Map([
  ['upgrade_required', 'needs a plan upgrade'],
  ['region_unavailable', 'not available in your region'],
  ['not_configured', 'not configured on this gateway'],
])

/**
 * "back in 3h 20m" for a model the gateway took out of rotation, from its `available_again_at`: minutes, or
 * hours and minutes, and never less than a minute. `null` without a time that can be read.
 */
export function backInLabel(availableAgainAt: string | null | undefined, now: number): string | null {
  if (!availableAgainAt) return null
  const at = Date.parse(availableAgainAt)
  if (Number.isNaN(at)) return null
  const minutes = Math.max(1, Math.ceil((at - now) / 60_000))
  const hours = Math.floor(minutes / 60)
  const rest = minutes % 60
  return `back in ${hours > 0 ? `${hours}h${rest > 0 ? ` ${rest}m` : ''}` : `${rest}m`}`
}

/**
 * Whether the gateway has taken `model` out of rotation and not yet given it back: until its
 * `available_again_at`, or until told otherwise when there is no time that can be read.
 */
export function isBlurred(model: ModelInfo, now: number): boolean {
  if (model.status !== 'blurred') return false
  const at = model.available_again_at ? Date.parse(model.available_again_at) : NaN
  return Number.isNaN(at) || at > now
}

/**
 * What the picker says about `model` besides its name, in words, as the terminal UI does: why it cannot be
 * picked (a code in words, or "not on your plan" when none is given), when the gateway gives it back, or for a
 * model that can be picked its health when that is worth a glance ("degraded", "busy") and its plans ("free",
 * "paid", "pro": plans, not providers). No other gateway field is printed as it came.
 */
export function modelDetail(model: ModelInfo, now: number = Date.now()): string {
  if (!model.available) {
    const reason = model.unavailable_reason
    return reason ? (UNAVAILABLE_REASONS.get(reason) ?? reason) : 'not on your plan'
  }
  if (isBlurred(model, now)) return backInLabel(model.available_again_at, now) ?? 'temporarily unavailable'
  const health = model.status === 'degraded' || model.status === 'busy' ? model.status : null
  return [health, model.tiers.join(', ')].filter(Boolean).join(' · ')
}
