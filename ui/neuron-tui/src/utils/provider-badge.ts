/**
 * The monogram the status bar draws in front of the model name: plain text,
 * never a logo image, a brand colour or a company's name.
 *
 *   ◇ Auto → Gemini 2.5 Flash      G Gemini 2.5 Flash      OA GPT-OSS 120B
 *
 * It names who made the model (utils/model-name.ts `modelMaker`), never the
 * route the Zero-X gateway takes to it: GPT-OSS 120B is OpenAI's whether
 * Groq, Workers AI or NVIDIA serves it, so the routes that resell other
 * makers' models (Groq, OpenRouter, Cloudflare, NIM) have no monogram.
 * Zero-X's `auto` router has its own mark. Unknown makers get a neutral dot.
 */

import type { GlyphMode } from './glyphs'

/** Monograms by maker id (lower case). */
const MONOGRAMS: Readonly<Record<string, string>> = {
  'zero-x': '◇',
  zerox: '◇',
  google: 'G',
  gemini: 'G',
  openai: 'OA',
  anthropic: 'A',
  meta: 'M',
  qwen: 'Q',
  zhipu: 'Z',
  deepseek: 'DS',
  mistral: 'MI',
  nvidia: 'NV',
  moonshot: 'K',
  minimax: 'MM',
  cohere: 'C',
  xai: 'X',
  stepfun: 'SF',
  microsoft: 'MS',
  ollama: 'L',
  local: 'L',
}

/** What a console font can draw for the marks above that are not letters. */
const CONSOLE_MONOGRAMS: Readonly<Record<string, string>> = {
  '◇': '*',
}

export const UNKNOWN_PROVIDER_BADGE = { unicode: '•', console: '.' } as const

/** The monogram for `maker` in this glyph mode. */
export function providerBadge(maker: string | null | undefined, mode: GlyphMode): string {
  const known = maker ? MONOGRAMS[maker.trim().toLowerCase()] : undefined
  if (!known) return UNKNOWN_PROVIDER_BADGE[mode]
  return mode === 'console' ? (CONSOLE_MONOGRAMS[known] ?? known) : known
}
