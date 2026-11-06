import { describe, expect, test } from 'bun:test'

import { cleanModelName, distinctModelName, isAutoModel, sameNameSuffixes } from '../model-name'

// The same cases as gateway_catalog.rs's
// `model_names_leave_out_the_route_to_the_model`: the two must agree.
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

  test('without a display name, use the model part of the id', () => {
    expect(cleanModelName('openai/cohere/north-mini-code:free')).toBe('north-mini-code')
    expect(cleanModelName('claude-sonnet-4-6')).toBe('claude-sonnet-4-6')
    expect(cleanModelName('x', '  ')).toBe('x')
  })
})
