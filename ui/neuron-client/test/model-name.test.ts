import { describe, expect, expectTypeOf, test } from 'bun:test'
import fs from 'node:fs'
import path from 'node:path'

import { cleanModelName, distinctModelName, findCatalogModel, sameNameSuffixes } from '../src/copy/model-name'

// The owner's rule is hard: a provider's name is never shown. Three copies of the cleaner keep it (this package's,
// the terminal UI's utils/model-name.ts and `clean_model_name` in rust/crates/rusty-claude-cli/src/gateway_catalog.rs),
// and all three answer the cases in one file, read by each one's tests, so none of them can drift from the others:
// rust/crates/rusty-claude-cli/tests/fixtures/model-names.json (its `$comment` says how the cases are built).
// Below them are this package's own tests: the numbering of names that are the same, the depth limit, the catalog lookup.
type Case = [id: string, displayName: string | null, provider: string | null, expected: string]

interface Vectors {
  sections: Array<{ name: string; cases: Case[] }>
  providerShapes: { providers: string[]; brackets: Array<[string, string]>; gaps: string[]; prefix: string; expected: string }
  joinedRoutes: { joiners: string[]; extras: string[]; expected: string }
  separatedRoutes: { separators: string[]; extras: Array<[string, string]> }
}

const vectors = JSON.parse(
  fs.readFileSync(path.resolve(import.meta.dir, '../../../rust/crates/rusty-claude-cli/tests/fixtures/model-names.json'), 'utf8'),
) as Vectors

const clean = (name: string, provider: string | null = null) => cleanModelName('x/model', name, provider)

describe('model names: the cases every copy of the cleaner answers the same way', () => {
  for (const section of vectors.sections) {
    test(section.name, () => {
      expect(section.cases.length).toBeGreaterThan(0)
      for (const [id, displayName, provider, expected] of section.cases) {
        expect([id, displayName, provider, cleanModelName(id, displayName, provider)]).toEqual([id, displayName, provider, expected])
      }
    })
  }

  test('every provider, in every bracket, whitespace and case', () => {
    const { providers, brackets, gaps, prefix, expected } = vectors.providerShapes
    let tried = 0
    for (const provider of providers) {
      for (const form of [provider, provider.toLowerCase(), provider.toUpperCase()]) {
        for (const [open, close] of brackets) {
          for (const gap of gaps) {
            const name = `${prefix}${gap}${open}${form}${close}`
            expect([name, clean(name)]).toEqual([name, expected])
            tried++
          }
        }
      }
    }
    expect(tried).toBe(providers.length * 3 * brackets.length * gaps.length)
  })

  test('a provider with free, paid, a tier or the cloud beside it, in one part, is still a route', () => {
    // Words of one part: `-` and `_` are spaces in a word, and every word is a route word.
    const { joiners, extras, expected } = vectors.joinedRoutes
    for (const joiner of joiners) {
      for (const extra of extras) {
        const name = `Model (Groq${joiner}${extra})`
        expect([name, clean(name)]).toEqual([name, expected])
      }
    }
  })

  test('with a separator each part is judged alone: a route goes, a bare AI, Cloud or Preview stays', () => {
    const { separators, extras } = vectors.separatedRoutes
    for (const separator of separators) {
      for (const [extra, expected] of extras) {
        const name = `Model (Groq${separator}${extra})`
        expect([name, clean(name)]).toEqual([name, expected])
      }
    }
  })

  test('is idempotent: cleaning what was cleaned changes nothing', () => {
    let tried = 0
    for (const section of vectors.sections) {
      for (const [id, displayName, provider] of section.cases) {
        const once = cleanModelName(id, displayName, provider)
        expect([id, once, provider, cleanModelName(id, once, provider)]).toEqual([id, once, provider, once])
        // The same names on a route id, the way the engine's own notices meet them.
        const routed = cleanModelName('groq/openai/gpt-oss-120b', displayName, 'groq')
        expect([displayName, cleanModelName('groq/openai/gpt-oss-120b', routed, 'groq')]).toEqual([displayName, routed])
        tried++
      }
    }
    // Guards the loop itself: an emptied or misread file would pass it with nothing tried.
    expect(tried).toBeGreaterThan(150)
  })
})

