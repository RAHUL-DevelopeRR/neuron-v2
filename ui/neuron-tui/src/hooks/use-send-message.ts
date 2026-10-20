// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
//
// Freebuff ran the agent here through the Codebuff SDK (`client.run`), with
// BYOK connection pinning, sponsored turns, Freebuff session admission and
// credit accounting around it. Neuron's engine runs the agent in another
// process, so a turn is now a `submit` to the engine client and a wait for its
// `turn_finished`. The UI half is unchanged: the user bubble, the AI message
// shell, the timer, interrupts, queue state and transcript checkpoints.

import { randomUUID } from 'node:crypto'

import { useCallback, useEffect, useRef } from 'react'

import {
  getNeuronEngineClient,
  runTurn,
  type NeuronEngineClient,
} from '../engine/neuron-engine-client'
import { setCurrentChatId } from '../project-files'
import { createStreamController } from './stream-state'
import { useChatStore } from '../state/chat-store'
import { createEventHandlerState } from '../utils/create-event-handler-state'
import { logger } from '../utils/logger'
import { clearActiveRun, registerActiveRun } from '../utils/active-run'
import {
  clearLiveChatStateProvider,
  loadMostRecentChatState,
  resolveCurrentChatDir,
  saveChatState,
  scheduleCheckpointSave,
  setLiveChatStateProvider,
  settleCheckpointSave,
} from '../utils/run-state-storage'
import {
  autoCollapsePreviousMessages,
  createAiMessageShell,
  createErrorMessage as createErrorChatMessage,
  generateAiMessageId,
  sanitizeRestoredMessages,
} from '../utils/send-message-helpers'
import { createSendMessageTimerController } from '../utils/send-message-timer'
import {
  handleRunCompletion,
  handleRunError,
  prepareUserMessage as prepareUserMessageHelper,
  resetEarlyReturnState,
  setupStreamingContext,
} from './helpers/send-message'
import { yieldToEventLoop } from '../utils/yield-to-event-loop'
import { STATE_SNAPSHOT_INTERRUPTION_MESSAGE } from '../types/agent-run'

import type { ElapsedTimeTracker } from './use-elapsed-time'
import type { StreamStatus } from './use-message-queue'
import type { PendingAttachment } from '../types/store'
import type { ChatMessage } from '../types/chat'
import type { SendMessageFn } from '../types/contracts/send-message'
import type { AgentMode } from '../utils/constants'
import type { SendMessageTimerEvent } from '../utils/send-message-timer'
import type { MessageContent, RunState } from '../types/agent-run'

interface UseSendMessageOptions {
  inputRef: React.MutableRefObject<any>
  activeSubagentsRef: React.MutableRefObject<Set<string>>
  isChainInProgressRef: React.MutableRefObject<boolean>
  setStreamStatus: (status: StreamStatus) => void
  setCanProcessQueue: (can: boolean) => void
  agentId?: string
  onBeforeMessageSend: () => Promise<{
    success: boolean
    errors: Array<{ id: string; message: string }>
  }>
  mainAgentTimer: ElapsedTimeTracker
  scrollToLatest: () => void
  onTimerEvent?: (event: SendMessageTimerEvent) => void
  isQueuePausedRef?: React.MutableRefObject<boolean>
  isProcessingQueueRef?: React.MutableRefObject<boolean>
  resumeQueue?: () => void
  /** Put a message back at the head of the queue. */
  requeueMessageAtFront?: (message: {
    content: string
    attachments: PendingAttachment[]
  }) => void
  /** Pause the queue. */
  pauseQueue?: () => void
  continueChat: boolean
  continueChatId?: string
  /** Dependency injection seam for run lifecycle tests. */
  getEngine?: () => NeuronEngineClient
}

// Respect bash context, but avoid sending empty prompts when only images are attached.
const buildPromptWithContext = (
  promptWithBashContext: string,
  messageContent: MessageContent[] | undefined,
) => {
  const trimmedPrompt = promptWithBashContext.trim()
  if (trimmedPrompt.length > 0) {
    return promptWithBashContext
  }

  if (messageContent && messageContent.length > 0) {
    return 'See attached image(s)'
  }

  return ''
}

