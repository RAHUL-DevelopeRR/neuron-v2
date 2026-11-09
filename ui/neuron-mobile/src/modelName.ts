import { cleanModelName, type StatusEvent } from '@neuron/client'

/**
 * The model a session runs on, as the chip in the session header names it. The engine's `status` carries the
 * model as an id (`groq/openai/gpt-oss-120b`), and Neuron never shows the provider a model is routed through
 * (Groq, OpenRouter, NVIDIA, Z.ai, ...). So the chip uses the engine's own name for it (an engine from
 * 7.0.0-alpha.5 sends it cleaned; an older one sends the catalog's as it is) through the shared client's
 * cleaner, and the model part of the id when there is no name.
 */
export function statusModelName(status: Pick<StatusEvent, 'model' | 'model_display_name' | 'provider'>): string {
  return cleanModelName(status.model, status.model_display_name, status.provider)
}
