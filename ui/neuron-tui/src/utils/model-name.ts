/**
 * How the UI names a model to a person: the catalog's name without the way
 * the Zero-X gateway reaches it. Neuron never shows the provider a model is
 * routed through (Groq, OpenRouter, NVIDIA, Z.ai, ...).
 *
 *   North Mini Code (OpenRouter, free)   →  North Mini Code
 *   GPT-OSS 120B (Groq)                  →  GPT-OSS 120B
 *   cohere/north-mini-code:free (no name) →  north-mini-code
 *   auto                                 →  Auto
 *
 * The raw id stays where a power user looks for it: `/status`, the line
 * REPL's `/model` list and the engine's JSON output. The engine sends names
 * already cleaned (`model_display_name`, `served_model_display_name`), and
 * this cleans the catalog's and older engines' names the same way. Two
 * catalog models whose names are then the same are told apart by a number
 * ({@link sameNameSuffixes}).
 *
 * The same rules are kept in three places: this file (its cleaning code is
 * the shared client's, ui/neuron-client/src/copy/model-name.ts) and
 * `clean_model_name` in rust/crates/rusty-claude-cli/src/gateway_catalog.rs.
 * All three answer the cases in
 * rust/crates/rusty-claude-cli/tests/fixtures/model-names.json, which each
 * one's tests read: change a rule in all three, and add its case to that file.
 */

import type { ModelInfo } from '../engine/protocol'

/**
 * What a part of a group at the end of a display name is, whole and in lower case, when it names the route
 * and not the model.
 */
const ROUTE_WORDS: ReadonlySet<string> = new Set([
  'openrouter',
  'groq',
  'nvidia',
  'nvidia nim',
  'nim',
  'workers ai',
  'cloudflare',
  'cloudflare workers ai',
  'google ai studio',
  'vertex',
  'vertex ai',
  'cerebras',
  'together',
  'together ai',
  'fireworks',
  'deepinfra',
  'sambanova',
  'hugging face',
  'huggingface',
  'azure',
  'zero-x',
  'free',
  'paid',
  'free tier',
  'paid tier',
  // The owner's rule names Z.ai, in the spellings a catalog uses. A name that merely has these letters in it
  // is not a match, as a part has to be exactly one of them.
  'z.ai',
  'zai',
  'z-ai',
])

/**
 * The providers' words. A part that is not one of {@link ROUTE_WORDS} is still a route when every word of it
 * (lower case, with `-`, `_` and `·` as spaces) is a {@link ROUTE_TOKENS} and at least one is one of these:
 * "Groq Cloud", "OpenRouter free", "Workers-AI", "Together.ai". The hyphenated spellings can only match
 * whole, through ROUTE_WORDS, as a hyphen is a space here.
 */
const PROVIDER_WORDS: ReadonlySet<string> = new Set([
  'openrouter',
  'groq',
  'nvidia',
  'nim',
  'workers',
  'cloudflare',
  'google',
  'vertex',
  'cerebras',
  'together',
  'fireworks',
  'deepinfra',
  'sambanova',
  'hugging',
  'huggingface',
  'azure',
  'zero-x',
  'bedrock',
  'novita',
  'nebius',
  'ollama',
  'z.ai',
  'zai',
  'z-ai',
  'together.ai',
])

/**
 * What a route is made of: the providers' words, and what goes with them ("free", "Cloud", "Inference"). Alone
 * a bare "(AI)" or "(Cloud)" is not a route, as it has no provider's word. "face" and "studio" only finish
 * "Hugging Face" and "Google AI Studio" when something follows them ("Hugging Face free").
 */
const ROUTE_TOKENS: ReadonlySet<string> = new Set([
  ...PROVIDER_WORDS,
  'free',
  'paid',
  'tier',
  'cloud',
  'ai',
  'inference',
  'api',
  'face',
  'studio',
])

/** Brackets a group can be in, opening and closing, including the full-width parentheses. Any closer closes any opener. */
const OPENERS = '([（'
const CLOSERS = ')]）'
/** What separates the parts of a group. */
const SEPARATORS = ',，·|'
/** How deep groups inside groups are looked at: a name is not a tree, so more than this is left as it came. */
const MAX_DEPTH = 6

/**
 * Characters that show nothing: a soft hyphen, zero-width spaces and joiners, a word joiner, a byte order mark.
 * Written as escapes, never as the characters, so they can be seen; the `u` flag makes `\u{..}` mean them.
 */
