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
