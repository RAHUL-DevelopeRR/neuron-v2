// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: an item
// keeps the engine's status (`in_progress` too) and its present-tense form.
// See ui/neuron-tui/PORTING.md.
import type { ChatMessage } from '../types/chat'

export type TodoStatus = 'pending' | 'in_progress' | 'completed'

export type TodoItem = {
  task: string
  completed: boolean
  /** The engine's status; absent from lists saved before it was kept. */
  status?: TodoStatus
  /** The item in the present tense ("Fixing the build"), while in progress. */
  activeForm?: string
}

export function parseTodos(input: unknown): TodoItem[] | null {
  if (!input || typeof input !== 'object' || !('todos' in input)) return null
  const { todos } = input
  if (!Array.isArray(todos)) return null
  return todos.every(
    (todo): todo is TodoItem =>
      todo !== null &&
      typeof todo === 'object' &&
      typeof todo.task === 'string' &&
      typeof todo.completed === 'boolean',
  )
    ? todos
    : null
}

/** An item's status, also for lists that only say whether it is done. */
export const todoStatusOf = (todo: TodoItem): TodoStatus =>
  todo.status ?? (todo.completed ? 'completed' : 'pending')

/** The engine's status word as a {@link TodoStatus} (unknown words are pending). */
export function toTodoStatus(value: unknown): TodoStatus {
  return value === 'completed' || value === 'in_progress' ? value : 'pending'
}

/** Each write replaces the whole list. Only inspect root tool blocks: a
 * subagent's own checklist must not replace the main agent's progress. */
export function getLatestTodos(messages: ChatMessage[]): TodoItem[] {
  for (let i = messages.length - 1; i >= 0; i--) {
    const message = messages[i]
    if (message.variant !== 'ai' || message.parentId) continue
    const blocks = message.blocks ?? []
    for (let j = blocks.length - 1; j >= 0; j--) {
      const block = blocks[j]
      if (block.type !== 'tool' || block.toolName !== 'write_todos') continue
      const todos = parseTodos(block.input)
      // An explicit empty list clears the old one; a malformed/partial input
      // does not. Restored chats use the same serialized blocks as live chats.
      if (todos !== null) return todos
    }
  }
  return []
}
