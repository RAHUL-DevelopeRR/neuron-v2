/**
 * The model chip in the phone's session header: Neuron never shows the provider a model is routed through
 * (Groq, OpenRouter, NVIDIA, Z.ai, ...), and the engine's `status` carries the model as an id
 * (`groq/openai/gpt-oss-120b`), so the chip names it through the shared client's cleaner.
 */
import { describe, expect, test } from 'bun:test'
import fs from 'node:fs'
import path from 'node:path'

import { statusModelName } from '../src/modelName'

describe('the model chip names the model the way a person reads it', () => {
  test('an id with a route in front is its model part', () => {
    expect(statusModelName({ model: 'groq/openai/gpt-oss-120b' })).toBe('gpt-oss-120b')
    expect(statusModelName({ model: 'openai/cohere/north-mini-code:free' })).toBe('north-mini-code')
    expect(statusModelName({ model: 'openrouter/z-ai/glm-5.2' })).toBe('glm-5.2')
    expect(statusModelName({ model: 'nvidia/nvidia/nemotron-3-ultra-550b-a55b' })).toBe('nemotron-3-ultra-550b-a55b')
  })

  test("the engine's own name for the model is used, and cleaned again for an engine that does not clean it", () => {
    expect(statusModelName({ model: 'groq/openai/gpt-oss-120b', model_display_name: 'GPT-OSS 120B' })).toBe('GPT-OSS 120B')
    // An older engine sends the catalog's name as it is, route and all.
    expect(statusModelName({ model: 'groq/openai/gpt-oss-120b', model_display_name: 'GPT-OSS 120B (Groq)', provider: 'groq' })).toBe('GPT-OSS 120B')
    expect(statusModelName({ model: 'z-ai/glm-5.2', model_display_name: 'GLM 5.2 (Z.ai)', provider: 'openrouter' })).toBe('GLM 5.2')
    expect(statusModelName({ model: 'x/m', model_display_name: 'Model (Preview, Groq)' })).toBe('Model (Preview)')
  })

  test('a name that is only a route, or only the id, is the last segment of the id', () => {
    expect(statusModelName({ model: 'groq/openai/gpt-oss-120b', model_display_name: 'Groq', provider: 'groq' })).toBe('gpt-oss-120b')
    expect(statusModelName({ model: 'z-ai/glm-5.2', model_display_name: 'z-ai/glm-5.2' })).toBe('glm-5.2')
    expect(statusModelName({ model: 'z-ai/glm-5.2', model_display_name: null, provider: null })).toBe('glm-5.2')
  })

  test('auto is Auto, whatever its entry says', () => {
    expect(statusModelName({ model: 'auto' })).toBe('Auto')
    expect(statusModelName({ model: 'openai/auto', model_display_name: 'Auto (best available model)', provider: 'zero-x' })).toBe('Auto')
  })

  test('no provider name reaches the chip, whatever shape the status has', () => {
    const statuses = [
      { model: 'groq/openai/gpt-oss-120b' },
      { model: 'openrouter/cohere/north-mini-code:free', model_display_name: 'North Mini Code (OpenRouter, free)' },
      { model: 'nvidia/stepfun-ai/step-5-preview', model_display_name: 'Step 5 Preview [NVIDIA]' },
      { model: 'z-ai/glm-5.2', model_display_name: 'GLM 5.2 (Z.ai)' },
      { model: 'z-ai/glm-5.2', model_display_name: 'GLM 5.2（Z.ai）' },
      { model: 'zai/glm-5.2', model_display_name: 'GLM 5.2 (zai, free)' },
      { model: 'x/m', model_display_name: 'Model (Preview | Groq)' },
    ]
    for (const status of statuses) {
      expect([status, statusModelName(status)]).toEqual([status, expect.not.stringMatching(/groq|openrouter|nvidia|z\.ai|zai|z-ai/i)])
    }
  })
})

describe('the session screen', () => {
  const screen = fs.readFileSync(path.resolve(import.meta.dir, '../src/screens/SessionScreen.tsx'), 'utf8')

  test("shows the model through the helper and never the status's raw id", () => {
    // The screen cannot be rendered here (no React Native in a bun test), so the guard is on its source: a
    // chip that shows `status.model` as it comes puts a route-prefixed id on the phone.
    expect(screen).toContain('statusModelName(status)')
    expect(screen).not.toMatch(/\{\s*status\??\.model\s*\}/)
  })
})