export const useSendMessage = ({
  inputRef,
  activeSubagentsRef,
  isChainInProgressRef,
  setStreamStatus,
  setCanProcessQueue,
  agentId,
  onBeforeMessageSend,
  mainAgentTimer,
  scrollToLatest,
  onTimerEvent = () => {},
  isQueuePausedRef,
  isProcessingQueueRef,
  resumeQueue,
  continueChat,
  continueChatId,
  getEngine = getNeuronEngineClient,
}: UseSendMessageOptions): {
  sendMessage: SendMessageFn
  clearMessages: () => void
} => {
  // Pull setters directly from store - these are stable references that don't need
  // to trigger re-renders, so using getState() outside of callbacks is intentional.
  const {
    setMessages,
    setFocusedAgentId,
    setInputFocused,
    setStreamingAgents,
    setActiveSubagents,
    setIsChainInProgress,
    setHasReceivedPlanResponse,
    setLastMessageMode,
    setRunState,
    setIsRetrying,
  } = useChatStore.getState()
  const previousRunStateRef = useRef<RunState | null>(
    useChatStore.getState().runState,
  )
  // Memoize stream controller to maintain referential stability across renders
  const streamRefsRef = useRef<ReturnType<
    typeof createStreamController
  > | null>(null)
  if (!streamRefsRef.current) {
    streamRefsRef.current = createStreamController()
  }
  const streamRefs = streamRefsRef.current

  useEffect(() => {
    if (continueChat && !previousRunStateRef.current) {
      const loadedState = loadMostRecentChatState(continueChatId ?? undefined)
      if (loadedState) {
        previousRunStateRef.current = loadedState.runState
        setRunState(loadedState.runState)
        setMessages(sanitizeRestoredMessages(loadedState.messages))
        if (loadedState.chatId) {
          setCurrentChatId(loadedState.chatId)
        }
      }
    }
  }, [continueChat, continueChatId, setMessages, setRunState])

  const updateChainInProgress = useCallback(
    (value: boolean) => {
      isChainInProgressRef.current = value
      setIsChainInProgress(value)
    },
    [setIsChainInProgress, isChainInProgressRef],
  )

  const updateActiveSubagents = useCallback(
    (mutate: (next: Set<string>) => void) => {
      setActiveSubagents((prev) => {
        const next = new Set(prev)
        mutate(next)
        activeSubagentsRef.current = next
        return next
      })
    },
    [setActiveSubagents, activeSubagentsRef],
  )

  const addActiveSubagent = useCallback(
    (subagentId: string) => {
      updateActiveSubagents((next) => next.add(subagentId))
    },
    [updateActiveSubagents],
  )

  const removeActiveSubagent = useCallback(
    (subagentId: string) => {
      updateActiveSubagents((next) => next.delete(subagentId))
    },
    [updateActiveSubagents],
  )

  function clearMessages() {
    previousRunStateRef.current = null
    setRunState(null)
  }

  const prepareUserMessage = useCallback(
    (params: {
      content: string
      agentMode: AgentMode
      postUserMessage?: (prev: ChatMessage[]) => ChatMessage[]
      attachments?: PendingAttachment[]
      signal?: AbortSignal
    }) => {
      // Access lastMessageMode fresh each call to get current value
      const { lastMessageMode } = useChatStore.getState()
      return prepareUserMessageHelper({
        ...params,
        deps: {
          setMessages,
          lastMessageMode,
          setLastMessageMode,
          scrollToLatest,
          setHasReceivedPlanResponse,
        },
      })
    },
    [
      setMessages,
      setLastMessageMode,
      scrollToLatest,
      setHasReceivedPlanResponse,
    ],
  )

  const sendMessage = useCallback<SendMessageFn>(
    async ({ content, agentMode, postUserMessage, attachments }) => {
      // CRITICAL: Set chain in progress immediately (synchronously) before any async work.
      // This ensures the router can detect that we're busy and queue subsequent messages.
      isChainInProgressRef.current = true
      updateChainInProgress(true)
      setCanProcessQueue(false)

      if (agentMode !== 'PLAN') {
        setHasReceivedPlanResponse(false)
      }

      // Initialize timer for elapsed time tracking
      const timerController = createSendMessageTimerController({
        mainAgentTimer,
        onTimerEvent,
        agentId,
      })
      setIsRetrying(false)

      // Own cancellation before the first await, so an interrupt or context
      // switch during attachment processing or validation is honoured.
      const runOwnerId = randomUUID()
      const abortController = new AbortController()
      const runChatDir = resolveCurrentChatDir()
      const runChatIsCurrent = () => resolveCurrentChatDir() === runChatDir
      let latestRunStateSnapshot: RunState = previousRunStateRef.current ?? {
        traceSessionId: randomUUID(),
        output: {
          type: 'error',
          message: STATE_SNAPSHOT_INTERRUPTION_MESSAGE,
        },
      }
      let streamingStarted = false

      setLiveChatStateProvider(runOwnerId, () => ({
        runState: latestRunStateSnapshot,
        messages: useChatStore.getState().messages,
      }))

      const releaseRunOwnership = () => {
        clearLiveChatStateProvider(runOwnerId)
        clearActiveRun(runOwnerId)
      }

      registerActiveRun(runOwnerId, (reason) => {
        if (abortController.signal.aborted) return

        // Once streaming starts, setupStreamingContext's abort listener owns
        // message/timer cleanup. Preflight cancellation still needs to drop
        // the shared busy UI state synchronously.
        abortController.abort(reason)
        // The engine stops its own turn; it ignores an interrupt when none runs.
        if (streamingStarted) {
          try {
            getEngine().interrupt()
          } catch (error) {
            logger.debug({ error }, '[send-message] Engine interrupt failed')
          }
        } else {
          setIsRetrying(false)
          setStreamStatus('idle')
          setStreamingAgents(() => new Set())
          updateChainInProgress(false)
          if (isProcessingQueueRef) isProcessingQueueRef.current = false
        }

        // Capture the old chat's array now. Context-changing callers reset the
        // store immediately after stopActiveRun returns.
        scheduleCheckpointSave(
          latestRunStateSnapshot,
          useChatStore.getState().messages,
          runChatDir,
        )
      })

      const releaseIfStopped = (): boolean => {
        if (!abortController.signal.aborted) return false
        releaseRunOwnership()
        return true
      }

      const finishPreflight = () => {
        resetEarlyReturnState({
          setCanProcessQueue,
          updateChainInProgress,
          isProcessingQueueRef,
          isQueuePausedRef,
        })
        releaseRunOwnership()
      }

      // Prepare user message (bash context, images, text attachments, mode divider)
      let messageContent: MessageContent[] | undefined
      let bashContextForPrompt: string | undefined
      let finalContent: string
      let userMessageId: string

      try {
        const prepared = await prepareUserMessage({
          content,
          agentMode,
          postUserMessage,
          attachments,
          signal: abortController.signal,
        })
        userMessageId = prepared.userMessageId
        messageContent = prepared.messageContent
        bashContextForPrompt = prepared.bashContextForPrompt
        finalContent = prepared.finalContent
      } catch (error) {
        if (releaseIfStopped()) return
        logger.error(
          { error },
          '[send-message] prepareUserMessage failed with exception',
        )
        setMessages((prev) => [
          ...prev,
          createErrorChatMessage(
            '⚠️ Failed to prepare message. Please try again.',
          ),
        ])
        finishPreflight()
        return
      }

      if (releaseIfStopped()) return

      try {
        const validationResult = await onBeforeMessageSend()

        if (releaseIfStopped()) return

        if (!validationResult.success) {
          logger.warn(
            { errors: validationResult.errors },
            '[send-message] Validation failed',
          )
          setMessages((prev) =>
            prev.map((msg) =>
              msg.id === userMessageId
                ? { ...msg, validationErrors: validationResult.errors }
                : msg,
            ),
          )
          finishPreflight()
          return
        }
      } catch (error) {
        if (releaseIfStopped()) return
        logger.error(
          { error },
          '[send-message] Validation before message send failed with exception',
        )

        setMessages((prev) => [
          ...prev,
          createErrorChatMessage(
            '⚠️ Validation failed unexpectedly. Please try again.',
          ),
        ])
        await yieldToEventLoop()
        if (releaseIfStopped()) return
        setTimeout(() => scrollToLatest(), 0)

        finishPreflight()
        return
      }

      // Reset UI focus state
      setFocusedAgentId(null)
      setInputFocused(true)
      inputRef.current?.focus()

      // Create AI message shell and setup streaming context
      const aiMessageId = generateAiMessageId()
      const aiMessage = createAiMessageShell(aiMessageId)

      const { updater, hasReceivedContentRef } = setupStreamingContext({
        aiMessageId,
        timerController,
        setMessages,
        streamRefs,
        abortController,
        setStreamStatus,
        setCanProcessQueue,
        isQueuePausedRef,
        isProcessingQueueRef,
        updateChainInProgress,
        setIsRetrying,
        setStreamingAgents,
      })
      streamingStarted = true
      setStreamStatus('waiting')
      // Combine auto-collapse and AI message addition into single atomic update
      // to prevent flicker from intermediate render states
      setMessages((prev) => [
        ...autoCollapsePreviousMessages(prev, aiMessageId),
        aiMessage,
      ])

      // Checkpoint the turn to disk immediately so that killing the process
      // (closed terminal, crash) can't lose the user's prompt.
      saveChatState(
        latestRunStateSnapshot,
        useChatStore.getState().messages,
        runChatDir,
      )

      try {
        const promptWithBashContext = bashContextForPrompt
          ? bashContextForPrompt + finalContent
          : finalContent
        const effectivePrompt = buildPromptWithContext(
          promptWithBashContext,
          messageContent,
        )

        // The handler state the chat's renderers read from. P2 maps the
        // engine's UiEvents onto it (text and reasoning chunks, tool calls and
        // results, usage) in the onEvent callback below.
        const eventHandlerState = createEventHandlerState({
          isActive: () => !abortController.signal.aborted && runChatIsCurrent(),
          streamRefs,
          setStreamingAgents,
          setStreamStatus,
          aiMessageId,
          updater,
          hasReceivedContentRef,
          addActiveSubagent,
          removeActiveSubagent,
          agentMode,
          setHasReceivedPlanResponse,
          logger,
          setIsRetrying,
          onTotalCost: () => {},
        })

        logger.info(
          {
            promptLength: effectivePrompt.length,
            contentBlockCount: messageContent?.length ?? 0,
          },
          '[send-message] Submitting turn to the engine',
        )
        // P2: attachments go to the engine as Submit.attachments (file,
        // folder, image paths) instead of being inlined into the prompt.
        const outcome = await runTurn(
          getEngine(),
          { text: effectivePrompt },
          (_event) => {
            void eventHandlerState
          },
        )

        const runState: RunState = {
          ...latestRunStateSnapshot,
          output: outcome.error
            ? { type: 'error', message: outcome.error }
            : { type: 'lastMessage' },
        }
        latestRunStateSnapshot = runState

        // Only adopt and persist the result while this run's chat is still
        // the active one. After a mid-run chat switch (/new, resuming from
        // /history) the store's state belongs to the new conversation.
        if (!abortController.signal.aborted && runChatIsCurrent()) {
          previousRunStateRef.current = runState
          setRunState(runState)
          setIsRetrying(false)
          // Drop any queued/in-flight async checkpoint first so a stale write
          // can't land after this authoritative final save.
          await settleCheckpointSave()
        }
        handleRunCompletion({
          runState,
          agentMode,
          timerController,
          updater,
          aiMessageId,
          wasAbortedByUser: abortController.signal.aborted,
          hasReceivedContent: hasReceivedContentRef.current,
          setStreamStatus,
          setCanProcessQueue,
          updateChainInProgress,
          setHasReceivedPlanResponse,
          resumeQueue,
          isProcessingQueueRef,
          isQueuePausedRef,
        })
        if (!abortController.signal.aborted && runChatIsCurrent()) {
          saveChatState(runState, useChatStore.getState().messages, runChatDir)
        }
      } catch (error) {
        // If this run was aborted, the abort handler already handled cleanup.
        if (!abortController.signal.aborted) {
          handleRunError({
            error,
            timerController,
            updater,
            setIsRetrying,
            setStreamStatus,
            setCanProcessQueue,
            updateChainInProgress,
            isProcessingQueueRef,
            isQueuePausedRef,
          })
          // Persist the last checkpoint plus the error banner so a restart
          // after a failed run still shows this turn.
          if (runChatIsCurrent()) {
            await settleCheckpointSave()
            saveChatState(
              latestRunStateSnapshot,
              useChatStore.getState().messages,
              runChatDir,
            )
          }
        } else {
          logger.debug({ error }, '[send-message] Ignoring error after abort')
        }
      } finally {
        releaseRunOwnership()
        // If this run was aborted, the abort handler already released the chain lock
        // and queue processing state. Don't touch shared state here to avoid
        // interfering with any new run that may have started after the abort.
        if (!abortController.signal.aborted) {
          if (isChainInProgressRef.current) {
            logger.warn(
              {},
              '[send-message] Chain still in progress after try/catch, forcing reset',
            )
            updateChainInProgress(false)
            setStreamStatus('idle')
            setCanProcessQueue(!isQueuePausedRef?.current)
          }
          // Safety net: ensure lock is always released (idempotent).
          if (isProcessingQueueRef) {
            isProcessingQueueRef.current = false
          }
        }
        updater.dispose()
      }
    },
    [
      addActiveSubagent,
      agentId,
      getEngine,
      inputRef,
      isChainInProgressRef,
      isProcessingQueueRef,
      isQueuePausedRef,
      mainAgentTimer,
      onBeforeMessageSend,
      onTimerEvent,
      prepareUserMessage,
      removeActiveSubagent,
      resumeQueue,
      scrollToLatest,
      setCanProcessQueue,
      setFocusedAgentId,
      setHasReceivedPlanResponse,
      setInputFocused,
      setIsRetrying,
      setMessages,
      setRunState,
      setStreamStatus,
      setStreamingAgents,
      streamRefs,
      updateChainInProgress,
    ],
  )

  return {
    sendMessage,
    clearMessages,
  }
}