describe('model names, numbered when they are the same', () => {
  test('number catalog models whose names are the same, in catalog order, without naming the route', () => {
    const catalog = [
      { id: 'groq/openai/gpt-oss-20b', display_name: 'GPT-OSS 20B (Groq)', provider: 'groq' },
      { id: 'gemini/gemini-2.5-flash', display_name: 'Gemini 2.5 Flash', provider: 'gemini' },
      { id: 'nvidia/openai/gpt-oss-20b', display_name: 'GPT-OSS 20B (NVIDIA)', provider: 'nvidia' },
      { id: 'y/gpt-oss-20b', display_name: 'gpt-oss 20b', provider: null },
    ]
    expect([...sameNameSuffixes(catalog)]).toEqual([
      ['nvidia/openai/gpt-oss-20b', ' (2)'],
      ['y/gpt-oss-20b', ' (3)'],
    ])
    // The same catalog gives the same answer (it is kept, not worked out again).
    expect(sameNameSuffixes(catalog)).toBe(sameNameSuffixes(catalog))
    expect(distinctModelName(catalog, 'nvidia/openai/gpt-oss-20b', 'GPT-OSS 20B')).toBe('GPT-OSS 20B (2)')
    expect(distinctModelName(catalog, 'groq/openai/gpt-oss-20b', 'GPT-OSS 20B')).toBe('GPT-OSS 20B')
    expect(distinctModelName(catalog, 'not/in-catalog', 'X')).toBe('X')
    expect(distinctModelName(null, 'nvidia/openai/gpt-oss-20b', 'GPT-OSS 20B')).toBe('GPT-OSS 20B')
    expect(distinctModelName(undefined, 'nvidia/openai/gpt-oss-20b', 'GPT-OSS 20B')).toBe('GPT-OSS 20B')
    expect(distinctModelName(catalog, undefined, 'GPT-OSS 20B')).toBe('GPT-OSS 20B')
    expect(distinctModelName(catalog, null, 'GPT-OSS 20B')).toBe('GPT-OSS 20B')
    expect(distinctModelName(catalog, '', 'GPT-OSS 20B')).toBe('GPT-OSS 20B')
  })

  test('numbering handles a large catalog', () => {
    const models = Array.from({ length: 10_000 }, (_, index) => ({
      id: `route-${index}/model-${index % 5_000}`,
      display_name: `Model ${index % 5_000} (Groq)`,
      provider: 'groq',
    }))
    const suffixes = sameNameSuffixes(models)
    expect(suffixes.size).toBe(5_000)
    expect(suffixes.get('route-5000/model-0')).toBe(' (2)')
    expect(suffixes.has('route-0/model-0')).toBe(false)
  })

  test('an empty catalog numbers nothing', () => {
    expect(sameNameSuffixes([]).size).toBe(0)
  })

  test("hands out a catalog's numbering to read, not to change: it is kept and shared by every caller", () => {
    // A compile-time check (`bun run typecheck`): a caller that could `.set` or `.delete` on it would change
    // what every other caller of the same catalog reads.
    expectTypeOf(sameNameSuffixes([])).toEqualTypeOf<ReadonlyMap<string, string>>()
  })

  test('numbers models whose names are the same once every route is off', () => {
    const catalog = [
      { id: 'a/m', display_name: 'GPT-OSS 20B [Groq]', provider: null },
      { id: 'b/m', display_name: 'GPT-OSS 20B（NVIDIA）', provider: null },
      { id: 'c/m', display_name: 'GPT-OSS 20B (Together.ai)', provider: null },
      { id: 'd/m', display_name: 'GPT-OSS 20B (Preview, Bedrock)', provider: null },
    ]
    expect([...sameNameSuffixes(catalog)]).toEqual([
      ['b/m', ' (2)'],
      ['c/m', ' (3)'],
    ])
  })

  test('two models that differ only by the Z.ai route are told apart by a number, not by the route', () => {
    const catalog = [
      { id: 'z-ai/glm-5.2', display_name: 'GLM 5.2 (Z.ai)', provider: 'openrouter' },
      { id: 'openrouter/glm-5.2', display_name: 'GLM 5.2 (OpenRouter)', provider: 'openrouter' },
    ]
    expect([...sameNameSuffixes(catalog)]).toEqual([['openrouter/glm-5.2', ' (2)']])
  })
})

