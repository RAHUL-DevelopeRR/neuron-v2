/**
 * Provider names are never shown: Groq, OpenRouter, NVIDIA, Workers AI and Z.ai name
 * the way the gateway reaches a model, not the model. The gateway's catalog carries
 * them in its display names ("GPT-OSS 120B (Groq)"), so every place the desktop names
 * a model (the status bar, the model chip and picker, the Settings list) cleans the
 * name first, and prefers the engine's own cleaned name for the model it is on.
 */
import { afterEach, describe, expect, test } from 'bun:test'
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import fs from 'fs'
import path from 'path'

import { cleanModelName, initialSession, reduceSession, type ModelInfo, type SessionState, type UiEvent } from '@neuron/client'

import { App } from '../src/App'
import { StatusBar } from '../src/components/Bars'
import { ModelPicker } from '../src/components/ModelPicker'
import { modelLabel, modelRowName } from '../src/modelNames'
import { FakeEngine, fakePlatform } from './fake-platform'

afterEach(() => {
  cleanup()
  try {
    localStorage.clear()
  } catch {
    // No storage: nothing to clear.
  }
})

/** The words the owner's rule keeps out of the product ("free" with them: a model is not named by its plan). */
const PROVIDER_WORDS = ['groq', 'openrouter', 'nvidia', 'workers ai', 'z.ai', 'zai', 'cloudflare', 'together', 'bedrock', 'novita', 'nebius', 'free']
/** Which of them `text` has, so a failure says which. */
const providerWordsIn = (text: string | null | undefined) => PROVIDER_WORDS.filter((word) => (text ?? '').toLowerCase().includes(word))

/** A catalog entry. Its tiers name plans ("pro"), which are not routes: the picker shows them. */
const entry = (id: string, display_name: string | null, provider: string | null, over: Partial<ModelInfo> = {}): ModelInfo => ({
  id,
  display_name,
  provider,
  available: true,
  unavailable_reason: null,
  context_window: 131_072,
  tiers: ['pro'],
  ...over,
})

/** Names as the real gateway catalog sends them, with how a person should read each. */
const CATALOG: ModelInfo[] = [
  entry('groq/openai/gpt-oss-120b', 'GPT-OSS 120B (Groq)', 'groq'),
  entry('openrouter/cohere/north-mini-code:free', 'North Mini Code (OpenRouter, free)', 'openrouter'),
  entry('@cf/meta/llama-3.3-70b-instruct-fp8-fast', 'Llama 3.3 70B (Workers AI)', 'cloudflare'),
  entry('nvidia/stepfun-ai/step-5-preview', 'Step 5 Preview (NVIDIA)', 'nvidia'),
  // No catalog name: the model part of the id, without its route and tag.
  entry('openrouter/z-ai/glm-4.7-flash:free', null, 'openrouter'),
  // Z.ai is a route whatever provider the catalog gives (the client cleans its spellings by name).
  entry('z-ai/glm-5.2', 'GLM 5.2 (Z.ai)', 'zhipu'),
]
const CATALOG_NAMES = ['GPT-OSS 120B', 'North Mini Code', 'Llama 3.3 70B', 'Step 5 Preview', 'glm-4.7-flash', 'GLM 5.2']

const readyEvent = { type: 'ready', protocol: 2, version: '1' } as UiEvent
const statusEvent = (over: Record<string, unknown> = {}): UiEvent =>
  ({
    type: 'status',
    model: 'auto',
    permission_mode: 'default',
    session_id: 's1',
    cwd: '/work/app',
    workspace_trusted: true,
    plan_mode: false,
    context_percent: 12,
    git: null,
    ...over,
  }) as UiEvent
const modelsEvent = (models: ModelInfo[], current = 'auto'): UiEvent => ({ type: 'models', current, models })

/** The session after `events`, as the reducer builds it. */
const sessionAfter = (...events: UiEvent[]): SessionState =>
  [readyEvent, ...events].reduce<SessionState>((state, event, index) => reduceSession(state, event, 1000 + index), initialSession())

/** The status bar for a session whose engine is on `status` and whose catalog is `models`. */
function statusBar(models: ModelInfo[], status: Record<string, unknown>) {
  const { container } = render(<StatusBar state={sessionAfter(modelsEvent(models), statusEvent(status))} onPane={() => {}} />)
  return {
    bar: container.querySelector('footer')?.textContent ?? '',
    model: container.querySelector('.model-tag')?.textContent ?? null,
    served: container.querySelector('.served-model')?.textContent ?? null,
  }
}