const INVISIBLE = /[\u{ad}\u{200b}-\u{200d}\u{2060}\u{feff}]/u
/** The same, to take them all out (a global pattern is stateful with `.test`, so there are two). */
const INVISIBLE_ALL = /[\u{ad}\u{200b}-\u{200d}\u{2060}\u{feff}]/gu

/** Whitespace of every kind (a no-break space, a tab, a full-width space) and the characters that show nothing. */
const isBlank = (char: string): boolean => /\s/u.test(char) || INVISIBLE.test(char)

/** `text` without blanks at the end. A loop: a pattern such as `\s+$` is quadratic on a long run of them. */
function trimEndAll(text: string): string {
  let end = text.length
  while (end > 0 && isBlank(text[end - 1]!)) end--
  return text.slice(0, end)
}

/** `text` without blanks at either end. */
function trimAll(text: string): string {
  const end = trimEndAll(text)
  let start = 0
  while (start < end.length && isBlank(end[start]!)) start++
  return end.slice(start)
}

/** `text` as route words are compared with: width folded (`Ｇｒｏｑ` is `Groq`), nothing invisible, lower case. */
const folded = (text: string): string => text.normalize('NFKC').replace(INVISIBLE_ALL, '').toLowerCase().trim()

/** `key` with `-`, `_` and `·` as spaces, in single spaces. */
const spaced = (key: string): string => key.replace(/[-_·]+/g, ' ').replace(/\s+/g, ' ').trim()

/** Whether `part` (a whole part of a group, or a whole name) names a route: `provider` is the catalog's, if it is one. */
function isRoute(part: string, provider: string | null | undefined): boolean {
  const key = folded(part)
  if (key === '') return false
  if (ROUTE_WORDS.has(key)) return true
  const words = spaced(key).split(' ')
  if (provider && spaced(folded(provider)) === words.join(' ')) return true
  return words.every((word) => ROUTE_TOKENS.has(word)) && words.some((word) => PROVIDER_WORDS.has(word))
}

/** A group of brackets at the end of a name. */
interface Group {
  /** The blanks before its opener, as they came. */
  gap: string
  open: string
  close: string
  /** What is between the brackets. */
  inner: string
  /** The group as it came, brackets included. */
  text: string
}

/** The index of the opener that matches the closer at the end of `text`, or -1. */
function openerOf(text: string): number {
  let depth = 0
  for (let index = text.length - 1; index >= 0; index--) {
    const char = text[index]!
    if (CLOSERS.includes(char)) depth++
    else if (OPENERS.includes(char) && --depth === 0) return index
  }
  return -1
}

/** `text` as what comes before the groups at its end, and those groups, in order. */
function peelGroups(text: string): { base: string; groups: Group[] } {
  let rest = trimEndAll(text)
  const groups: Group[] = []
  while (rest !== '' && CLOSERS.includes(rest[rest.length - 1]!)) {
    const open = openerOf(rest)
    if (open < 0) break
    const before = rest.slice(0, open)
    const base = trimEndAll(before)
    groups.push({ gap: before.slice(base.length), open: rest[open]!, close: rest[rest.length - 1]!, inner: rest.slice(open + 1, -1), text: rest.slice(open) })
    rest = base
  }
  return { base: rest, groups: groups.reverse() }
}

/** `inner` cut at its separators, outside any brackets inside it. */
function splitParts(inner: string): string[] {
  const parts: string[] = []
  let depth = 0
  let start = 0
  for (let index = 0; index < inner.length; index++) {
    const char = inner[index]!
    if (OPENERS.includes(char)) depth++
    else if (CLOSERS.includes(char)) depth = Math.max(0, depth - 1)
    else if (depth === 0 && SEPARATORS.includes(char)) {
      parts.push(inner.slice(start, index))
      start = index + 1
    }
  }
  parts.push(inner.slice(start))
  return parts
}

/**
 * What is left of a group's `inner` once its route parts are off: `null` when nothing is, `inner` itself
 * when none was a route, else what is kept, separated by commas.
 */
function scrubGroup(inner: string, provider: string | null | undefined, depth: number): string | null {
  const kept: string[] = []
  let changed = false
  for (const raw of splitParts(inner)) {
    const part = trimAll(raw)
    const cleaned = scrub(part, provider, depth + 1)
    if (cleaned === '' || isRoute(cleaned, provider)) {
      changed = true
      continue
    }
    if (cleaned !== part) changed = true
    kept.push(cleaned)
  }
  if (kept.length === 0) return null
  return changed ? kept.join(', ') : inner
}

