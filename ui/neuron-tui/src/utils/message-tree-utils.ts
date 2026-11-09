import type { ChatMessage } from '../types/chat'

export function buildMessageTree(messages: ChatMessage[]): {
  tree: Map<string, ChatMessage[]>
  topLevelMessages: ChatMessage[]
} {
  const messageTree = new Map<string, ChatMessage[]>()
  const topLevelMessages: ChatMessage[] = []

  for (const message of messages) {
    if (message.parentId) {
      const siblings = messageTree.get(message.parentId) ?? []
      siblings.push(message)
      messageTree.set(message.parentId, siblings)
    } else {
      topLevelMessages.push(message)
    }
  }

  return { tree: messageTree, topLevelMessages }
}

const sameMessages = (a: readonly ChatMessage[], b: readonly ChatMessage[]): boolean =>
  a.length === b.length && a.every((message, index) => message === b[index])

/**
 * `next`, or `previous` when it holds the same child messages under the same
 * parents. Every streamed update builds the tree anew; the transcript's
 * messages read it, so an unchanged one keeps them from being drawn again.
 * A changed parent's list is new; the others keep their arrays.
 */
export function reuseMessageTree(
  previous: Map<string, ChatMessage[]> | null,
  next: Map<string, ChatMessage[]>,
): Map<string, ChatMessage[]> {
  if (!previous) return next
  let same = previous.size === next.size
  for (const [parentId, children] of next) {
    const before = previous.get(parentId)
    if (before && sameMessages(before, children)) next.set(parentId, before)
    else same = false
  }
  return same ? previous : next
}
