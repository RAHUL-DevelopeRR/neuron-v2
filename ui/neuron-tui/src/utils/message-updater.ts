import { hasIncompleteCodeFence, hasMarkdown, streamingMarkdownSplit } from './markdown-renderer'

import type { ChatMessage, ContentBlock, TextContentBlock } from '../types/chat'

// Small wrapper to avoid repeating the ai-message map/update pattern.
export type SetMessagesFn = (
  updater: (messages: ChatMessage[]) => ChatMessage[],
) => void

export type MessageUpdater = {
  updateAiMessage: (updater: (msg: ChatMessage) => ChatMessage) => void
  updateAiMessageBlocks: (
    blockUpdater: (blocks: ContentBlock[]) => ContentBlock[],
  ) => void
  markComplete: (metadata?: Partial<ChatMessage>) => void
  setError: (message: string) => void
  /** Clears the userError field (e.g., when a new message is sent successfully) */
  clearUserError: () => void
  addBlock: (block: ContentBlock) => void
}

/** Extended updater with batching control methods */
export type BatchedMessageUpdater = MessageUpdater & {
  /** Immediately flush all pending updates to state */
  flush: () => void
  /** Stop the flush interval and clean up. Called automatically by markComplete/setError */
  dispose: () => void
}

/**
 * Default interval for batched updates in milliseconds: about 30 updates a
 * second, so streamed text moves smoothly (at 100 ms it came in steps). Each
 * update draws only the streaming message, and of its text only what came
 * after the last finished block (see ContentWithMarkdown).
 */
export const DEFAULT_FLUSH_INTERVAL_MS = 33

/**
 * The interval while the text each update parses again is long: a list, a
 * table or a quote has no blank line to cut it at, so every update parses
 * all of it, and 30 times a second made a long list answer 2 to 4 times as
 * costly as 10 times a second. 100 ms is what every answer had in alpha.4.
 */
export const LONG_REPARSE_FLUSH_INTERVAL_MS = 100

/**
 * From how many characters of text parsed again per update the interval
 * grows. Paragraphs, headings and code are cut at their blank lines and stay
 * well under it; a list, table or quote passes it after about 170 words.
 */
export const LONG_REPARSE_CHARS = 1024

/**
 * How many characters of `message` each update parses again, as
 * ContentWithMarkdown draws it: of its last block when that is text (the one
 * streaming; earlier ones do not change, so they are not drawn again), the
 * part after its finished blocks (streamingMarkdownSplit) and before a code
 * fence still open (drawn as typed, not parsed). Text with no Markdown is
 * drawn as it is and costs nothing to parse.
 */
export function reparsedChars(message: ChatMessage | undefined): number {
  if (!message) return 0
  const blocks = message.blocks ?? []
  const last = blocks[blocks.length - 1]
  const content = !last ? message.content : last.type === 'text' ? last.content : ''
  if (!content || !hasMarkdown(content)) return 0
  const start = Math.max(0, streamingMarkdownSplit(content))
  const end = hasIncompleteCodeFence(content)
    ? Math.max(start, content.lastIndexOf('```'))
    : content.length
  return end - start
}

/** The time between two updates of `message` while it streams. */
export function streamingFlushIntervalMs(message: ChatMessage | undefined): number {
  return reparsedChars(message) >= LONG_REPARSE_CHARS
    ? LONG_REPARSE_FLUSH_INTERVAL_MS
    : DEFAULT_FLUSH_INTERVAL_MS
}

