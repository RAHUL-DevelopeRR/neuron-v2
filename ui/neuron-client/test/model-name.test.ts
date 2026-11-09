import { describe, expect, test } from 'bun:test'

import { cleanModelName, distinctModelName, findCatalogModel, sameNameSuffixes } from '../src/copy/model-name'

// The same cases as gateway_catalog.rs's `model_names_leave_out_the_route_to_the_model`
// and the terminal UI's utils/__tests__/model-name.test.ts: the three must agree.
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
    expect(cleanModelName('x/m', 'Model (Preview, Groq)', null)).toBe('Model (Preview, Groq)')
    expect(cleanModelName('x', '(Groq)', null)).toBe('(Groq)')
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
    // An empty part is not a route.
    expect(cleanModelName('x/m', 'Model (Groq,)', null)).toBe('Model (Groq,)')
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
    expect(cleanModelName('x/m', '<b>Bold</b> (Groq)', null)).toBe('<b>Bold</b>')
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
