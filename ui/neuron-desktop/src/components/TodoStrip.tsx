import { useState } from 'react'

import type { TodoItem } from '@neuron/client'

const MARK: Record<string, string> = { completed: '✓', in_progress: '▸' }

/** The agent's checklist as one line between the transcript and the composer; expands to every step. */
export function TodoStrip({ todos }: { todos: TodoItem[] }) {
  const [open, setOpen] = useState(false)
  if (todos.length === 0) return null
  const done = todos.filter((todo) => todo.status === 'completed').length
  const active = todos.find((todo) => todo.status === 'in_progress')
  const activeText = active ? active.active_form || active.content : ''
  return (
    <section className="todo-strip" aria-label="Tasks">
      <button className="todo-strip-head" aria-expanded={open} aria-controls="todo-strip-list" onClick={() => setOpen(!open)}>
        <span className="todo-strip-caret" aria-hidden>
          {open ? '▾' : '▸'}
        </span>
        <span className="todo-strip-count">
          {done}/{todos.length} done
        </span>
        {activeText && <span className="todo-strip-active"> · {activeText}</span>}
      </button>
      {open && (
        <ul id="todo-strip-list" className="todos todo-strip-list">
          {todos.map((todo, index) => (
            <li key={index} className={`todo-${todo.status}`}>
              {MARK[todo.status] ?? '○'} {todo.status === 'in_progress' ? todo.active_form || todo.content : todo.content}
            </li>
          ))}
        </ul>
      )}
    </section>
  )
}
