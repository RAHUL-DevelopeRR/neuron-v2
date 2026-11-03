/**
 * One line above the input while the running turn has a task list:
 *
 *   Todos 2/5 · ▸ Styling the cart page · next: Wire the checkout button
 *
 * The item in progress reads in its present tense, highlighted. The whole
 * list stays one `/todo` away; the strip goes when the turn ends.
 */

import { TextAttributes } from '@opentui/core'
import React from 'react'

import { useTheme } from '../hooks/use-theme'
import { useLiveTurnStore } from '../state/live-turn-store'
import { GLYPHS } from '../utils/glyphs'
import { todoStatusOf, type TodoItem } from '../utils/todos'

export interface TodoStripSummary {
  done: number
  total: number
  /** The item in progress, in its present tense when it has one. */
  current: string | null
  /** The first item not started yet. */
  next: string | null
}

export function todoStripSummary(todos: TodoItem[]): TodoStripSummary {
  const current = todos.find((todo) => todoStatusOf(todo) === 'in_progress')
  const next = todos.find((todo) => todoStatusOf(todo) === 'pending')
  return {
    done: todos.filter((todo) => todoStatusOf(todo) === 'completed').length,
    total: todos.length,
    current: current ? (current.activeForm?.trim() || current.task) : null,
    next: next ? next.task : null,
  }
}

export const TodoStrip = ({ hidden = false }: { hidden?: boolean }) => {
  const theme = useTheme()
  const todos = useLiveTurnStore((state) => state.todos)
  const shown = useLiveTurnStore((state) => state.active && state.todosThisTurn)
  if (hidden || !shown || !todos || todos.length === 0) return null

  const { done, total, current, next } = todoStripSummary(todos)
  return (
    <box style={{ paddingLeft: 1, paddingRight: 1, flexShrink: 0, width: '100%' }}>
      <text style={{ wrapMode: 'none' }}>
        <span fg={theme.muted}>{`Todos ${done}/${total}`}</span>
        {current ? (
          <>
            <span fg={theme.muted}> · </span>
            <span fg={theme.primary}>{`${GLYPHS.todoActive} `}</span>
            <span fg={theme.foreground} attributes={TextAttributes.BOLD}>
              {current}
            </span>
          </>
        ) : null}
        {next ? <span fg={theme.muted}>{` · next: ${next}`}</span> : null}
        {done === total ? (
          <span fg={theme.success}>{` · ${GLYPHS.check} all done`}</span>
        ) : null}
      </text>
    </box>
  )
}
