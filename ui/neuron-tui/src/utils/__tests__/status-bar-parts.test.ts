import { describe, expect, test } from 'bun:test'
import stringWidth from 'string-width'

import {
  STATUS_BAR_ACCOUNT,
  STATUS_BAR_MODELS,
  STATUS_BAR_STATUS,
} from '../../testing/status-bar-fixtures'
import { modelMaker } from '../model-name'
import { providerBadge } from '../provider-badge'
import {
  effortLabel,
  folderName,
  lineWidth,
  reasoningEffortsOf,
  statusBarLine,
  statusLineText,
  usageWindowPercent,
  type StatusBarInput,
  type StatusLine,
} from '../status-bar-parts'

import type { GlyphMode } from '../glyphs'
import type { StatusEvent } from '../../engine/protocol'

const input = (
  overrides: Partial<StatusEvent> = {},
  mode: GlyphMode = 'unicode',
  rest: Partial<StatusBarInput> = {},
): StatusBarInput => ({
  status: { ...STATUS_BAR_STATUS, ...overrides },
  account: STATUS_BAR_ACCOUNT,
  models: STATUS_BAR_MODELS,
  mode,
  ...rest,
})

const render = (value: StatusBarInput, width: number): string =>
  statusLineText(statusBarLine(value, width)!, width)

const line = (value: StatusBarInput, width: number): StatusLine => statusBarLine(value, width)!

const targets = (value: StatusLine) => value.left.map((segment) => segment.target)

describe('the status bar at each width', () => {
  test('200 and 160 columns show everything, with the account on the right', () => {
    for (const width of [200, 160]) {
      const text = render(input(), width)
      expect(text.startsWith(
        'G Gemini 2.5 Flash ▾ │ ● think high │ ⌂ neuron-v2 ⎇ feat/tui-status-bar +1 ~2 │ ✎ Edit workspace │ ●●●◑○○○○○○ 35% ctx',
      )).toBe(true)
      expect(text.endsWith('Free · 5h 12% · 37/100 req')).toBe(true)
      expect(stringWidth(text)).toBe(width)
    }
  })

  test('120 columns drop the labels and shorten the account group', () => {
    const text = render(input(), 120)
    expect(text.startsWith(
      'G Gemini 2.5 Flash │ ● high │ ⌂ neuron-v2 ⎇ feat/tui-status-bar +1 ~2 │ ✎ Edit │ ●●●◑○○○○○○ 35%',
    )).toBe(true)
    expect(text.endsWith('5h 12%')).toBe(true)
    expect(stringWidth(text)).toBe(120)
  })

  test('80 columns drop the folder and git counts and halve the meter', () => {
    expect(render(input(), 80)).toBe(
      'G Gemini 2.5 Flash │ ● high │ ⎇ feat/tui-status-bar │ ✎ Edit │ ●◕○○○ 35%  5h 12%',
    )
  })

  test('60 and 40 columns cut the branch and then the model, never the mode or %', () => {
    expect(render(input(), 60)).toBe('G Gemini 2.5 Flash │ ⎇ feat/tui-status… │ ✎ Edit │ ●◕○○○ 35%')
    expect(render(input(), 44)).toBe('G Gemini 2.5 Fl… │ ⎇ feat/tu… │ ✎ Edit │ 35%')
    // Narrower still, the branch goes and the model gets its room back.
    expect(render(input(), 40)).toBe('G Gemini 2.5 Flash │ ✎ Edit │ 35%')
    expect(render(input(), 24)).toBe('G │ ✎ Edit │ 35%')
  })

  test('every width fits, down to the never-dropped parts', () => {
    for (let width = 200; width >= 24; width--) {
      const fitted = line(input(), width)
      expect(lineWidth(fitted)).toBeLessThanOrEqual(width)
      expect(targets(fitted)).toContain('permission')
      expect(targets(fitted)).toContain('context')
      expect(fitted.left[0]!.spans[0]!.text).toBe('G')
    }
  })

  test('the console glyph set draws only what classic console fonts have', () => {
    const text = render(input({}, 'console'), 160)
    expect(text).toContain('G Gemini 2.5 Flash ▼ │ ● think high │ ⌂ neuron-v2 ± feat/tui-status-bar +1 ~2 │ + Edit workspace │ ●●●●○○○○○○ 35% ctx')
    expect(text).not.toMatch(/[◔◑◕⎇✎◌▾]/)
  })
})