describe('the status bar names models without their route', () => {
  test("it shows the engine's own name for the model it is on, and not the catalog's raw one", () => {
    // alpha.5 sends the name cleaned; the catalog still has "(Groq)" on it.
    const { bar, model } = statusBar(CATALOG, { model: 'groq/openai/gpt-oss-120b', model_display_name: 'GPT-OSS 120B' })
    expect(model).toBe('GPT-OSS 120B')
    expect(providerWordsIn(bar)).toEqual([])
  })

  test("it cleans the catalog's name for an engine that sends none", () => {
    const { bar, model } = statusBar(CATALOG, { model: 'groq/openai/gpt-oss-120b' })
    expect(model).toBe('GPT-OSS 120B')
    expect(providerWordsIn(bar)).toEqual([])
  })

  test('it cleans a name an older engine sends with the route still on it', () => {
    // alpha.4 sent model_display_name as the catalog had it.
    const { bar, model } = statusBar([], {
      model: 'openrouter/cohere/north-mini-code:free',
      model_display_name: 'North Mini Code (OpenRouter, free)',
    })
    expect(model).toBe('North Mini Code')
    expect(providerWordsIn(bar)).toEqual([])
  })

  test('a parenthesis naming the provider the engine reports is a route, whatever the provider is called', () => {
    const { model } = statusBar([], { model: 'acme/fast', model_display_name: 'Fast (Acme)', provider: 'acme' })
    expect(model).toBe('Fast')
    cleanup()
    // Without that provider, "(Acme)" could be part of the model's name.
    expect(statusBar([], { model: 'acme/fast', model_display_name: 'Fast (Acme)' }).model).toBe('Fast (Acme)')
  })

  test('it finds the catalog entry of a family by the id the engine asks with', () => {
    // The gateway lists families (openai/gpt-oss-120b); the engine strips openai/ and may hold a route id.
    const families = [
      entry('openai/gpt-oss-120b', 'GPT-OSS 120B (Groq)', 'groq'),
      entry('cohere/north-mini-code', 'North Mini Code (OpenRouter, free)', 'openrouter'),
    ]
    for (const [asked, name] of [
      ['gpt-oss-120b', 'GPT-OSS 120B'],
      ['groq/openai/gpt-oss-120b', 'GPT-OSS 120B'],
      ['openrouter/cohere/north-mini-code:free', 'North Mini Code'],
    ]) {
      cleanup()
      const { bar, model } = statusBar(families, { model: asked })
      expect(model).toBe(name)
      expect(providerWordsIn(bar)).toEqual([])
    }
  })

  test('answered by: the engine\'s name for the served model, else the catalog\'s, else the id\'s model part', () => {
    const engineName = statusBar(CATALOG, {
      model: 'auto',
      served_model: 'groq/openai/gpt-oss-120b',
      served_model_display_name: 'GPT-OSS 120B',
    })
    expect(engineName.served).toBe('answered by GPT-OSS 120B')
    expect(providerWordsIn(engineName.bar)).toEqual([])
    cleanup()

    const catalogName = statusBar(CATALOG, { model: 'auto', served_model: '@cf/meta/llama-3.3-70b-instruct-fp8-fast' })
    expect(catalogName.served).toBe('answered by Llama 3.3 70B')
    expect(providerWordsIn(catalogName.bar)).toEqual([])
    cleanup()

    // An older engine's served name may still carry its route.
    const olderEngine = statusBar([], {
      model: 'auto',
      served_model: 'nvidia/stepfun-ai/step-5-preview',
      served_model_display_name: 'Step 5 Preview (NVIDIA)',
    })
    expect(olderEngine.served).toBe('answered by Step 5 Preview')
    expect(providerWordsIn(olderEngine.bar)).toEqual([])
    cleanup()

    const noCatalog = statusBar([], { model: 'auto', served_model: 'openrouter/z-ai/glm-4.7-flash:free' })
    expect(noCatalog.served).toBe('answered by glm-4.7-flash')
    expect(providerWordsIn(noCatalog.bar)).toEqual([])
  })

  test('the served model is still left out when it is the model picked, by another route', () => {
    const { served } = statusBar(CATALOG, { model: 'openai/gpt-oss-120b', served_model: 'groq/openai/gpt-oss-120b' })
    expect(served).toBeNull()
  })

  test('auto is Auto, whatever the catalog calls it', () => {
    const models = [entry('auto', 'Auto (best available model)', 'zero-x')]
    expect(statusBar(models, { model: 'auto' }).model).toBe('Auto')
    cleanup()
    expect(statusBar([], { model: 'auto' }).model).toBe('Auto')
  })

  test('two catalog models with one name are told apart by a number, as in the picker', () => {
    const twins = [entry('groq/openai/gpt-oss-20b', 'GPT-OSS 20B (Groq)', 'groq'), entry('nvidia/openai/gpt-oss-20b', 'GPT-OSS 20B (NVIDIA)', 'nvidia')]
    expect(statusBar(twins, { model: 'groq/openai/gpt-oss-20b' }).model).toBe('GPT-OSS 20B')
    cleanup()
    const second = statusBar(twins, { model: 'nvidia/openai/gpt-oss-20b' })
    expect(second.model).toBe('GPT-OSS 20B (2)')
    expect(providerWordsIn(second.bar)).toEqual([])
  })

  test('without a catalog or a name it is the id\'s model part, and nothing is shown before the first status', () => {
    expect(statusBar([], { model: 'nvidia/nvidia/nemotron-3-ultra-550b-a55b' }).model).toBe('nemotron-3-ultra-550b-a55b')
    cleanup()
    const { container } = render(<StatusBar state={initialSession()} onPane={() => {}} />)
    expect(container.querySelector('.model-tag')?.textContent).toBe('')
    expect(container.querySelector('.served-model')).toBeNull()
  })
})

