import { describe, expect, test } from 'bun:test'
import fs from 'node:fs'
import path from 'node:path'

import { cleanModelName, distinctModelName, isAutoModel, sameNameSuffixes } from '../model-name'

// The owner's rule is hard: a provider's name is never shown. Three copies of the cleaner keep it (the shared
// client's ui/neuron-client/src/copy/model-name.ts, this UI's utils/model-name.ts and `clean_model_name` in
// rust/crates/rusty-claude-cli/src/gateway_catalog.rs), and all three answer the cases in one file, read by each
// one's tests, so none of them can drift from the others:
// rust/crates/rusty-claude-cli/tests/fixtures/model-names.json (its `$comment` says how the cases are built).
type Case = [id: string, displayName: string | null, provider: string | null, expected: string]

interface Vectors {
  sections: Array<{ name: string; cases: Case[] }>
  providerShapes: { providers: string[]; brackets: Array<[string, string]>; gaps: string[]; prefix: string; expected: string }
  joinedRoutes: { joiners: string[]; extras: string[]; expected: string }
  separatedRoutes: { separators: string[]; extras: Array<[string, string]> }
}

const vectors = JSON.parse(
  fs.readFileSync(
    path.resolve(import.meta.dir, '../../../../../rust/crates/rusty-claude-cli/tests/fixtures/model-names.json'),
    'utf8',
  ),
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

  test('groups inside groups are looked at only so deep, so no name can overflow the stack', () => {
    const deep = `Model ${'('.repeat(100_000)}Groq${')'.repeat(100_000)}`
    expect(() => clean(deep)).not.toThrow()
    expect(clean(deep).startsWith('Model')).toBe(true)
    // A long name is not slow either.
    const started = Date.now()
    clean(`${'word '.repeat(20_000)}(Groq)`)
    clean(`Model${'(a)'.repeat(2_000)}(Groq)`)
    expect(Date.now() - started).toBeLessThan(2_000)
  })
})

describe('model names, auto and the numbering of names that are the same', () => {
  test('auto is told by its id, whatever its catalog entry says', () => {
    expect(isAutoModel('openai/auto')).toBe(true)
    expect(isAutoModel('gemini/gemini-2.5-flash')).toBe(false)
  })

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
    expect(distinctModelName(catalog, undefined, 'GPT-OSS 20B')).toBe('GPT-OSS 20B')
  })

  test('numbers models whose names are the same once every route is off', () => {
    const catalog = [
      { id: 'a/m', display_name: 'GPT-OSS 20B [Groq]', provider: null },
      { id: 'b/m', display_name: 'GPT-OSS 20B（NVIDIA）', provider: null },
      { id: 'c/m', display_name: 'GPT-OSS 20B (Together.ai)', provider: null },
      { id: 'd/m', display_name: 'GPT-OSS 20B (Preview, Bedrock)', provider: null },
      { id: 'z-ai/glm-5.2', display_name: 'GPT-OSS 20B (Z.ai)', provider: null },
    ]
    expect([...sameNameSuffixes(catalog)]).toEqual([
      ['b/m', ' (2)'],
      ['c/m', ' (3)'],
      ['z-ai/glm-5.2', ' (4)'],
    ])
  })
})
