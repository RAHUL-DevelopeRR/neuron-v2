import { describe, expect, test } from 'bun:test'

import { cleanModelName, distinctModelName, findCatalogModel, sameNameSuffixes } from '../src/copy/model-name'

// The cases of gateway_catalog.rs's `model_names_leave_out_the_route_to_the_model` and the terminal UI's
// utils/__tests__/model-name.test.ts, where the three agree. This copy is stricter on purpose (see "stricter
// than Rust and the terminal UI" below), so two of the cases and one of this file's own differ and say why.
describe('model names', () => {
  test('leave out the route the gateway takes to the model', () => {
    expect(cleanModelName('cohere/north-mini-code:free', 'North Mini Code (OpenRouter, free)', 'openrouter')).toBe(
      'North Mini Code',
    )
    expect(cleanModelName('groq/openai/gpt-oss-120b', 'GPT-OSS 120B (Groq)', 'groq')).toBe('GPT-OSS 120B')
    expect(
      cleanModelName('@cf/meta/llama-3.3-70b-instruct-fp8-fast', 'Llama 3.3 70B (Workers AI)', 'cloudflare'),
    ).toBe('Llama 3.3 70B')
    expect(cleanModelName('nvidia/stepfun-ai/step-5-preview', 'Step 5 Preview (NVIDIA)', null)).toBe('Step 5 Preview')
    expect(cleanModelName('gemini/gemini-2.5-flash', 'Gemini 2.5 Flash', 'gemini')).toBe('Gemini 2.5 Flash')
  })

  test('keep a parenthesis about the model itself', () => {
    expect(cleanModelName('x/qwen', 'Qwen3 (32B)', null)).toBe('Qwen3 (32B)')
    // Rust and the terminal UI leave a group alone unless all of it is a route ("Model (Preview, Groq)").
    // The owner's rule is that a provider's name is never shown, so this copy takes the route out of the
    // group and keeps what is about the model.
    expect(cleanModelName('x/m', 'Model (Preview, Groq)', null)).toBe('Model (Preview)')
    // And a name that is only a route has nothing to show: it is the id's last segment (Rust: "(Groq)").
    expect(cleanModelName('x', '(Groq)', null)).toBe('x')
  })

  test('treat the provider the catalog names as a route too', () => {
    expect(cleanModelName('x/m', 'Fast (Acme)', 'acme')).toBe('Fast')
  })

  test('name auto Auto, whatever its catalog entry says', () => {
    expect(cleanModelName('auto', 'Auto (best available model)', 'zero-x')).toBe('Auto')
    expect(cleanModelName('openai/auto')).toBe('Auto')
    expect(cleanModelName('  AUTO  ', 'whatever')).toBe('Auto')
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
    expect(distinctModelName(undefined, 'nvidia/openai/gpt-oss-20b', 'GPT-OSS 20B')).toBe('GPT-OSS 20B')
    expect(distinctModelName(catalog, undefined, 'GPT-OSS 20B')).toBe('GPT-OSS 20B')
    expect(distinctModelName(catalog, null, 'GPT-OSS 20B')).toBe('GPT-OSS 20B')
    expect(distinctModelName(catalog, '', 'GPT-OSS 20B')).toBe('GPT-OSS 20B')
  })

  test('without a display name, use the model part of the id', () => {
    expect(cleanModelName('openai/cohere/north-mini-code:free')).toBe('north-mini-code')
    expect(cleanModelName('claude-sonnet-4-6')).toBe('claude-sonnet-4-6')
    expect(cleanModelName('x', '  ')).toBe('x')
    expect(cleanModelName('x', null, 'groq')).toBe('x')
    expect(cleanModelName('groq/openai/gpt-oss-120b', undefined, 'groq')).toBe('gpt-oss-120b')
  })

  test('take off every trailing route, whatever its case, and nothing else', () => {
    expect(cleanModelName('x/m', 'Model (Groq) (free)', null)).toBe('Model')
    expect(cleanModelName('x/m', 'Model (Preview) (Groq)', null)).toBe('Model (Preview)')
    expect(cleanModelName('x/m', 'Model (GROQ)', null)).toBe('Model')
    expect(cleanModelName('x/m', 'Model (OpenRouter , Free)', null)).toBe('Model')
    expect(cleanModelName('x/m', '  Model   (Workers AI)  ', null)).toBe('Model')
    // A route word inside the name, or before the end, is the model's own.
    expect(cleanModelName('x/m', 'Groq Fast', null)).toBe('Groq Fast')
    expect(cleanModelName('x/m', 'Model (Groq) Turbo', null)).toBe('Model (Groq) Turbo')
    // An empty part is nothing to keep: what is left of "(Groq,)" is nothing, so the group goes too
    // (it used to stay, route and all, because the empty part was not a route).
    expect(cleanModelName('x/m', 'Model (Groq,)', null)).toBe('Model')
  })

  test('cleaning a name that is already clean changes nothing', () => {
    for (const name of ['GPT-OSS 120B', 'North Mini Code', 'Llama 3.3 70B', 'Qwen3 (32B)', 'Auto']) {
      expect(cleanModelName('x/m', name, 'groq')).toBe(name)
      expect(cleanModelName('x/m', cleanModelName('x/m', name, 'groq'), 'groq')).toBe(name)
    }
  })

  test('handle blank ids and names with special characters', () => {
    expect(cleanModelName('')).toBe('')
    expect(cleanModelName('   ')).toBe('')
    expect(cleanModelName('x/m', 'Modèle 日本語 🚀 (Groq)', null)).toBe('Modèle 日本語 🚀')
    expect(cleanModelName('x/m', "Robert'); DROP TABLE models;-- (Groq)", null)).toBe("Robert'); DROP TABLE models;--")
    // Markup is text. A name with a `/` in it is an id, not a name (a closing tag has one), so it is the id's
    // last segment; this copy used to keep it ("<b>Bold</b>").
    expect(cleanModelName('x/m', '<b>Bold</b> (Groq)', null)).toBe('m')
    expect(cleanModelName('x/m', '<b>Bold (Groq)', null)).toBe('<b>Bold')
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
})

// The owner's rule is hard: a provider's name is never shown. gateway_catalog.rs and the terminal UI take off
// only a plain " (Route)" at the end of a name; this copy also takes off the shapes a catalog's names can have
// beyond that, so none of them reaches the screen. A model's own name is never scrubbed: a maker's name inside
// it ("NVIDIA Nemotron 3 Ultra") is the model's, and the owner decides that separately.
describe('model names, stricter than Rust and the terminal UI on purpose', () => {
  const clean = (name: string, provider: string | null = null) => cleanModelName('x/model', name, provider)

  /** Every shape a route takes after a name, with what a person should read. */
  const SHAPES: Array<[string, string]> = [
    // Other brackets, no space before the bracket, and any whitespace before it.
    ['GPT-OSS 120B [Groq]', 'GPT-OSS 120B'],
    ['GPT-OSS 120B(Groq)', 'GPT-OSS 120B'],
    ['GPT-OSS 120B[Groq]', 'GPT-OSS 120B'],
    ['GPT-OSS 120B\u{a0}(Groq)', 'GPT-OSS 120B'],
    ['GPT-OSS 120B\t(Groq)', 'GPT-OSS 120B'],
    ['GPT-OSS 120B\u{a0}[Groq]', 'GPT-OSS 120B'],
    ['GPT-OSS 120B\u{3000}（Groq）', 'GPT-OSS 120B'],
    ['GPT-OSS 120B（Groq）', 'GPT-OSS 120B'],
    ['GPT-OSS 120B (Groq]', 'GPT-OSS 120B'],
    ['GPT-OSS 120B (Groq)\u{a0}', 'GPT-OSS 120B'],
    // A group with other things in it keeps them; it goes when nothing is left.
    ['Model (Preview, Groq)', 'Model (Preview)'],
    ['Model (Groq, Preview)', 'Model (Preview)'],
    ['Model (Preview · Groq)', 'Model (Preview)'],
    ['Model (Preview | Groq)', 'Model (Preview)'],
    ['Model (a, b, Groq)', 'Model (a, b)'],
    ['Model [Preview, Groq]', 'Model [Preview]'],
    ['Model（Preview，Groq）', 'Model（Preview）'],
    ['Model (Groq) (Preview)', 'Model (Preview)'],
    ['Model (Preview) [Groq]', 'Model (Preview)'],
    // A group that stays keeps its own spacing and brackets.
    ['Model(Preview)(Groq)', 'Model(Preview)'],
    ['Model  (Preview)  (Groq)', 'Model  (Preview)'],
    ['Model[Preview] (Groq)', 'Model[Preview]'],
    ['Model\u{a0}(Preview)\u{a0}(Groq)', 'Model\u{a0}(Preview)'],
    ['Model (a (Groq))', 'Model (a)'],
    ['Model (a (Groq) (free))', 'Model (a)'],
    ['Model ((Groq))', 'Model'],
    ['Model (Groq, free)', 'Model'],
    ['Model (Groq,)', 'Model'],
    ['Model ()', 'Model'],
    // Spelling variants of a route.
    ['Model (OpenRouter free)', 'Model'],
    ['Model (OpenRouter · free)', 'Model'],
    ['Model (OpenRouter | free)', 'Model'],
    ['Model (Groq Cloud)', 'Model'],
    ['Model (Groq Inference)', 'Model'],
    ['Model (Fireworks API)', 'Model'],
    ['Model (Together.ai)', 'Model'],
    ['Model (Together.AI)', 'Model'],
    ['Model (Workers-AI)', 'Model'],
    ['Model (Workers_AI)', 'Model'],
    ['Model (Workers AI)', 'Model'],
    ['Model (Cloudflare Workers AI)', 'Model'],
    ['Model (Hugging Face)', 'Model'],
    ['Model (Google AI Studio)', 'Model'],
    ['Model (Vertex AI)', 'Model'],
    ['Model (NVIDIA NIM)', 'Model'],
    ['Model (Free tier)', 'Model'],
    ['Model (Hugging Face free)', 'Model'],
    ['Model (Google AI Studio free)', 'Model'],
    ['Model (HuggingFace · free)', 'Model'],
    // Providers the list lacked.
    ['Model (Bedrock)', 'Model'],
    ['Model (Novita)', 'Model'],
    ['Model (Nebius)', 'Model'],
    ['Model (Ollama)', 'Model'],
    // Characters that make a word look different without being one.
    ['Model (Ｇｒｏｑ)', 'Model'],
    ['Model (Gr\u{200b}oq)', 'Model'],
    ['Model (Groq)\u{200b}', 'Model'],
  ]

  /** What is not a route, and so is the model's own: left exactly as it came. */
  const STAYS: string[] = [
    'Model (AI)',
    'Model [AI]',
    'Model (Preview)',
    'Model (Cloud)',
    'Model (API)',
    'Model (Inference)',
    'Model (Tier 2)',
    'Model (Preview · Beta)',
    'Model (OpenAI)',
    'Model (Meta)',
    'Model (Gemini Pro)',
    'Model (Free Preview)',
    'Model (Groq Fast)',
    'Model (Google Gemma)',
    'Model (Azure Dragon)',
    'Model (Not Groq)',
    'NVIDIA Nemotron 3 Ultra',
    'Google Gemma 3',
    'Groq Fast',
    'Free Willy',
    'Ollama Chat',
    'Together We Stand',
    'Azure Dragon',
    'Model (Groq) Turbo',
    // Not a clean name tidied: how it came, spacing and all.
    'Qwen3(32B)',
    'Model  (Preview)',
    'Model\u{a0}(Preview)',
  ]

  test('every shape a route takes after a name is taken off', () => {
    for (const [name, expected] of SHAPES) expect([name, clean(name)]).toEqual([name, expected])
  })

  test('every provider, in every bracket, whitespace and case', () => {
    const providers = [
      'OpenRouter', 'Groq', 'NVIDIA', 'NIM', 'Workers AI', 'Cloudflare', 'Google', 'Vertex', 'Cerebras', 'Together',
      'Fireworks', 'DeepInfra', 'SambaNova', 'Hugging Face', 'HuggingFace', 'Azure', 'Zero-X', 'Bedrock', 'Novita',
      'Nebius', 'Ollama', 'Z.ai', 'Zai', 'Z-ai', 'Together.ai',
    ]
    const brackets = [['(', ')'], ['[', ']'], ['（', '）']] as const
    const gaps = ['', ' ', '  ', '\t', '\u{a0}', '\u{3000}']
    let tried = 0
    for (const provider of providers) {
      for (const form of [provider, provider.toLowerCase(), provider.toUpperCase()]) {
        for (const [open, close] of brackets) {
          for (const gap of gaps) {
            const name = `Model${gap}${open}${form}${close}`
            expect([name, clean(name)]).toEqual([name, 'Model'])
            tried++
          }
        }
      }
    }
    expect(tried).toBe(providers.length * 3 * brackets.length * gaps.length)
  })

  test('a provider with free, paid, a tier or the cloud beside it, in one part, is still a route', () => {
    // Words of one part: `-` and `_` are spaces in a word, and every word is a route word.
    for (const joiner of [' ', '-', ' - ', '_']) {
      for (const extra of ['free', 'paid', 'tier', 'Cloud', 'AI', 'Inference', 'API']) {
        const name = `Model (Groq${joiner}${extra})`
        expect([name, clean(name)]).toEqual([name, 'Model'])
      }
    }
  })

  test('with a separator each part is judged alone: a route goes, a bare AI, Cloud or Preview stays', () => {
    for (const separator of [', ', ' · ', ' | ', '，', '|']) {
      for (const [extra, expected] of [
        ['free', 'Model'],
        ['paid', 'Model'],
        ['Cloud', 'Model (Cloud)'],
        ['AI', 'Model (AI)'],
        ['Preview', 'Model (Preview)'],
      ] as const) {
        const name = `Model (Groq${separator}${extra})`
        expect([name, clean(name)]).toEqual([name, expected])
      }
    }
  })

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

  test('what is not a route is left exactly as it came', () => {
    for (const name of STAYS) expect([name, clean(name)]).toEqual([name, name])
  })

  test('a name that is only a route, or only the id, is the last segment of the id, without its tag', () => {
    const id = 'groq/openai/gpt-oss-120b'
    for (const name of ['(Groq)', '[Groq]', '（Groq）', 'Groq', 'GROQ', 'Groq (free)', '(Groq) (free)']) {
      expect([name, cleanModelName(id, name, 'groq')]).toEqual([name, 'gpt-oss-120b'])
    }
    expect(cleanModelName('x', '(Groq)', null)).toBe('x')
    expect(cleanModelName('cohere/north-mini-code:free', '(OpenRouter, free)', 'openrouter')).toBe('north-mini-code')
    // A name with a slash in it is an id, not a name; so is one that is the id.
    expect(cleanModelName(id, id)).toBe('gpt-oss-120b')
    expect(cleanModelName(id, 'Groq/OpenAI/GPT-OSS-120B (Groq)')).toBe('gpt-oss-120b')
    expect(cleanModelName('openai/gpt-oss-120b', 'openai/gpt-oss-120b')).toBe('gpt-oss-120b')
    expect(cleanModelName('cohere/north-mini-code:free', 'cohere/north-mini-code:free')).toBe('north-mini-code')
    expect(cleanModelName('gpt-oss-120b', 'GPT-OSS-120B')).toBe('gpt-oss-120b')
    expect(cleanModelName('x/m', 'Model/Turbo')).toBe('m')
    // Nothing to fall back on.
    expect(cleanModelName('', 'Groq')).toBe('')
  })

  test("the catalog's provider is a route in any bracket, and only when it is the whole part", () => {
    expect(clean('Fast [Acme]', 'acme')).toBe('Fast')
    expect(clean('Fast（Acme）', 'acme')).toBe('Fast')
    expect(clean('Fast (Acme, free)', 'acme')).toBe('Fast')
    expect(clean('Fast (Preview, Acme)', 'acme')).toBe('Fast (Preview)')
    expect(clean('Fast (Workers AI)', 'workers-ai')).toBe('Fast')
    expect(clean('Fast (Acme Labs)', 'acme')).toBe('Fast (Acme Labs)')
    expect(clean('Fast (Acme)', null)).toBe('Fast (Acme)')
    // Inside a group only: a model may be named like the provider that serves it.
    expect(clean('Acme', 'acme')).toBe('Acme')
    expect(clean('Acme Fast', 'acme')).toBe('Acme Fast')
    expect(cleanModelName('acme/m', 'Cerebras', 'cerebras')).toBe('m')
  })

  test('is idempotent: cleaning what was cleaned changes nothing', () => {
    const names = [...SHAPES.map(([name]) => name), ...STAYS, 'Llama 3.3 70B (Workers AI)', 'GLM 5.2 (Z.ai)', 'Auto (best available model)']
    for (const name of names) {
      const once = cleanModelName('groq/openai/gpt-oss-120b', name, 'groq')
      expect([name, cleanModelName('groq/openai/gpt-oss-120b', once, 'groq')]).toEqual([name, once])
    }
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
})

// This client's own cases: the owner's rule names Z.ai as a provider never shown, but gateway_catalog.rs
// and the terminal UI have no Z.ai route word yet, so only this copy cleans it by name.
describe('Z.ai is a route too', () => {
  test('a name ending in (Z.ai) is cleaned, whatever the catalog says the provider is', () => {
    for (const provider of [null, undefined, '', 'z.ai', 'zai', 'z-ai', 'zhipu', 'openrouter', 'acme']) {
      expect(cleanModelName('z-ai/glm-5.2', 'GLM 5.2 (Z.ai)', provider)).toBe('GLM 5.2')
    }
  })

  test('every spelling, in any case, alone or beside another route', () => {
    for (const spelling of ['Z.ai', 'z.ai', 'Z.AI', 'zai', 'Zai', 'ZAI', 'z-ai', 'Z-AI']) {
      expect(cleanModelName('x/m', `GLM 5.2 (${spelling})`, null)).toBe('GLM 5.2')
    }
    expect(cleanModelName('x/m', 'GLM 4.7 Flash (Z.ai, free)', null)).toBe('GLM 4.7 Flash')
    expect(cleanModelName('x/m', 'GLM 4.7 Flash (OpenRouter, Z.ai)', null)).toBe('GLM 4.7 Flash')
    expect(cleanModelName('x/m', 'GLM 4.7 Flash (Z.ai) (free)', null)).toBe('GLM 4.7 Flash')
    expect(cleanModelName('x/m', 'GLM 4.7 Flash (free) (Z.ai)', null)).toBe('GLM 4.7 Flash')
  })

  test('a model whose own name merely has those letters in it is left alone', () => {
    for (const name of [
      'Zaire 7B',
      'Mazai Chat',
      'Gonzaiz Pro',
      'Zaidoo (Preview)',
      'Lazai Pro (32B)',
      'Model (Zaire)',
      'Model (Zaitech)',
      'Model (Z.ai Labs)',
      'Model (Zai Labs)',
      'Model (not-zai)',
      'Z.ai Model',
      'Zai Model',
    ]) {
      expect(cleanModelName('x/m', name, null)).toBe(name)
      expect(cleanModelName('x/m', name, 'zai')).toBe(name)
    }
    // No catalog name: the id's model part, which is the model's own name.
    expect(cleanModelName('acme/zai-large')).toBe('zai-large')
    expect(cleanModelName('openrouter/z-ai/glm-4.7-flash:free')).toBe('glm-4.7-flash')
  })

  test('two models that differ only by that route are told apart by a number, not by the route', () => {
    const catalog = [
      { id: 'z-ai/glm-5.2', display_name: 'GLM 5.2 (Z.ai)', provider: 'openrouter' },
      { id: 'openrouter/glm-5.2', display_name: 'GLM 5.2 (OpenRouter)', provider: 'openrouter' },
    ]
    expect([...sameNameSuffixes(catalog)]).toEqual([['openrouter/glm-5.2', ' (2)']])
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