function openPicker(models: ModelInfo[], props: Partial<React.ComponentProps<typeof ModelPicker>> = {}) {
  const view = render(<ModelPicker current="" models={models} disabled={false} onOpen={() => {}} onPick={() => {}} {...props} />)
  const chip = screen.getByTitle('Switch model')
  fireEvent.click(chip)
  const list = screen.getByRole('listbox', { name: 'Models' })
  return {
    view,
    chip,
    list,
    names: within(list)
      .getAllByRole('option')
      .map((option) => option.querySelector('span')?.textContent),
  }
}

describe('the model picker names models without their route', () => {
  test('every row shows the model, not the way the gateway reaches it', () => {
    const { names, list, chip } = openPicker(CATALOG)
    expect(names).toEqual(CATALOG_NAMES)
    expect(providerWordsIn(list.textContent)).toEqual([])
    expect(providerWordsIn(chip.textContent)).toEqual([])
  })

  test('picking a row still sends the model id, route and all', () => {
    const picked: string[] = []
    const { list } = openPicker(CATALOG, { onPick: (id) => picked.push(id) })
    fireEvent.click(within(list).getByRole('option', { name: /North Mini Code/ }))
    expect(picked).toEqual(['openrouter/cohere/north-mini-code:free'])
  })

  test('a route in any bracket, with or without a space, in a group with more, or from a provider the plain rule lacked, is off too', () => {
    const odd = [
      entry('a/llama', 'Llama 3 70B [Bedrock]', 'bedrock'),
      entry('b/mixtral', 'Mixtral 8x7B（Novita）', 'novita'),
      entry('c/qwen', 'Qwen3 235B (Preview, Nebius)', 'nebius'),
      entry('d/phi', 'Phi-4(Ollama)', 'ollama'),
      entry('e/glm', 'GLM 5.2\u{a0}[Z.ai]', 'zhipu'),
      entry('f/kimi', 'Kimi K2 (Together.ai)', 'together'),
      // Nothing left of a name that is only a route: the id's last segment.
      entry('g/only', '(Groq)', 'groq'),
    ]
    const { names, list, chip } = openPicker(odd)
    expect(names).toEqual(['Llama 3 70B', 'Mixtral 8x7B', 'Qwen3 235B (Preview)', 'Phi-4', 'GLM 5.2', 'Kimi K2', 'only'])
    expect(providerWordsIn(list.textContent)).toEqual([])
    expect(providerWordsIn(chip.textContent)).toEqual([])
    cleanup()
    // And the status bar, for an engine that still sends the catalog's own name.
    expect(statusBar(odd, { model: 'a/llama', model_display_name: 'Llama 3 70B [Bedrock]' }).model).toBe('Llama 3 70B')
    cleanup()
    expect(statusBar(odd, { model: 'auto', served_model: 'b/mixtral' }).served).toBe('answered by Mixtral 8x7B')
  })

  test('two rows with one name are told apart by a number, and not by the route', () => {
    const { names, list } = openPicker([
      entry('groq/openai/gpt-oss-20b', 'GPT-OSS 20B (Groq)', 'groq'),
      entry('gemini/gemini-2.5-flash', 'Gemini 2.5 Flash', 'gemini'),
      entry('nvidia/openai/gpt-oss-20b', 'GPT-OSS 20B (NVIDIA)', 'nvidia'),
    ])
    expect(names).toEqual(['GPT-OSS 20B', 'Gemini 2.5 Flash', 'GPT-OSS 20B (2)'])
    expect(providerWordsIn(list.textContent)).toEqual([])
  })

  test('the chip names the model the engine is on: its own cleaned name first, else the catalog\'s', () => {
    const own = render(<ModelPicker current="groq/openai/gpt-oss-120b" currentName="GPT-OSS 120B" models={[]} disabled={false} onOpen={() => {}} onPick={() => {}} />)
    expect(screen.getByTitle('Switch model').textContent).toBe('GPT-OSS 120B')
    own.unmount()

    const fromCatalog = render(<ModelPicker current="gpt-oss-120b" models={[entry('openai/gpt-oss-120b', 'GPT-OSS 120B (Groq)', 'groq')]} disabled={false} onOpen={() => {}} onPick={() => {}} />)
    expect(screen.getByTitle('Switch model').textContent).toBe('GPT-OSS 120B')
    fromCatalog.unmount()

    // An older engine's name with its route still on it.
    const older = render(<ModelPicker current="x/m" currentName="Fast Model (Groq)" models={[]} disabled={false} onOpen={() => {}} onPick={() => {}} />)
    expect(screen.getByTitle('Switch model').textContent).toBe('Fast Model')
    older.unmount()

    // And one whose route is the provider the engine reports.
    render(<ModelPicker current="acme/fast" currentName="Fast (Acme)" currentProvider="acme" models={[]} disabled={false} onOpen={() => {}} onPick={() => {}} />)
    expect(screen.getByTitle('Switch model').textContent).toBe('Fast')
  })

  test('the chip says "model" before the engine names one, and auto is Auto', () => {
    const none = render(<ModelPicker current="" models={[]} disabled={false} onOpen={() => {}} onPick={() => {}} />)
    expect(screen.getByTitle('Switch model').textContent).toBe('model')
    none.unmount()
    render(<ModelPicker current="auto" models={[entry('auto', 'Auto (best available model)', 'zero-x')]} disabled={false} onOpen={() => {}} onPick={() => {}} />)
    expect(screen.getByTitle('Switch model').textContent).toBe('Auto')
  })
})