/**
 * `text` without the groups at its end that only name a route, and without the route in those that hold
 * more. `''` when what is left is itself only a route. Returned as it came (trimmed) when nothing was taken
 * off, so a name without a route keeps its spacing; a group that stays keeps its brackets and its gap.
 */
function scrub(text: string, provider: string | null | undefined, depth = 0): string {
  const whole = trimAll(text)
  if (depth > MAX_DEPTH) return whole
  const { base, groups } = peelGroups(whole)
  let rest = base
  let changed = false
  for (const group of groups) {
    const inner = scrubGroup(group.inner, provider, depth)
    if (inner === null) {
      changed = true
    } else if (inner === group.inner) {
      rest += group.gap + group.text
    } else {
      changed = true
      rest += group.gap + group.open + inner + group.close
    }
  }
  const result = changed ? trimAll(rest) : whole
  // A name that is only a route has nothing to show. The catalog's provider counts for parts of a group, not
  // for a whole name: a model may be named like the provider that serves it.
  return isRoute(result, null) ? '' : result
}

/** The id without the `openai/` transport prefix the engine adds. */
const shownId = (id: string): string => id.trim().replace(/^openai\//, '')

/**
 * The name for model `id`, from its catalog `displayName` (served by `provider`, the catalog's `owned_by`)
 * when it has one, without the way the gateway reaches it. When there is no name, or nothing is left of it
 * (it was only a route), or it is the id (or has a `/`, so is one), the last segment of the id, without its tag.
 */
export function cleanModelName(id: string, displayName?: string | null, provider?: string | null): string {
  const shown = shownId(id)
  if (shown.toLowerCase() === 'auto') return 'Auto'
  const name = displayName?.trim()
  if (name) {
    const cleaned = scrub(name, provider)
    const key = cleaned.toLowerCase()
    if (cleaned !== '' && !cleaned.includes('/') && key !== shown.toLowerCase() && key !== id.trim().toLowerCase()) return cleaned
  }
  const leaf = shown.split('/').pop()!.split(':')[0]!
  return leaf || shown
}

/** Whether `id` is the gateway's `auto` (it picks a model per request). */
export const isAutoModel = (id: string): boolean => shownId(id).toLowerCase() === 'auto'

type NamedModel = Pick<ModelInfo, 'id' | 'display_name' | 'provider'>

const suffixCache = new WeakMap<ReadonlyArray<NamedModel>, Map<string, string>>()

/**
 * What tells a catalog's models apart when their names are the same without
 * the route ("GPT-OSS 20B (Groq)" and "GPT-OSS 20B (NVIDIA)" are both
 * "GPT-OSS 20B"): nothing for the first in catalog order, then " (2)",
 * " (3)", ..., which do not name the route either. By catalog id; a model
 * whose name no earlier one has is left out.
 */
export function sameNameSuffixes(models: ReadonlyArray<NamedModel>): Map<string, string> {
  const cached = suffixCache.get(models)
  if (cached) return cached
  const seen = new Map<string, number>()
  const suffixes = new Map<string, string>()
  for (const model of models) {
    const key = cleanModelName(model.id, model.display_name, model.provider).toLowerCase()
    const count = (seen.get(key) ?? 0) + 1
    seen.set(key, count)
    if (count > 1) suffixes.set(model.id, ` (${count})`)
  }
  suffixCache.set(models, suffixes)
  return suffixes
}

/** `name` for catalog model `id`, with its {@link sameNameSuffixes} suffix when it has one. */
export function distinctModelName(
  models: ReadonlyArray<NamedModel> | null | undefined,
  id: string | null | undefined,
  name: string,
): string {
  if (!models || !id) return name
  return name + (sameNameSuffixes(models).get(id) ?? '')
}

/** Routes that resell other makers' models: their monogram would name the route. */
const ROUTE_PROVIDERS: ReadonlySet<string> = new Set([
  'groq',
  'openrouter',
  'cloudflare',
  'workers-ai',
  'nim',
  'together',
  'fireworks',
  'deepinfra',
  'cerebras',
  'sambanova',
  'azure',
])

/** Model families by the start of the model's own name, and who makes them. */
const FAMILIES: ReadonlyArray<[RegExp, string]> = [
  [/^(gemini|gemma)/, 'google'],
  [/^(gpt|chatgpt|o[1-9](\b|-))/, 'openai'],
  [/^(claude|opus|sonnet|haiku)/, 'anthropic'],
  [/^llama/, 'meta'],
  [/^(qwen|qwq)/, 'qwen'],
  [/^(glm|chatglm)/, 'zhipu'],
  [/^deepseek/, 'deepseek'],
  [/^(mistral|mixtral|codestral|devstral|magistral|ministral)/, 'mistral'],
  [/^nemotron/, 'nvidia'],
  [/^kimi/, 'moonshot'],
  [/^minimax/, 'minimax'],
  [/^(north|command|aya)/, 'cohere'],
  [/^grok/, 'xai'],
  [/^step/, 'stepfun'],
  [/^phi/, 'microsoft'],
]

/** Org segments of ids (`groq/openai/...`, `@cf/meta/...`) that name the maker. */
const MAKER_ORGS: Readonly<Record<string, string>> = {
  google: 'google',
  openai: 'openai',
  anthropic: 'anthropic',
  meta: 'meta',
  'meta-llama': 'meta',
  qwen: 'qwen',
  'z-ai': 'zhipu',
  zai: 'zhipu',
  thudm: 'zhipu',
  'deepseek-ai': 'deepseek',
  deepseek: 'deepseek',
  mistralai: 'mistral',
  nvidia: 'nvidia',
  moonshotai: 'moonshot',
  minimax: 'minimax',
  cohere: 'cohere',
  'x-ai': 'xai',
  xai: 'xai',
  'stepfun-ai': 'stepfun',
  microsoft: 'microsoft',
}

/**
 * Who made model `id` (`openai` for `groq/openai/gpt-oss-120b`, `meta` for
 * `@cf/meta/llama-3.3-70b`), for the status bar's monogram; `zero-x` for
 * `auto`. `provider` (the catalog's) counts only when it is not a route that
 * resells others' models. `null` when nothing says.
 */
export function modelMaker(id: string, provider?: string | null): string | null {
  const shown = shownId(id).toLowerCase()
  if (shown === 'auto' || shown.startsWith('auto-') || shown.startsWith('zero-x/')) return 'zero-x'
  const segments = shown.split('/')
  const leaf = segments[segments.length - 1]!.split(':')[0]!
  for (const [family, maker] of FAMILIES) if (family.test(leaf)) return maker
  for (const segment of segments.slice(0, -1).reverse()) {
    const maker = MAKER_ORGS[segment]
    if (maker) return maker
  }
  if (segments[0] === 'ollama' || segments[0] === 'local') return 'local'
  const own = provider?.trim().toLowerCase()
  return own && !ROUTE_PROVIDERS.has(own) && own !== 'nvidia' ? own : null
}

/** The id as the gateway lists it: lower case, `openai/` stripped only when a provider id follows. */
const normalizedId = (id: string): string => {
  const lowered = id.trim().toLowerCase()
  const rest = lowered.startsWith('openai/') ? lowered.slice('openai/'.length) : null
  return rest !== null && (rest.includes('/') || rest === 'auto' || rest === 'default') ? rest : lowered
}

/**
 * The catalog entry for `asked`. The gateway lists one entry per model family
 * (`openai/gpt-oss-120b`), but the engine strips `openai/` (`gpt-oss-120b`) and
 * a saved session or setting can hold a route id (`groq/openai/gpt-oss-120b`,
 * `nvidia/nvidia/nemotron-3-ultra-550b-a55b`): each finds its family. Mirrors
 * `Catalog::find` in rust/crates/rusty-claude-cli/src/gateway_catalog.rs.
 */
export function findCatalogModel<T extends { id: string }>(
  models: ReadonlyArray<T> | null | undefined,
  asked: string,
): T | null {
  if (!models || !asked.trim()) return null
  const wanted = normalizedId(asked)
  const byId = (id: string): T | undefined => models.find((model) => normalizedId(model.id) === id)
  const variantOf = (variant: string): T | undefined => {
    if (!variant.includes('/')) return undefined
    let best: T | undefined
    for (const model of models) {
      const id = normalizedId(model.id)
      if (!id.includes('/') || !variant.startsWith(id)) continue
      const next = variant[id.length]
      if ((next === '-' || next === ':') && (!best || model.id.length > best.id.length)) best = model
    }
    return best
  }
  const bare = wanted.replace(/^(openai|xai)\//, '')
  const exact = byId(wanted) ?? byId(bare) ?? byId(`openai/${bare}`) ?? byId(`xai/${bare}`)
  if (exact) return exact
  // `provider/maker/name`: each tail that is still `maker/name` or longer.
  let tail = wanted
  for (;;) {
    const slash = tail.indexOf('/')
    if (slash < 0) break
    tail = tail.slice(slash + 1)
    if (!tail.includes('/')) break
    const found = byId(tail) ?? variantOf(tail)
    if (found) return found
  }
  return variantOf(tail) ?? null
}