export const createMessageUpdater = (
  aiMessageId: string,
  setMessages: SetMessagesFn,
): MessageUpdater => {
  const updateAiMessage = (updater: (msg: ChatMessage) => ChatMessage) => {
    setMessages((prev) =>
      prev.map((msg) => (msg.id === aiMessageId ? updater(msg) : msg)),
    )
  }

  const updateAiMessageBlocks = (
    blockUpdater: (blocks: ContentBlock[]) => ContentBlock[],
  ) => {
    updateAiMessage((msg) => ({
      ...msg,
      blocks: blockUpdater(msg.blocks ?? []),
    }))
  }

  const addBlock = (block: ContentBlock) => {
    updateAiMessage((msg) => ({
      ...msg,
      blocks: [...(msg.blocks ?? []), block],
    }))
  }

  const markComplete = (metadata?: Partial<ChatMessage>) => {
    updateAiMessage((msg) => {
      const { metadata: messageMetadata, ...rest } = metadata ?? {}
      
      // Mark native reasoning blocks as complete by setting thinkingOpen = false
      // This ensures thinking blocks auto-collapse when the message finishes
      // Check for thinkingOpen !== false to handle both true (native) and undefined (legacy)
      const updatedBlocks = msg.blocks?.map((block) => {
        if (
          block.type === 'text' &&
          (block as TextContentBlock).textType === 'reasoning' &&
          (block as TextContentBlock).thinkingOpen !== false
        ) {
          return { ...block, thinkingOpen: false } as ContentBlock
        }
        return block
      })
      
      const nextMessage: ChatMessage = {
        ...msg,
        isComplete: true,
        ...(updatedBlocks && { blocks: updatedBlocks }),
        ...rest,
      }

      if (messageMetadata) {
        nextMessage.metadata = {
          ...(msg.metadata ?? {}),
          ...messageMetadata,
        }
      }

      return nextMessage
    })
  }

  const setError = (message: string) => {
    updateAiMessage((msg) => ({
      ...msg,
      userError: message,
      isComplete: true,
    }))
  }

  /**
   * Clears the userError field from the message.
   * Call this when starting a new successful interaction to dismiss any previous error banners.
   */
  const clearUserError = () => {
    updateAiMessage((msg) => {
      if (!msg.userError) return msg
      const { userError: _, ...rest } = msg
      return rest as ChatMessage
    })
  }

  return {
    updateAiMessage,
    updateAiMessageBlocks,
    markComplete,
    setError,
    clearUserError,
    addBlock,
  }
}

/**
 * Batched updater that queues updates and flushes at a regular interval.
 * Without `flushIntervalMs` the interval follows the message: every
 * DEFAULT_FLUSH_INTERVAL_MS, or every LONG_REPARSE_FLUSH_INTERVAL_MS while
 * each update parses much of its text again (streamingFlushIntervalMs).
 */