describe('the model picker marks the model the engine is on', () => {
  // The gateway lists families (`openai/gpt-oss-120b`); the engine asks with the id `openai/` stripped
  // (`gpt-oss-120b`), and a saved session or setting can hold a route id (`groq/openai/gpt-oss-120b`).
  const families = [
    entry('openai/gpt-oss-120b', 'GPT-OSS 120B (Groq)', 'groq'),
    entry('openai/gpt-oss-20b', 'GPT-OSS 20B (Groq)', 'groq'),
    entry('cohere/north-mini-code', 'North Mini Code (OpenRouter, free)', 'openrouter'),
  ]

  /** The rows marked as current. `aria-selected` and the `current` class must always say the same. */
  function marked(list: HTMLElement): string[] {
    const rows = within(list).getAllByRole('option')
    for (const row of rows) expect(row.classList.contains('current')).toBe(row.getAttribute('aria-selected') === 'true')
    return rows.filter((row) => row.getAttribute('aria-selected') === 'true').map((row) => row.querySelector('span')?.textContent ?? '')
  }
  const markedFor = (current: string, models: ModelInfo[] = families) => marked(openPicker(models, { current }).list)

  test("by the catalog's own id, as before", () => {
    expect(markedFor('openai/gpt-oss-120b')).toEqual(['GPT-OSS 120B'])
    cleanup()
    expect(markedFor('cohere/north-mini-code')).toEqual(['North Mini Code'])
  })

  test('by the id the engine asks with, `openai/` stripped', () => {
    expect(markedFor('gpt-oss-120b')).toEqual(['GPT-OSS 120B'])
    cleanup()
    expect(markedFor('gpt-oss-20b')).toEqual(['GPT-OSS 20B'])
  })

  test('by a route id, which names the family it serves', () => {
    for (const [current, name] of [
      ['groq/openai/gpt-oss-120b', 'GPT-OSS 120B'],
      ['workers-ai/openai/gpt-oss-20b', 'GPT-OSS 20B'],
      ['openrouter/cohere/north-mini-code:free', 'North Mini Code'],
    ]) {
      cleanup()
      expect(markedFor(current!)).toEqual([name!])
    }
  })

  test('whatever the case of the id', () => {
    expect(markedFor('GPT-OSS-120B')).toEqual(['GPT-OSS 120B'])
  })

  test('one row, even when two rows have one name', () => {
    const twins = [entry('groq/openai/gpt-oss-20b', 'GPT-OSS 20B (Groq)', 'groq'), entry('nvidia/openai/gpt-oss-20b', 'GPT-OSS 20B (NVIDIA)', 'nvidia')]
    expect(markedFor('nvidia/openai/gpt-oss-20b', twins)).toEqual(['GPT-OSS 20B (2)'])
    cleanup()
    expect(markedFor('groq/openai/gpt-oss-20b', twins)).toEqual(['GPT-OSS 20B'])
  })

  test('no row, when the catalog does not have the model or the engine has not named one', () => {
    expect(markedFor('acme/made-up-model')).toEqual([])
    cleanup()
    expect(markedFor('groq/openai/gpt-oss-1200b')).toEqual([])
    cleanup()
    expect(markedFor('')).toEqual([])
  })
})

