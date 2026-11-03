/**
 * The provider monogram the status bar draws in front of the model name:
 * plain text, never a logo image, a brand colour or a provider's name.
 *
 *   ◇ auto → Gemini 2.5 Flash      G Gemini 2.5 Flash      Q Llama 3.3 70B
 *
 * The provider comes from the engine (`status.provider`, or the catalog's
 * `provider` for the model) and, from engines before them, is read from the
 * model id. Unknown providers get a neutral dot.
 */

import type { GlyphMode } from './glyphs'

/** Monograms by provider id (lower case). Zero-X's `auto` router has its own mark. */
const MONOGRAMS: Readonly<Record<string, string>> = {
  'zero-x': '◇',
  zerox: '◇',
  gemini: 'G',
  google: 'G',
  groq: 'Q',
  cloudflare: 'CF',
  'workers-ai': 'CF',
  nvidia: 'NV',
  nim: 'NV',
  openrouter: 'OR',
  openai: 'OA',
  anthropic: 'A',
  xai: 'X',
  ollama: 'L',
  local: 'L',
}

/** What a console font can draw for the marks above that are not letters. */
const CONSOLE_MONOGRAMS: Readonly<Record<string, string>> = {
  '◇': '*',
}

export const UNKNOWN_PROVIDER_BADGE = { unicode: '•', console: '.' } as const

/**
 * The provider for `model` when the engine did not say: `auto` is Zero-X's
 * router, and a few id shapes name their provider. `null` when nothing does.
 */
export function inferProvider(model: string): string | null {
  const id = model.trim().toLowerCase().replace(/^openai\//, '')
  if (id === 'auto' || id.startsWith('auto-') || id.startsWith('zero-x/')) return 'zero-x'
  if (id.startsWith('@cf/') || id.startsWith('cloudflare/')) return 'cloudflare'
  if (id.startsWith('nvidia/')) return 'nvidia'
  if (id.startsWith('groq/')) return 'groq'
  if (id.startsWith('openrouter/')) return 'openrouter'
  if (id.startsWith('ollama/')) return 'ollama'
  if (/^(models\/)?gemini/.test(id) || id.startsWith('google/')) return 'gemini'
  if (/^(gpt|o[1-9]|chatgpt)/.test(id)) return 'openai'
  if (/^(claude|opus|sonnet|haiku)/.test(id) || id.startsWith('anthropic/')) return 'anthropic'
  if (id.startsWith('grok') || id.startsWith('xai/')) return 'xai'
  return null
}

/** The monogram for `provider` in this glyph mode. */
export function providerBadge(provider: string | null | undefined, mode: GlyphMode): string {
  const known = provider ? MONOGRAMS[provider.trim().toLowerCase()] : undefined
  if (!known) return UNKNOWN_PROVIDER_BADGE[mode]
  return mode === 'console' ? (CONSOLE_MONOGRAMS[known] ?? known) : known
}