export const createBatchedMessageUpdater = (
  aiMessageId: string,
  setMessages: SetMessagesFn,
  flushIntervalMs?: number,
): BatchedMessageUpdater => {
  // Queue of message updater functions to be applied on next flush
  const pendingUpdaters: Array<(msg: ChatMessage) => ChatMessage> = []
  let intervalId: ReturnType<typeof setInterval> | null = null
  let isDisposed = false
  // For the adaptive interval: when the last flush was, and the time it wants
  // until the next one (from the message as it left it).
  let lastFlushAt = -Infinity
  let nextFlushGapMs = DEFAULT_FLUSH_INTERVAL_MS

  const flush = () => {
    if (pendingUpdaters.length === 0) return

    // Capture and clear the queue atomically
    const updaters = pendingUpdaters.splice(0, pendingUpdaters.length)

    // Compose all pending updaters into a single transform
    let flushed: ChatMessage | undefined
    const composedUpdater = (msg: ChatMessage): ChatMessage => {
      flushed = updaters.reduce((m, fn) => fn(m), msg)
      return flushed
    }

    // Apply composed update to the target message
    setMessages((prev) =>
      prev.map((msg) => (msg.id === aiMessageId ? composedUpdater(msg) : msg)),
    )
    lastFlushAt = Date.now()
    if (flushIntervalMs === undefined && flushed) {
      nextFlushGapMs = streamingFlushIntervalMs(flushed)
    }
  }

  const dispose = () => {
    if (isDisposed) return
    // Flush any pending updates before disposing to prevent data loss
    flush()
    isDisposed = true
    if (intervalId !== null) {
      clearInterval(intervalId)
      intervalId = null
    }
  }

  // Start the periodic flush interval. The adaptive one ticks at the short
  // interval and lets a tick pass while the message wants the long one (a
  // tick counts as on time within half a tick, as timers fire late).
  const tick =
    flushIntervalMs !== undefined
      ? flush
      : () => {
          const elapsed = Date.now() - lastFlushAt
          if (elapsed < nextFlushGapMs - DEFAULT_FLUSH_INTERVAL_MS / 2) return
          flush()
        }
  intervalId = setInterval(tick, flushIntervalMs ?? DEFAULT_FLUSH_INTERVAL_MS)

  const updateAiMessage = (updater: (msg: ChatMessage) => ChatMessage) => {
    if (isDisposed) {
      // If disposed, apply immediately as fallback
      setMessages((prev) =>
        prev.map((msg) => (msg.id === aiMessageId ? updater(msg) : msg)),
      )
      return
    }
    pendingUpdaters.push(updater)
  }

  const updateAiMessageBlocks = (
    blockUpdater: (blocks: ContentBlock[]) => ContentBlock[],
  ) => {
    updateAiMessage((msg) => ({
      ...msg,
      blocks: blockUpdater(msg.blocks ?? []),
    }))
  }

  const addBlock = (block: ContentBlock) => {
    updateAiMessage((msg) => ({
      ...msg,
      blocks: [...(msg.blocks ?? []), block],
    }))
  }

  const markComplete = (metadata?: Partial<ChatMessage>) => {
    // Flush any pending updates first
    flush()
    // Stop the interval
    dispose()

    // Apply completion immediately
    setMessages((prev) =>
      prev.map((msg) => {
        if (msg.id !== aiMessageId) return msg
        const { metadata: messageMetadata, ...rest } = metadata ?? {}
        
        // Mark native reasoning blocks as complete by setting thinkingOpen = false
        // This ensures thinking blocks auto-collapse when the message finishes
        // Check for thinkingOpen !== false to handle both true (native) and undefined (legacy)
        const updatedBlocks = msg.blocks?.map((block) => {
          if (
            block.type === 'text' &&
            (block as TextContentBlock).textType === 'reasoning' &&
            (block as TextContentBlock).thinkingOpen !== false
          ) {
            return { ...block, thinkingOpen: false } as ContentBlock
          }
          return block
        })
        
        const nextMessage: ChatMessage = {
          ...msg,
          isComplete: true,
          ...(updatedBlocks && { blocks: updatedBlocks }),
          ...rest,
        }
        if (messageMetadata) {
          nextMessage.metadata = {
            ...(msg.metadata ?? {}),
            ...messageMetadata,
          }
        }
        return nextMessage
      }),
    )
  }

  const setError = (message: string) => {
    // Flush any pending updates first so we don't lose streamed content
    flush()
    // Stop the interval
    dispose()

    // Apply error immediately while preserving existing content and blocks
    setMessages((prev) =>
      prev.map((msg) => {
        if (msg.id !== aiMessageId) return msg
        return {
          ...msg,
          userError: message,
          isComplete: true,
        }
      }),
    )
  }

  /**
   * Clears the userError field from the message immediately (bypasses batch queue).
   * Call this when starting a new successful interaction to dismiss any previous error banners.
   */
  const clearUserError = () => {
    // Apply immediately (bypass batch queue) so error banners are dismissed instantly
    setMessages((prev) =>
      prev.map((msg) => {
        if (msg.id !== aiMessageId || !msg.userError) return msg
        const { userError: _, ...rest } = msg
        return rest as ChatMessage
      }),
    )
  }

  return {
    updateAiMessage,
    updateAiMessageBlocks,
    markComplete,
    setError,
    clearUserError,
    addBlock,
    flush,
    dispose,
  }
}