// Picking the model the engine is already on changes nothing, and the engine answers it by printing its
// whole catalog, ids and raw names with the routes, into the chat as a notice.
describe('the model picker and the model the engine is already on', () => {
  const families = [entry('openai/gpt-oss-120b', 'GPT-OSS 120B (Groq)', 'groq'), entry('openai/gpt-oss-20b', 'GPT-OSS 20B (Groq)', 'groq')]

  test('clicking the current row closes the menu and picks nothing, however the engine spells its id', () => {
    for (const current of ['openai/gpt-oss-120b', 'gpt-oss-120b', 'groq/openai/gpt-oss-120b', 'GPT-OSS-120B']) {
      const picked: string[] = []
      const { list } = openPicker(families, { current, onPick: (id) => picked.push(id) })
      fireEvent.click(within(list).getByRole('option', { name: /GPT-OSS 120B/ }))
      expect(picked).toEqual([])
      expect(screen.queryAllByRole('listbox').length).toBe(0)
      cleanup()
    }
  })

  test('clicking another row still picks it, by its id, and closes the menu', () => {
    const picked: string[] = []
    const { list } = openPicker(families, { current: 'gpt-oss-120b', onPick: (id) => picked.push(id) })
    fireEvent.click(within(list).getByRole('option', { name: /GPT-OSS 20B/ }))
    expect(picked).toEqual(['openai/gpt-oss-20b'])
    expect(screen.queryAllByRole('listbox').length).toBe(0)
  })

  test('with a current model the catalog lacks, or none named yet, every row picks', () => {
    for (const current of ['acme/made-up-model', '']) {
      const picked: string[] = []
      const { list } = openPicker(families, { current, onPick: (id) => picked.push(id) })
      fireEvent.click(within(list).getByRole('option', { name: /GPT-OSS 120B/ }))
      expect(picked).toEqual(['openai/gpt-oss-120b'])
      cleanup()
    }
  })
})

