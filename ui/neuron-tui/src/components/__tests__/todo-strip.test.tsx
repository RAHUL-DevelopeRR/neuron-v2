import { afterEach, beforeAll, describe, expect, test } from 'bun:test'
import { createTestRenderer } from '@opentui/core/testing'
import { createRoot, flushSync } from '@opentui/react'
import React from 'react'

import { initializeThemeStore } from '../../hooks/use-theme'
import { resetLiveTurnStore, useLiveTurnStore } from '../../state/live-turn-store'
import { TodoStrip, todoStripSummary } from '../todo-strip'
import { WriteTodosItem } from '../tools/write-todos'

import type { TodoItem } from '../../utils/todos'

beforeAll(() => {
  initializeThemeStore()
})

afterEach(() => {
  resetLiveTurnStore()
})

const PLAN: TodoItem[] = [
  { task: 'Scaffold the shop', completed: true, status: 'completed', activeForm: 'Scaffolding the shop' },
  { task: 'Style the cart page', completed: false, status: 'in_progress', activeForm: 'Styling the cart page' },
  { task: 'Wire the checkout button', completed: false, status: 'pending' },
  { task: 'Run the tests', completed: false, status: 'pending' },
  { task: 'Write the README', completed: false, status: 'pending' },
]

async function frame(hidden = false, node?: React.ReactNode, height = 2): Promise<string> {
  const setup = await createTestRenderer({ width: 100, height })
  const root = createRoot(setup.renderer)
  flushSync(() => root.render(node ?? <TodoStrip hidden={hidden} />))
  try {
    await setup.renderOnce()
    return setup.captureCharFrame()
  } finally {
    flushSync(() => root.unmount())
    setup.renderer.destroy()
  }
}

describe('todo strip', () => {
  test('shows progress, the current item in its present tense, and what is next', async () => {
    useLiveTurnStore.setState({ active: true, todos: PLAN, todosThisTurn: true })
    const shown = await frame()
    expect(shown).toContain(
      'Todos 1/5 · ▸ Styling the cart page · next: Wire the checkout button',
    )
  })

  test('only while the running turn has a list', async () => {
    useLiveTurnStore.setState({ active: true, todos: PLAN, todosThisTurn: false })
    expect((await frame()).trim()).toBe('')
    useLiveTurnStore.setState({ active: false, todosThisTurn: true })
    expect((await frame()).trim()).toBe('')
    useLiveTurnStore.setState({ active: true, todos: [], todosThisTurn: true })
    expect((await frame()).trim()).toBe('')
    useLiveTurnStore.setState({ todos: PLAN })
    expect((await frame(true)).trim()).toBe('')
  })

  test('summaries: older lists without a status, and a finished list', () => {
    expect(
      todoStripSummary([
        { task: 'a', completed: true },
        { task: 'b', completed: false },
      ]),
    ).toEqual({ done: 1, total: 2, current: null, next: 'b' })
    expect(
      todoStripSummary(PLAN.map((todo) => ({ ...todo, completed: true, status: 'completed' }))),
    ).toEqual({ done: 5, total: 5, current: null, next: null })
    expect(todoStripSummary([{ task: 'Fix it', completed: false, status: 'in_progress' }]).current).toBe(
      'Fix it',
    )
  })

  test('a finished list says so', async () => {
    useLiveTurnStore.setState({
      active: true,
      todosThisTurn: true,
      todos: PLAN.map((todo) => ({ ...todo, completed: true, status: 'completed' as const })),
    })
    expect(await frame()).toContain('Todos 5/5 · ✓ all done')
  })

  test('the TODOs cell and /todo mark the item in progress', async () => {
    const shown = await frame(false, <WriteTodosItem todos={PLAN} />, 7)
    expect(shown).toContain('▸  Style the cart page')
    expect(shown).toContain('☐  Wire the checkout button')
  })
})