describe('segments', () => {
  test('Full access is the error tone and bold; plan mode reads Plan whatever the mode', () => {
    const full = line(input({ permission_mode: 'danger-full-access' }), 200)
    const permission = full.left.find((segment) => segment.target === 'permission')!
    expect(permission.spans[0]).toEqual({ text: '! Full access', tone: 'error', bold: true })

    const plan = line(input({ permission_mode: 'danger-full-access', plan_mode: true }), 200)
    expect(plan.left.find((segment) => segment.target === 'permission')!.spans[0]!.text).toBe('◇ Plan')

    const readOnly = render(input({ permission_mode: 'read-only' }), 200)
    expect(readOnly).toContain('◌ Read-only')
  })

  test('auto shows the model that answered', () => {
    const text = render(
      input({ model: 'auto', provider: 'zero-x', model_display_name: 'Auto', served_model: 'gemini-2.5-flash' }),
      200,
    )
    expect(text.startsWith('◇ Auto → Gemini 2.5 Flash ▾')).toBe(true)
  })

  test('an untrusted folder is a warning in place of the git counts', () => {
    const fitted = line(input({ workspace_trusted: false }), 200)
    const branch = fitted.left.find((segment) => segment.target === 'branch')!
    expect(branch.spans.map((span) => span.text).join('')).toBe('⎇ feat/tui-status-bar ⚠ untrusted')
    expect(branch.spans[1]!.tone).toBe('warning')
  })

  test('outside git the folder stands alone', () => {
    const text = render(input({ git: null }), 200)
    expect(text).toContain('│ ⌂ neuron-v2 │')
    expect(text).not.toContain('⎇')
  })

  test('signed out of Zero-X reads as a warning on the right', () => {
    const fitted = line(input({ signed_in: false, sign_in_required: true, session_id: '' }), 200)
    expect(fitted.right[0]!.spans[0]).toEqual({ text: 'Signed out: /login', tone: 'warning' })
  })

  test('another provider has no account group', () => {
    const fitted = line(input({ signed_in: false, sign_in_required: false, account: null }), 200)
    expect(fitted.right).toEqual([])
  })

  test('a model without thinking levels has no thinking segment', () => {
    const fitted = line(input({ reasoning_efforts: [] }), 200)
    expect(targets(fitted)).not.toContain('think')
  })

  test('an older engine: no breakdown, the old percentage clamped and marked ~', () => {
    const fitted = line(
      input({ context: undefined, context_percent: 999, provider: undefined, reasoning_efforts: undefined }),
      200,
    )
    const context = fitted.left.find((segment) => segment.target === 'context')!
    expect(context.spans.map((span) => span.text).join('')).toBe('●●●●●●●●●● ~100% ctx')
    expect(context.spans.at(-1)!.tone).toBe('error')
  })

  test('a locked model says so', () => {
    const text = render(input({ model: 'glm-5.2', model_display_name: null, provider: null }), 200)
    // Its maker's monogram (Zhipu's GLM), not the route's (Cloudflare).
    expect(text.startsWith('Z GLM 5.2 (locked) ▾')).toBe(true)
  })

  test('names leave out the gateway’s route; auto names the model that answered', () => {
    const routed = render(
      input({
        model: 'cohere/north-mini-code:free',
        provider: 'openrouter',
        model_display_name: 'North Mini Code (OpenRouter, free)',
      }),
      200,
    )
    expect(routed.startsWith('C North Mini Code ▾')).toBe(true)
    expect(routed).not.toMatch(/openrouter|:free|cohere\//i)

    // An older engine: the served model's catalog name, cleaned here.
    const models = {
      ...STATUS_BAR_MODELS,
      models: [
        ...STATUS_BAR_MODELS.models,
        {
          ...STATUS_BAR_MODELS.models[1]!,
          id: 'groq/openai/gpt-oss-120b',
          display_name: 'GPT-OSS 120B (Groq)',
          provider: 'groq',
        },
      ],
    }
    const auto = render(
      input(
        { model: 'auto', provider: 'zero-x', model_display_name: 'Auto (best available model)', served_model: 'groq/openai/gpt-oss-120b' },
        'unicode',
        { models },
      ),
      200,
    )
    expect(auto.startsWith('◇ Auto → GPT-OSS 120B ▾')).toBe(true)
    // A newer engine names it; without a catalog entry, the model part of the id.
    const named = render(
      input({ model: 'auto', served_model: 'x/glm-4.7-flash', served_model_display_name: 'GLM-4.7 Flash' }),
      200,
    )
    expect(named.startsWith('◇ Auto → GLM-4.7 Flash ▾')).toBe(true)
    const bare = render(input({ model: 'auto', served_model: 'cohere/north-mini-code:free' }), 200)
    expect(bare.startsWith('◇ Auto → north-mini-code ▾')).toBe(true)
  })

  test('a model whose name another catalog model has is numbered as in the picker', () => {
    const base = STATUS_BAR_MODELS.models[1]!
    const models = {
      ...STATUS_BAR_MODELS,
      models: [
        ...STATUS_BAR_MODELS.models,
        { ...base, id: 'groq/openai/gpt-oss-20b', display_name: 'GPT-OSS 20B (Groq)', provider: 'groq' },
        { ...base, id: 'nvidia/openai/gpt-oss-20b', display_name: 'GPT-OSS 20B (NVIDIA)', provider: 'nvidia' },
      ],
    }
    const served = (id: string) =>
      render(
        input(
          { model: 'auto', provider: 'zero-x', model_display_name: 'Auto', served_model: id, served_model_display_name: 'GPT-OSS 20B' },
          'unicode',
          { models },
        ),
        200,
      )
    expect(served('groq/openai/gpt-oss-20b').startsWith('◇ Auto → GPT-OSS 20B ▾')).toBe(true)
    expect(served('nvidia/openai/gpt-oss-20b').startsWith('◇ Auto → GPT-OSS 20B (2) ▾')).toBe(true)
    const picked = render(
      input(
        { model: 'nvidia/openai/gpt-oss-20b', provider: 'nvidia', model_display_name: 'GPT-OSS 20B' },
        'unicode',
        { models },
      ),
      200,
    )
    expect(picked).toContain('GPT-OSS 20B (2) ▾')
    expect(picked).not.toMatch(/nvidia|groq/i)
  })

  test('no part names the provider, only its monogram', () => {
    for (const provider of ['anthropic', 'openai', 'gemini', 'groq']) {
      const text = render(input({ provider, model_display_name: 'Model X' }), 200)
      expect(text).not.toMatch(/claude|anthropic|openai|google|groq/i)
    }
  })

  test('nothing before the first status', () => {
    expect(statusBarLine({ status: null, account: null, models: null, mode: 'unicode' }, 120)).toBeNull()
  })
})

describe('helpers', () => {
  test('monograms by who made the model, never the route to it', () => {
    expect(providerBadge('google', 'unicode')).toBe('G')
    expect(providerBadge('zero-x', 'unicode')).toBe('◇')
    expect(providerBadge('zero-x', 'console')).toBe('*')
    expect(providerBadge('someone', 'unicode')).toBe('•')
    expect(providerBadge(null, 'console')).toBe('.')
    // The routes that resell other makers' models have none.
    for (const route of ['groq', 'openrouter', 'cloudflare', 'workers-ai', 'nim']) {
      expect(providerBadge(route, 'unicode')).toBe('•')
    }
    expect(modelMaker('auto')).toBe('zero-x')
    expect(modelMaker('openai/gpt-5')).toBe('openai')
    expect(modelMaker('claude-sonnet-4-6')).toBe('anthropic')
    expect(modelMaker('@cf/meta/llama-3.3-70b-instruct-fp8-fast', 'cloudflare')).toBe('meta')
    expect(modelMaker('groq/openai/gpt-oss-120b', 'groq')).toBe('openai')
    expect(modelMaker('nvidia/stepfun-ai/step-5-preview', 'nvidia')).toBe('stepfun')
    expect(modelMaker('nvidia/nvidia/nemotron-3-ultra-550b-a55b', 'nvidia')).toBe('nvidia')
    expect(modelMaker('cohere/north-mini-code:free', 'openrouter')).toBe('cohere')
    expect(modelMaker('glm-5.2', 'cloudflare')).toBe('zhipu')
    expect(modelMaker('mystery-model', 'openrouter')).toBeNull()
    expect(modelMaker('mystery-model', 'acme')).toBe('acme')
  })

  test('thinking levels come from the status, then the catalog, then the default ladder', () => {
    const status = { ...STATUS_BAR_STATUS, reasoning_efforts: undefined }
    expect(reasoningEffortsOf(status, STATUS_BAR_MODELS.models[1]!)).toEqual(['none', 'low', 'medium', 'high'])
    expect(
      reasoningEffortsOf(status, { ...STATUS_BAR_MODELS.models[1]!, reasoning_efforts: null }),
    ).toEqual(['low', 'medium', 'high'])
    expect(reasoningEffortsOf(status, { ...STATUS_BAR_MODELS.models[1]!, supports_reasoning: false })).toBeNull()
    expect(effortLabel(null)).toBe('default')
    expect(effortLabel('none')).toBe('off')
  })

  test('the 5-hour window from tokens, or from what remains', () => {
    expect(usageWindowPercent(STATUS_BAR_ACCOUNT)).toBe(12)
    expect(
      usageWindowPercent({
        ...STATUS_BAR_ACCOUNT,
        usage_window: { tokens: null, limit_tokens: 1_000, remaining_tokens: 250, resets_at: null, window_seconds: null },
      }),
    ).toBe(75)
    expect(usageWindowPercent({ ...STATUS_BAR_ACCOUNT, usage_window: null })).toBeNull()
  })

  test('folder names from either kind of path', () => {
    expect(folderName('C:\\RAHUL\\neuron-v2')).toBe('neuron-v2')
    expect(folderName('/home/dev/app/')).toBe('app')
    expect(folderName('')).toBeNull()
  })
})