describe('the desktop app names models without their route', () => {
  async function start(statusOverrides: Record<string, unknown> = {}) {
    const engine = new FakeEngine(statusOverrides)
    render(<App platform={fakePlatform(engine)} />)
    await screen.findByLabelText('Message Neuron')
    return engine
  }
  const emit = (engine: FakeEngine, ...events: UiEvent[]) =>
    act(() => {
      for (const event of events) engine.emit(event)
    })
  const sidebar = () => screen.getByRole('navigation', { name: 'Neuron' })

  test("the chip and the status bar show the engine's name before the catalog is asked for", async () => {
    await start({ model: 'groq/openai/gpt-oss-120b', model_display_name: 'GPT-OSS 120B', provider: 'groq' })
    expect(screen.getByTitle('Switch model').textContent).toBe('GPT-OSS 120B')
    expect(document.querySelector('.model-tag')?.textContent).toBe('GPT-OSS 120B')
  })

  test('the chip takes the provider the engine reports, as the status bar does', async () => {
    await start({ model: 'acme/fast', model_display_name: 'Fast (Acme)', provider: 'acme' })
    expect(screen.getByTitle('Switch model').textContent).toBe('Fast')
    expect(document.querySelector('.model-tag')?.textContent).toBe('Fast')
  })

  test('the picker marks the family of the id the engine reports, which has no `openai/` in it', async () => {
    const engine = await start({ model: 'gpt-oss-120b' })
    fireEvent.click(screen.getByTitle('Switch model'))
    await emit(
      engine,
      modelsEvent([entry('openai/gpt-oss-120b', 'GPT-OSS 120B (Groq)', 'groq'), entry('openai/gpt-oss-20b', 'GPT-OSS 20B (Groq)', 'groq')], 'gpt-oss-120b'),
    )
    const rows = within(screen.getByRole('listbox', { name: 'Models' })).getAllByRole('option')
    expect(rows.map((row) => [row.querySelector('span')?.textContent, row.getAttribute('aria-selected')])).toEqual([
      ['GPT-OSS 120B', 'true'],
      ['GPT-OSS 20B', 'false'],
    ])
  })

  test('in the app, picking the model it is on sends nothing, and picking another sends set_model', async () => {
    const engine = await start({ model: 'gpt-oss-120b' })
    fireEvent.click(screen.getByTitle('Switch model'))
    await emit(
      engine,
      modelsEvent([entry('openai/gpt-oss-120b', 'GPT-OSS 120B (Groq)', 'groq'), entry('openai/gpt-oss-20b', 'GPT-OSS 20B (Groq)', 'groq')], 'gpt-oss-120b'),
    )
    const setModel = () => engine.commands().filter((command) => (command as { type?: string }).type === 'set_model')
    fireEvent.click(within(screen.getByRole('listbox', { name: 'Models' })).getByRole('option', { name: /GPT-OSS 120B/ }))
    expect(setModel()).toEqual([])
    expect(screen.queryAllByRole('listbox').length).toBe(0)

    fireEvent.click(screen.getByTitle('Switch model'))
    fireEvent.click(within(screen.getByRole('listbox', { name: 'Models' })).getByRole('option', { name: /GPT-OSS 20B/ }))
    expect(setModel()).toEqual([{ kind: 'command', type: 'set_model', model: 'openai/gpt-oss-20b' }])
  })

  test('the picker, once the catalog arrives, and the chip agree', async () => {
    const engine = await start({ model: 'groq/openai/gpt-oss-120b' })
    fireEvent.click(screen.getByTitle('Switch model'))
    await emit(engine, modelsEvent(CATALOG, 'groq/openai/gpt-oss-120b'))
    const list = screen.getByRole('listbox', { name: 'Models' })
    expect(
      within(list)
        .getAllByRole('option')
        .map((option) => option.querySelector('span')?.textContent),
    ).toEqual(CATALOG_NAMES)
    expect(screen.getByTitle('Switch model').textContent).toBe('GPT-OSS 120B')
    expect(document.querySelector('.model-tag')?.textContent).toBe('GPT-OSS 120B')
    expect(providerWordsIn(list.textContent)).toEqual([])
  })

  test('Settings lists models by name, and what it saves is still the id', async () => {
    const engine = await start()
    await emit(engine, modelsEvent(CATALOG))
    fireEvent.click(within(sidebar()).getByRole('button', { name: 'Settings' }))
    fireEvent.click(screen.getByRole('button', { name: 'Sessions' }))
    const options = [...document.querySelectorAll<HTMLOptionElement>('#model-ids option')]
    expect(options.map((option) => option.textContent)).toEqual(CATALOG_NAMES)
    expect(options.map((option) => option.getAttribute('value'))).toEqual(CATALOG.map((model) => model.id))
    expect(providerWordsIn(document.querySelector('.settings-content')?.textContent)).toEqual([])
  })

  test('Settings numbers two models with one name', async () => {
    const engine = await start()
    await emit(
      engine,
      modelsEvent([entry('groq/openai/gpt-oss-20b', 'GPT-OSS 20B (Groq)', 'groq'), entry('nvidia/openai/gpt-oss-20b', 'GPT-OSS 20B (NVIDIA)', 'nvidia')]),
    )
    fireEvent.click(within(sidebar()).getByRole('button', { name: 'Settings' }))
    fireEvent.click(screen.getByRole('button', { name: 'Sessions' }))
    expect([...document.querySelectorAll('#model-ids option')].map((option) => option.textContent)).toEqual(['GPT-OSS 20B', 'GPT-OSS 20B (2)'])
  })
})

