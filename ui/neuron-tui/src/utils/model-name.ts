/**
 * How the UI names a model to a person: the catalog's name without the way
 * the Zero-X gateway reaches it.
 *
 *   North Mini Code (OpenRouter, free)   →  North Mini Code
 *   GPT-OSS 120B (Groq)                  →  GPT-OSS 120B
 *   cohere/north-mini-code:free (no name) →  north-mini-code
 *   auto                                 →  Auto
 *
 * The raw id stays where a power user looks for it: `/status`, the line
 * REPL's `/model` list and the engine's JSON output. Mirrors
 * `clean_model_name` in rust/crates/rusty-claude-cli/src/gateway_catalog.rs;
 * the engine sends names already cleaned (`model_display_name`,
 * `served_model_display_name`), and this cleans the catalog's and older
 * engines' names the same way. Two catalog models whose names are then the
 * same are told apart by a number ({@link sameNameSuffixes}).
 */

import type { ModelInfo } from '../engine/protocol'

/** What a display name's last parenthesis holds when it names the route, not the model. */
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
])

/** The id without the `openai/` transport prefix the engine adds. */
const shownId = (id: string): string => id.trim().replace(/^openai\//, '')

export const isAutoModel = (id: string): boolean => shownId(id).toLowerCase() === 'auto'

/** `name` without trailing parentheses that only name its route (or `provider`). */
function withoutRoute(name: string, provider: string | null | undefined): string {
  let kept = name.trim()
  for (;;) {
    const open = kept.lastIndexOf(' (')
    if (open < 0 || !kept.endsWith(')')) break
    const parts = kept.slice(open + 2, -1).split(',').map((part) => part.trim())
    const onlyRoute = parts.every(
      (part) =>
        part !== '' &&
        (ROUTE_WORDS.has(part.toLowerCase()) ||
          (!!provider && provider.toLowerCase() === part.toLowerCase())),
    )
    if (!onlyRoute) break
    kept = kept.slice(0, open).trimEnd()
  }
  return kept || name.trim()
}

/**
 * The name for model `id`, from its catalog `displayName` (served by
 * `provider`, the catalog's `owned_by`) when it has one.
 */
export function cleanModelName(
  id: string,
  displayName?: string | null,
  provider?: string | null,
): string {
  const shown = shownId(id)
  if (shown.toLowerCase() === 'auto') return 'Auto'
  const name = displayName?.trim()
  if (name) return withoutRoute(name, provider)
  const leaf = shown.split('/').pop()!.split(':')[0]!
  return leaf || shown
}

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
