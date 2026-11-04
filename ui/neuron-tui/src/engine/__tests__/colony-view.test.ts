import { describe, expect, test } from 'bun:test'

import { applyColonyEvent, colonyEventHandler, renderColonyView } from '../colony-view'

import type { ColonyAgentInfo, UiEvent } from '../protocol'
import type { ChatMessage } from '../../types/chat'

const agent = (overrides: Partial<ColonyAgentInfo>): ColonyAgentInfo => ({
  id: 'worker-t1',
  role: 'worker',
  title: 'Add a.txt',
  model: 'worker-model',
  status: 'running',
  tokens_used: 1_200,
  token_budget: 100_000,
  branch: 'neuron/agent/colony-1-t1',
  note: null,
  ...overrides,
})

const run: UiEvent[] = [
  {
    type: 'colony_started',
    run_id: 'colony-1',
    task: 'Add files',
    workers: 2,
    token_cap: 300_000,
    tasks: [
      { id: 't1', title: 'Add a.txt', writes: true },
      { id: 't2', title: 'Read', writes: false },
    ],
  },
  { type: 'colony_agent', run_id: 'colony-1', agent: agent({}) },
  {
    type: 'colony_agent',
    run_id: 'colony-1',
    agent: agent({ id: 'worker-t2', title: 'Read', branch: null, tokens_used: 800 }),
  },
  { type: 'colony_agent', run_id: 'colony-1', agent: agent({ status: 'done', tokens_used: 4_000 }) },
  { type: 'colony_budget', run_id: 'colony-1', used: 4_800, limit: 300_000 },
]

describe('Colony live rows', () => {
  test('one row per agent, replaced as it changes, under the run totals', () => {
    const view = run.reduce(applyColonyEvent, null)
    expect(view?.agents.map((row) => [row.id, row.status, row.tokens_used])).toEqual([
      ['worker-t1', 'done', 4_000],
      ['worker-t2', 'running', 800],
    ])
    const text = renderColonyView(view!)
    expect(text.split('\n')[0]).toBe('Colony run · 2 subtasks · 2 at a time · 4,800 / 300,000 tokens')
    expect(text).toContain(
      '- worker · Add a.txt · done · 4,000 / 100,000 tokens · worker-model · neuron/agent/colony-1-t1',
    )
  })

  test('the end says how it went and where the changes are', () => {
    const view = [
      ...run,
      {
        type: 'colony_finished',
        run_id: 'colony-1',
        status: 'conflict',
        tokens_used: 5_000,
        token_cap: 300_000,
        merged: false,
        branch: 'neuron/agent/colony-1-synthesis',
        conflict: 'neuron/agent/colony-1-t2',
      } satisfies UiEvent,
    ].reduce(applyColonyEvent, null)
    const text = renderColonyView(view!)
    expect(text.startsWith('Colony run conflict')).toBe(true)
    expect(text).toContain('Merging stopped at a conflict in neuron/agent/colony-1-t2.')
    expect(text).toContain('The changes are on branch neuron/agent/colony-1-synthesis.')
  })

  test('other events change nothing', () => {
    expect(applyColonyEvent(null, { type: 'turn_finished' })).toBeNull()
  })

  test('the handler adds one message and keeps it current', () => {
    let messages: ChatMessage[] = []
    const handle = colonyEventHandler((update) => {
      messages = update(messages)
    })
    handle({ type: 'notice', message: 'hello', kind: null })
    expect(messages).toHaveLength(0)
    for (const event of run) handle(event)
    expect(messages).toHaveLength(1)
    expect(messages[0]?.id).toBe('colony-colony-1')
    expect(messages[0]?.content).toContain('4,800 / 300,000 tokens')
  })
})