// The names the Rust gateway catalog's own tests use (read only, never changed from here): the catalog entries
// the tests parse, and the names its `clean_model_name` test feeds in. Whatever route they carry, the owner's
// rule holds for every surface that names a model, so none of them may show a provider.
describe('the names in the Rust gateway catalog tests never reach the screen with a provider in them', () => {
  const RUST_CATALOG = path.resolve(import.meta.dir, '../../../rust/crates/rusty-claude-cli/src/gateway_catalog.rs')
  interface RustName {
    id: string
    name: string
    provider: string | null
  }

  function rustNames(): RustName[] {
    const source = fs.readFileSync(RUST_CATALOG, 'utf8')
    const found: RustName[] = []
    // The one-line entries of the catalogs the tests parse: {"id": "...", ..., "display_name": "...", "owned_by": "..."}.
    for (const line of source.split(/\r?\n/)) {
      const name = /"display_name"\s*:\s*"((?:[^"\\]|\\.)*)"/.exec(line)?.[1]
      if (name === undefined) continue
      found.push({
        id: /"id"\s*:\s*"([^"]*)"/.exec(line)?.[1] ?? 'x/model',
        name,
        provider: /"(?:owned_by|provider)"\s*:\s*"([^"]*)"/.exec(line)?.[1] ?? null,
      })
    }
    // The calls of the naming test: name("id", Some("display name"), Some("provider") or None).
    const call = /\bname\(\s*"([^"]*)"\s*,\s*Some\(\s*"((?:[^"\\]|\\.)*)"\s*\)\s*,\s*(?:Some\(\s*"([^"]*)"\s*\)|None)\s*\)/g
    for (const match of source.matchAll(call)) found.push({ id: match[1]!, name: match[2]!, provider: match[3] ?? null })
    return found
  }

  /** The words of the rule in `text`. NVIDIA counts only as a bracketed route: a maker's name in a model's own name stays. */
  const leaks = (text: string) => [
    ...['groq', 'openrouter', 'workers ai', 'z.ai', 'zai', 'cloudflare', 'together', 'bedrock', 'novita', 'nebius'].filter((word) =>
      text.toLowerCase().includes(word),
    ),
    ...(/[(\[（][^)\]）]*nvidia/i.test(text) ? ['nvidia (bracketed)'] : []),
  ]

  test('the names were found, so the check below is not empty', () => {
    const names = rustNames().map((found) => found.name)
    expect(names.length).toBeGreaterThanOrEqual(12)
    for (const known of [
      'GPT-OSS 120B (Groq)',
      'North Mini Code (OpenRouter, free)',
      'Llama 3.3 70B (Workers AI)',
      'Step 5 Preview (NVIDIA)',
      'Model (Preview, Groq)',
      '(Groq)',
    ]) {
      expect(names).toContain(known)
    }
  })

  test('cleanModelName, modelRowName and modelLabel show none of them, from the catalog or from an engine that sends the raw name', () => {
    for (const { id, name, provider } of rustNames()) {
      const model = entry(id, name, provider)
      const shown = {
        cleanModelName: cleanModelName(id, name, provider),
        modelRowName: modelRowName([model], model),
        'modelLabel (catalog)': modelLabel(id, [model]),
        'modelLabel (engine name)': modelLabel(id, [], { name, provider }),
      }
      for (const [how, text] of Object.entries(shown)) expect([name, how, leaks(text)]).toEqual([name, how, []])
    }
  })

  test('the checker finds a route when there is one, so a pass means something', () => {
    expect(leaks('GPT-OSS 120B (Groq)')).toEqual(['groq'])
    expect(leaks('Step 5 Preview (NVIDIA)')).toEqual(['nvidia (bracketed)'])
    expect(leaks('Step 5 Preview [NVIDIA, free]')).toEqual(['nvidia (bracketed)'])
    expect(leaks('NVIDIA Nemotron 3 Ultra')).toEqual([])
    expect(leaks('Model (Together.ai)')).toEqual(['together'])
  })
})