describe('model names, deep and long ones', () => {
  test('groups inside groups are looked at only so deep, so no name can overflow the stack', () => {
    const deep = `Model ${'('.repeat(100_000)}Groq${')'.repeat(100_000)}`
    expect(() => clean(deep)).not.toThrow()
    expect(clean(deep).startsWith('Model')).toBe(true)
    // Within reach, a route in a group in a group is off.
    expect(clean('Model (a (b (c (Groq))))')).toBe('Model (a (b (c)))')
    // A long name is not slow either.
    const started = Date.now()
    clean(`${'word '.repeat(20_000)}(Groq)`)
    clean(`Model${'(a)'.repeat(2_000)}(Groq)`)
    expect(Date.now() - started).toBeLessThan(2_000)
  })
})

// The same cases as gateway_catalog.rs's `family_ids_and_route_ids_find_the_same_catalog_entry`.
describe('finding a model in the catalog', () => {
  const catalog = [
    { id: 'openai/gpt-oss-120b', display_name: 'GPT-OSS 120B', provider: 'openai' },
    { id: 'openai/gpt-oss-20b', display_name: 'GPT-OSS 20B', provider: 'openai' },
    { id: 'nvidia/nemotron-3-ultra-550b', display_name: 'Nemotron 3 Ultra 550B', provider: 'nvidia' },
    { id: 'cohere/north-mini-code', display_name: 'North Mini Code', provider: 'cohere' },
  ]

  test('a family entry answers to its own id, the bare id the engine asks with and route ids', () => {
    const asks: Array<[string, string]> = [
      ['openai/gpt-oss-120b', 'openai/gpt-oss-120b'],
      ['gpt-oss-120b', 'openai/gpt-oss-120b'],
      ['GPT-OSS-120B', 'openai/gpt-oss-120b'],
      ['groq/openai/gpt-oss-120b', 'openai/gpt-oss-120b'],
      ['openai/groq/openai/gpt-oss-120b', 'openai/gpt-oss-120b'],
      ['workers-ai/openai/gpt-oss-20b', 'openai/gpt-oss-20b'],
      ['nvidia/nvidia/nemotron-3-ultra-550b-a55b', 'nvidia/nemotron-3-ultra-550b'],
      ['openrouter/cohere/north-mini-code:free', 'cohere/north-mini-code'],
    ]
    for (const [asked, wanted] of asks) expect(findCatalogModel(catalog, asked)?.id).toBe(wanted)
  })

  test('a model the catalog lacks stays unknown, and a longer name is not a prefix match', () => {
    for (const asked of ['acme/made-up-model', 'groq/openai/gpt-oss-1200b', 'groq/openai/gpt-oss-12', 'nvidia/nemotron-3-ultra']) {
      expect(findCatalogModel(catalog, asked)).toBeNull()
    }
  })

  test('finds nothing without a catalog or without an id', () => {
    expect(findCatalogModel(null, 'gpt-oss-120b')).toBeNull()
    expect(findCatalogModel(undefined, 'gpt-oss-120b')).toBeNull()
    expect(findCatalogModel([], 'gpt-oss-120b')).toBeNull()
    expect(findCatalogModel(catalog, '')).toBeNull()
    expect(findCatalogModel(catalog, '   ')).toBeNull()
  })

  test('hands back the catalog entry itself, whatever else it carries', () => {
    const entry = { id: 'openai/gpt-oss-120b', display_name: 'GPT-OSS 120B (Groq)', provider: 'groq', context_window: 131072 }
    expect(findCatalogModel([entry], 'groq/openai/gpt-oss-120b')).toBe(entry)
  })

  test('finds auto and the xai transport prefix', () => {
    const models = [{ id: 'auto' }, { id: 'xai/grok-4' }]
    expect(findCatalogModel(models, 'auto')?.id).toBe('auto')
    expect(findCatalogModel(models, 'openai/auto')?.id).toBe('auto')
    expect(findCatalogModel(models, 'grok-4')?.id).toBe('xai/grok-4')
  })

  test('a route-id name for a family found this way is cleaned like the family is', () => {
    const entry = findCatalogModel(
      [{ id: 'openai/gpt-oss-120b', display_name: 'GPT-OSS 120B (Groq)', provider: 'groq' }],
      'groq/openai/gpt-oss-120b',
    )
    expect(cleanModelName('groq/openai/gpt-oss-120b', entry?.display_name, entry?.provider)).toBe('GPT-OSS 120B')
  })
})
