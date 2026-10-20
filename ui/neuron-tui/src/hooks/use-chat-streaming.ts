// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
/**
 * Chat view adapter for connection status, queue presentation, and exit handling.
 *
 * Freebuff polled the Codebuff backend's health endpoint for `isConnected`.
 * Neuron's UI talks only to its local engine; until P2 reports the engine's
 * state, the UI counts as connected.
 */

import { useExitHandler } from './use-exit-handler'
import { useQueueControls } from './use-queue-controls'
import { useQueueUi } from './use-queue-ui'
import { useChatRuntime } from '../contexts/chat-runtime-context'
import { useChatStore } from '../state/chat-store'

import type { QueuedMessage, StreamStatus } from './use-message-queue'
import type { PendingAttachment } from '../types/store'
import type { MutableRefObject } from 'react'

export interface UseChatStreamingOptions {
  inputValue: string
  setInputValue: (value: {
    text: string
    cursorPosition: number
    lastEditDueToNav: boolean
  }) => void
  terminalWidth: number
  separatorWidth: number
}

export interface UseChatStreamingReturn {
  // Connection state
  isConnected: boolean
  showReconnectionMessage: boolean

  // Timer
  timerStartTime: number | null

  // Stream status
  streamStatus: StreamStatus
  isWaitingForResponse: boolean
  isStreaming: boolean

  // Queue management
  queuedMessages: QueuedMessage[]
  queuePaused: boolean
  streamMessageIdRef: MutableRefObject<string | null>
  addToQueue: (message: string, attachments?: PendingAttachment[]) => void
  setCanProcessQueue: (value: boolean | ((prev: boolean) => boolean)) => void
  clearQueue: () => QueuedMessage[]

  // Queue UI
  queuedCount: number
  shouldShowQueuePreview: boolean
  inputBoxTitle: string | undefined
  inputPlaceholder: string

  // Exit handling
  handleCtrlC: () => true
  ensureQueueActiveBeforeSubmit: () => boolean
  nextCtrlCWillExit: boolean
}

export function useChatStreaming({
  inputValue,
  setInputValue,
  terminalWidth,
  separatorWidth,
}: UseChatStreamingOptions): UseChatStreamingReturn {
  const runtime = useChatRuntime()
  const isConnected = true
  const showReconnectionMessage = false

  const {
    timerStartTime,
    queuedMessages,
    streamStatus,
    isWaitingForResponse,
    isStreaming,
    queuePaused,
    streamMessageIdRef,
    addToQueue,
    setCanProcessQueue,
    resumeQueue,
    clearQueue,
  } = runtime

  // A plain message sent now joins the running task.
  const isChainInProgress = useChatStore((state) => state.isChainInProgress)
  const steersRun = isStreaming || isChainInProgress

  // Queue UI
  const {
    queuedCount,
    shouldShowQueuePreview,
    inputBoxTitle,
    inputPlaceholder,
  } = useQueueUi({
    queuePaused,
    queuedMessages,
    separatorWidth,
    terminalWidth,
    steersRun,
  })

  // Exit handling
  const { handleCtrlC: baseHandleCtrlC, nextCtrlCWillExit } = useExitHandler({
    inputValue,
    setInputValue,
  })

  // Queue controls
  const { handleCtrlC, ensureQueueActiveBeforeSubmit } = useQueueControls({
    queuePaused,
    queuedCount,
    clearQueue,
    resumeQueue,
    inputHasText: Boolean(inputValue),
    baseHandleCtrlC,
  })

  return {
    // Connection state
    isConnected,
    showReconnectionMessage,

    // Timer
    timerStartTime,

    // Stream status
    streamStatus,
    isWaitingForResponse,
    isStreaming,

    // Queue management
    queuedMessages,
    queuePaused,
    streamMessageIdRef,
    addToQueue,
    setCanProcessQueue,
    clearQueue,

    // Queue UI
    queuedCount,
    shouldShowQueuePreview,
    inputBoxTitle,
    inputPlaceholder,

    // Exit handling
    handleCtrlC,
    ensureQueueActiveBeforeSubmit,
    nextCtrlCWillExit,
  }
}
