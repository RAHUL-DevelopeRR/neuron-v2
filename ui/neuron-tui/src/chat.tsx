// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the ads
// (display, inline, dock, partner rows), sponsored proposals and runs, the
// Freebuff session header/controls/footer, BYOK, Codebuff login, credits and
// subscription banners, feedback, publishing, the profile survey and every
// analytics event are gone. The engine's permission requests, sign-in and model
// picker open panels above the input, and its status shows in a footer line
// below it, as does a command's report. A submit sends the store's latest text.
// A running turn's task list shows in a strip above the input. The git views (/changes, the /commit card,
// the /rewind picker) take the input's place too, and the files the agent
// changed show in a line above it; a card or picker that came while the user
// was typing waits behind the input instead. See ui/neuron-tui/PORTING.md.
import { HelpBanner } from './components/help-banner'
import { ReportPanel, scrollOpenReport } from './components/report-panel'
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react'
import { useShallow } from 'zustand/react/shallow'

import { routeUserPrompt, addBashMessageToHistory } from './commands/router'
import { ChatInputBar } from './components/chat-input-bar'
import { ChatHeader } from './components/chat-header'
import { ChangesPanel, ChangesSummary } from './components/changes-panel'
import { CommitProposalCard } from './components/commit-proposal'
import { GitPanelWaiting } from './components/git-panel-waiting'
import { LoadPreviousButton } from './components/load-previous-button'
import { QueuePanel } from './components/queue-panel'
import { ReviewScreen } from './components/review-screen'
import { MessageWithAgents } from './components/message-with-agents'
import { PendingBashMessage } from './components/pending-bash-message'
import { EngineFooter } from './components/engine-footer'
import { ModelPicker } from './components/model-picker'
import { PermissionPrompt } from './components/permission-prompt'
import { RewindPicker } from './components/rewind-picker'
import { SignInPanel } from './components/sign-in-panel'
import { StatusBar } from './components/status-bar'
import {
  SuggestedPrompts,
  type SuggestedPromptSelection,
} from './components/suggested-prompts'
import { TodoPanel } from './components/todo-panel'
import { TodoStrip } from './components/todo-strip'
import { TopBanner } from './components/top-banner'
import { useChatRuntime } from './contexts/chat-runtime-context'
import { getSlashCommandsWithSkills } from './data/slash-commands'
import { useAskUserBridge } from './hooks/use-ask-user-bridge'
import { useChatInput } from './hooks/use-chat-input'
import {
  useChatKeyboard,
  type ChatKeyboardHandlers,
} from './hooks/use-chat-keyboard'
import { useChatMessages } from './hooks/use-chat-messages'
import { useChatState } from './hooks/use-chat-state'
import { useChatStreaming } from './hooks/use-chat-streaming'
import { useChatUI } from './hooks/use-chat-ui'
import { useClipboard } from './hooks/use-clipboard'
import { useEvent } from './hooks/use-event'
import { useInputHistory } from './hooks/use-input-history'
import { useSuggestionEngine } from './hooks/use-suggestion-engine'
import { getProjectRoot } from './project-files'
import { useChatHistoryStore } from './state/chat-history-store'
import { useChatStore } from './state/chat-store'
import { useEngineStore } from './state/engine-store'
import { isGitPanelOpen, useGitViewsStore } from './state/git-views-store'
import { useQueuePanelStore } from './state/queue-panel-store'
import { useReviewStore } from './state/review-store'
import { useMessageBlockStore } from './state/message-block-store'
import { stopActiveRun } from './utils/active-run'
import { showClipboardMessage } from './utils/clipboard'
import { readClipboardImage } from './utils/clipboard-image'
import { GLYPHS } from './utils/glyphs'
import { getSystemMessage } from './utils/message-history'
import { getInputModeConfig } from './utils/input-modes'
import {
  hasSubmittedFirstPrompt,
  markFirstPromptSubmitted,
} from './utils/settings'

import {
  type ChatKeyboardState,
  createDefaultChatKeyboardState,
} from './utils/keyboard-actions'
import { loadLocalAgents } from './utils/local-agent-registry'
import { logger } from './utils/logger'
import {
  addClipboardPlaceholder,
  addPendingFileFromPath,
  addPendingImageFromFile,
  validateAndAddImage,
} from './utils/pending-attachments'
import { getLoadedSkills } from './utils/skill-registry'
import { getStatusIndicatorState } from './utils/status-indicator-state'
import { createPasteHandler } from './utils/strings'
import { setTerminalTitle } from './utils/terminal-title'
import { computeInputLayoutMetrics } from './utils/text-layout'

import type { CommandResult } from './commands/command-registry'
import type { MultilineInputHandle } from './components/multiline-input'
import type { MatchedSlashCommand } from './hooks/use-suggestion-engine'
import type { AgentMode } from './utils/constants'
import type { FileTreeNode } from '@neuron-tui/common/util/file'
import type { BoxRenderable, ScrollBoxRenderable } from '@opentui/core'

export const Chat = ({
  consumeInitialPrompt,
  fileTree,
  inputRef,
  initialMode,
  gitRoot,
  onSwitchToGitRoot,
}: {
  consumeInitialPrompt: () => string | null
  fileTree: FileTreeNode[]
  inputRef: React.MutableRefObject<MultilineInputHandle | null>
  initialMode?: AgentMode
  gitRoot?: string | null
  onSwitchToGitRoot?: () => void
}) => {
  const [forceFileOnlyMentions, setForceFileOnlyMentions] = useState(false)
  const headerRef = useRef<BoxRenderable | null>(null)
  const [isHeaderVisible, setIsHeaderVisible] = useState(true)
  const [todoPanelOpen, setTodoPanelOpen] = useState(false)
  const [modelPickerOpen, setModelPickerOpen] = useState(false)
  const chatSessionId = useChatStore((state) => state.chatSessionId)
  useEffect(() => setTodoPanelOpen(false), [chatSessionId])

  // Freebuff's first-run starter prompts are off: they suggested Freebuff's
  // own tasks. The component stays for a Neuron set later.
  const [showSuggestedPrompts, setShowSuggestedPrompts] = useState(false)

  // Subscribe to ask_user bridge to trigger form display
  useAskUserBridge()

  // Get chat state from extracted hook
  const {
    inputValue,
    cursorPosition,
    lastEditDueToNav,
    setInputValue,
    inputFocused,
    setInputFocused,
    slashSelectedIndex,
    setSlashSelectedIndex,
    agentSelectedIndex,
    setAgentSelectedIndex,
    focusedAgentId,
    setFocusedAgentId,
    messages,
    setMessages,
    agentMode,
    setAgentMode,
    toggleAgentMode,
    isRetrying,
    pendingBashMessages,
  } = useChatState()

  const { statusMessage } = useClipboard()
  const {
    isChainInProgressRef,
    sendMessage,
    clearMessages,
    registerScrollToLatest,
    queuedMessages,
    editQueuedMessage,
    removeQueuedMessage,
    moveQueuedMessage,
  } = useChatRuntime()
  // Set initial mode from CLI flag on mount
  useEffect(() => {
    if (initialMode) {
      setAgentMode(initialMode)
    }
  }, [initialMode, setAgentMode])

  // Use extracted chat messages hook for message tree and pagination
  const {
    messageTree,
    visibleTopLevelMessages,
    hiddenMessageCount,
    handleCollapseToggle,
    isUserCollapsing,
    handleLoadPreviousMessages,
    handleToggleAll,
  } = useChatMessages({ messages, setMessages })

  // Use extracted UI hook for scroll, terminal dimensions, and theme
  const {
    scrollRef,
    scrollToLatest,
    scrollUp,
    scrollDown,
    appliedScrollboxProps,
    isAtBottom,
    hasOverflow,
    terminalWidth,
    terminalHeight,
    separatorWidth,
    messageAvailableWidth,
    isCompactHeight,
    isNarrowWidth,
    theme,
    markdownPalette,
  } = useChatUI({ messages, isUserCollapsing })

  useEffect(
    () => registerScrollToLatest(scrollToLatest),
    [registerScrollToLatest, scrollToLatest],
  )

  const updateHeaderVisibility = useCallback(() => {
    const header = headerRef.current
    const viewport = scrollRef.current?.viewport
    if (!header || !viewport) return

    const headerTop = header.screenY
    const headerBottom = headerTop + header.height
    const viewportTop = viewport.screenY
    const viewportBottom = viewportTop + viewport.height
    const visible = headerTop < viewportBottom && headerBottom > viewportTop
    setIsHeaderVisible((current) => (current === visible ? current : visible))
  }, [scrollRef])

  useEffect(() => {
    const scrollbox = scrollRef.current
    if (!scrollbox) return

    const timeoutId = setTimeout(updateHeaderVisibility, 0)
    scrollbox.verticalScrollBar.on('change', updateHeaderVisibility)
    return () => {
      clearTimeout(timeoutId)
      scrollbox.verticalScrollBar.off('change', updateHeaderVisibility)
    }
  }, [scrollRef, updateHeaderVisibility])

  useEffect(() => {
    const timeoutId = setTimeout(updateHeaderVisibility, 0)
    return () => clearTimeout(timeoutId)
  }, [messages, terminalHeight, terminalWidth, updateHeaderVisibility])

  const localAgents = useMemo(() => loadLocalAgents(), [])
  const inputMode = useChatStore((state) => state.inputMode)
  const setInputMode = useChatStore((state) => state.setInputMode)
  const askUserState = useChatStore((state) => state.askUserState)
  // A tool call waiting for approval takes the composer's place.
  const report = useEngineStore((state) => state.report)
  const permissionRequest = useEngineStore(
    (state) => state.permissionQueue[0] ?? null,
  )

  // The engine's git views: the changed files, and the /changes, /commit and
  // /rewind panels, which take the composer's place (state/git-views-store.ts).
  const gitViews = useGitViewsStore(
    useShallow((state) => ({
      features: state.features,
      changes: state.changes,
      changesPanelOpen: state.changesPanelOpen,
      proposal: state.proposal,
      rewind: state.rewind,
      deferred: state.deferred,
    })),
  )
  const gitPanelOpen = isGitPanelOpen(gitViews)

  // Get loaded skills for slash commands (and the git ones the engine has)
  const loadedSkills = useMemo(() => getLoadedSkills(), [])
  const filteredSlashCommands = useMemo(
    () => getSlashCommandsWithSkills(loadedSkills, gitViews.features),
    [loadedSkills, gitViews.features],
  )

  const {
    slashContext,
    mentionContext,
    slashMatches,
    agentMatches,
    fileMatches,
    slashSuggestionItems: slashCommandItems,
    agentSuggestionItems,
    fileSuggestionItems,
  } = useSuggestionEngine({
    disableAgentSuggestions: forceFileOnlyMentions || inputMode !== 'default',
    inputValue: inputMode === 'bash' ? '' : inputValue,
    cursorPosition,
    slashCommands: filteredSlashCommands,
    localAgents,
    fileTree,
    currentAgentMode: agentMode,
  })

  useEffect(() => {
    if (!mentionContext.active) {
      setForceFileOnlyMentions(false)
    }
  }, [mentionContext.active])

  // Reset suggestion menu indexes when context changes
  useEffect(() => {
    if (!slashContext.active) {
      setSlashSelectedIndex(0)
      return
    }
    setSlashSelectedIndex(0)
  }, [slashContext.active, slashContext.query, setSlashSelectedIndex])

  useEffect(() => {
    if (!mentionContext.active) {
      setAgentSelectedIndex(0)
      return
    }
    setAgentSelectedIndex(0)
  }, [mentionContext.active, mentionContext.query, setAgentSelectedIndex])

  useEffect(() => {
    const totalMatches = agentMatches.length + fileMatches.length
    if (totalMatches > 0 && agentSelectedIndex >= totalMatches) {
      setAgentSelectedIndex(totalMatches - 1)
    }
    if (totalMatches === 0 && agentSelectedIndex !== 0) {
      setAgentSelectedIndex(0)
    }
  }, [
    agentMatches.length,
    fileMatches.length,
    agentSelectedIndex,
    setAgentSelectedIndex,
  ])

  const openFileMenuWithTab = useCallback(() => {
    const safeCursor = Math.max(0, Math.min(cursorPosition, inputValue.length))

    let wordStart = safeCursor
    while (wordStart > 0 && !/\s/.test(inputValue[wordStart - 1])) {
      wordStart--
    }

    const before = inputValue.slice(0, wordStart)
    const wordAtCursor = inputValue.slice(wordStart, safeCursor)
    const after = inputValue.slice(safeCursor)
    const mentionWord = wordAtCursor.startsWith('@')
      ? wordAtCursor
      : `@${wordAtCursor}`

    const text = `${before}${mentionWord}${after}`
    const nextCursor = before.length + mentionWord.length

    setInputValue({
      text,
      cursorPosition: nextCursor,
      lastEditDueToNav: false,
    })
    setForceFileOnlyMentions(true)
  }, [cursorPosition, inputValue, setInputValue])

  const { saveToHistory, navigateUp, navigateDown, resetHistoryNavigation } =
    useInputHistory(inputValue, setInputValue, { inputMode, setInputMode })

  // Use extracted streaming hook for connection, timer, queue, and exit handling
  const {
    isConnected,
    showReconnectionMessage,
    timerStartTime,
    streamStatus,
    isWaitingForResponse,
    isStreaming,
    queuePaused,
    streamMessageIdRef,
    addToQueue,
    setCanProcessQueue,
    clearQueue,
    queuedCount,
    shouldShowQueuePreview,
    inputBoxTitle,
    inputPlaceholder,
    handleCtrlC,
    ensureQueueActiveBeforeSubmit,
    nextCtrlCWillExit,
  } = useChatStreaming({
    inputValue,
    setInputValue,
    terminalWidth,
    separatorWidth,
  })

  // When streaming completes, flush any pending bash commands into history (ghost mode only)
  // Non-ghost mode commands are already in history and will be cleared when user sends next message
  useEffect(() => {
    if (
      !isStreaming &&
      !streamMessageIdRef.current &&
      !isChainInProgressRef.current &&
      pendingBashMessages.length > 0
    ) {
      // Only flush ghost mode commands (those not already added to history) to UI
      const ghostModeMessages = pendingBashMessages.filter(
        (msg) => !msg.isRunning && !msg.addedToHistory,
      )

      // Add ghost mode messages to UI history
      for (const msg of ghostModeMessages) {
        addBashMessageToHistory({
          command: msg.command,
          stdout: msg.stdout,
          stderr: msg.stderr ?? null,
          exitCode: msg.exitCode,
          cwd: msg.cwd || process.cwd(),
          setMessages,
        })
      }

      // Mark ghost mode messages as added to history (so they don't show as ghost UI)
      // but keep them in pendingBashMessages so they get sent to LLM with next user message
      if (ghostModeMessages.length > 0) {
        const ghostIds = new Set(ghostModeMessages.map((m) => m.id))
        useChatStore.setState((state) => ({
          pendingBashMessages: state.pendingBashMessages.map((m) =>
            ghostIds.has(m.id) ? { ...m, addedToHistory: true } : m,
          ),
        }))
      }
    }
  }, [isStreaming, pendingBashMessages, setMessages])

  const onSubmitPrompt = useEvent(
    async (
      content: string,
      mode: AgentMode,
      options?: { preserveInputValue?: boolean },
    ) => {
      ensureQueueActiveBeforeSubmit()

      const preserveInput = options?.preserveInputValue === true
      const previousInputValue = preserveInput
        ? (() => {
            const {
              inputValue: text,
              cursorPosition,
              lastEditDueToNav,
            } = useChatStore.getState()
            return { text, cursorPosition, lastEditDueToNav }
          })()
        : null

      // Preserve attachments if needed (inline logic to avoid abstraction overhead)
      const preservedAttachments = preserveInput
        ? (() => {
            const items = useChatStore.getState().pendingAttachments
            if (items.length > 0) {
              useChatStore.getState().clearPendingAttachments()
              return [...items]
            }
            return null
          })()
        : null

      try {
        const result = await routeUserPrompt({
          agentMode: mode,
          inputRef,
          inputValue: content,
          isChainInProgressRef,
          isStreaming,
          streamMessageIdRef,
          addToQueue,
          hasQueuedMessages: () => queuedCount > 0,
          clearMessages,
          saveToHistory,
          scrollToLatest,
          sendMessage,
          setCanProcessQueue,
          setInputFocused,
          setInputValue,
          setMessages,
        })

        return result
      } finally {
        if (previousInputValue) {
          setInputValue({
            text: previousInputValue.text,
            cursorPosition: previousInputValue.cursorPosition,
            lastEditDueToNav: previousInputValue.lastEditDueToNav,
          })
        }

        // Restore attachments if they were preserved and none have been added since
        if (
          preservedAttachments &&
          useChatStore.getState().pendingAttachments.length === 0
        ) {
          useChatStore.setState((state) => {
            state.pendingAttachments = preservedAttachments
          })
        }
      }
    },
  )

  // Retire onboarding suggested prompts once the user submits anything
  // (typed or clicked), persisting so they don't return on future launches.
  useEffect(() => {
    if (showSuggestedPrompts && messages.length > 0) {
      markFirstPromptSubmitted()
      setShowSuggestedPrompts(false)
    }
  }, [showSuggestedPrompts, messages.length])

  // Submit a suggested onboarding prompt as if the user had typed and sent it
  const handleSelectSuggestedPrompt = useEvent(
    (prompt: string, _selection: SuggestedPromptSelection) => {
      onSubmitPrompt(prompt, agentMode).catch((error) => {
        logger.error({ error }, '[suggested-prompt] Failed to submit prompt')
        showClipboardMessage('Failed to send prompt', { durationMs: 3000 })
      })
    },
  )

  // Handle followup suggestion clicks
  useEffect(() => {
    const handleFollowupClick = (event: Event) => {
      const customEvent = event as CustomEvent<{
        prompt: string
        index: number
        toolCallId: string
      }>
      const { prompt, index, toolCallId } = customEvent.detail

      logger.info(
        { promptLength: prompt.length, index, toolCallId, agentMode },
        '[followup-click] Followup clicked',
      )

      // Mark this followup as clicked (persisted per toolCallId)
      useChatStore.getState().markFollowupClicked(toolCallId, index)

      // Send the followup prompt directly, preserving the user's current input
      onSubmitPrompt(prompt, agentMode, {
        preserveInputValue: true,
      })
        .then((result) => {
          logger.info(
            { hasResult: !!result },
            '[followup-click] onSubmitPrompt completed',
          )
        })
        .catch((error) => {
          logger.error(
            { error },
            '[followup-click] onSubmitPrompt failed with error',
          )
          showClipboardMessage('Failed to send followup', { durationMs: 3000 })
        })
    }

    globalThis.addEventListener('neuron:send-followup', handleFollowupClick)
    return () => {
      globalThis.removeEventListener(
        'neuron:send-followup',
        handleFollowupClick,
      )
    }
  }, [onSubmitPrompt, agentMode])

  // handleSlashItemClick is defined later, after the panel stores

  const handleMentionItemClick = useCallback(
    (index: number) => {
      if (mentionContext.startIndex < 0) return

      let replacement: string
      if (index < agentMatches.length) {
        const selected = agentMatches[index]
        if (!selected) return
        replacement = `@${selected.id} `
      } else {
        const fileIndex = index - agentMatches.length
        const selectedFile = fileMatches[fileIndex]
        if (!selectedFile) return
        replacement = `@${selectedFile.filePath} `
      }
      const before = inputValue.slice(0, mentionContext.startIndex)
      const after = inputValue.slice(
        mentionContext.startIndex + 1 + mentionContext.query.length,
      )
      setInputValue({
        text: before + replacement + after,
        cursorPosition: before.length + replacement.length,
        lastEditDueToNav: false,
      })
      setAgentSelectedIndex(0)
    },
    [
      mentionContext,
      agentMatches,
      fileMatches,
      inputValue,
      setInputValue,
      setAgentSelectedIndex,
    ],
  )

  const { inputWidth, handleBuildFast, handleBuildMax, handleBuildLite } =
    useChatInput({
      setInputValue,
      agentMode,
      setAgentMode,
      separatorWidth,
      consumeInitialPrompt,
      onSubmitPrompt,
      isCompactHeight,
      isNarrowWidth,
    })

  const { reviewMode, closeReviewScreen } = useReviewStore(
    useShallow((state) => ({
      reviewMode: state.reviewMode,
      closeReviewScreen: state.closeReviewScreen,
    })),
  )

  const { queuePanelOpen, openQueuePanel, closeQueuePanel } =
    useQueuePanelStore(
      useShallow((state) => ({
        queuePanelOpen: state.queuePanelOpen,
        openQueuePanel: state.openQueuePanel,
        closeQueuePanel: state.closeQueuePanel,
      })),
    )

  const slashSuggestionItems = slashCommandItems

  useEffect(() => {
    if (slashSuggestionItems.length > 0 && slashSelectedIndex >= slashSuggestionItems.length) {
      setSlashSelectedIndex(slashSuggestionItems.length - 1)
    }
    if (slashSuggestionItems.length === 0 && slashSelectedIndex !== 0) {
      setSlashSelectedIndex(0)
    }
  }, [slashSuggestionItems.length, slashSelectedIndex, setSlashSelectedIndex])

  // Review and ask_user take the composer's place too. Leaving the panel
  // flagged open behind them would keep chat's keyboard disabled with nothing
  // rendered to handle keys, so hand the surface back for real.
  useEffect(() => {
    if (queuePanelOpen && (reviewMode || askUserState !== null)) {
      closeQueuePanel()
    }
  }, [queuePanelOpen, reviewMode, askUserState, closeQueuePanel])

  // The panel store outlives this component. Without this, a remounted chat
  // would open onto a panel for a queue that no longer exists.
  useEffect(() => () => useQueuePanelStore.getState().closeQueuePanel(), [])

  // An approval, question or another panel takes priority over this read-only
  // view. Close it rather than leave its global keyboard listener active.
  useEffect(() => {
    if (reviewMode || askUserState !== null || queuePanelOpen) {
      setTodoPanelOpen(false)
    }
  }, [reviewMode, askUserState, queuePanelOpen])

  const handleCommandResult = useCallback(
    (result?: CommandResult) => {
      if (!result) return

      if (result.openChatHistory) {
        useChatHistoryStore.getState().openChatHistory()
      }

      if (result.openReviewScreen) {
        useReviewStore.getState().openReviewScreen()
      }

      if (result.openQueuePanel) {
        // The panel closes itself once the queue drains, so opening an empty
        // one would just flash. Say so instead.
        if (queuedCount > 0) useQueuePanelStore.getState().openQueuePanel()
        else
          setMessages((prev) => [...prev, getSystemMessage('Nothing queued.')])
      }

      if (result.openTodoPanel) setTodoPanelOpen(true)
      if (result.openModelPicker) setModelPickerOpen(true)
    },
    [queuedCount, setMessages],
  )

  // Helper to apply insertText for slash commands - returns true if handled
  const applySlashInsertText = useCallback(
    (selected: MatchedSlashCommand): boolean => {
      if (selected.insertText != null && slashContext.startIndex >= 0) {
        const before = inputValue.slice(0, slashContext.startIndex)
        const after = inputValue.slice(
          slashContext.startIndex + 1 + slashContext.query.length,
        )
        setInputValue({
          text: before + selected.insertText + after,
          cursorPosition: before.length + selected.insertText.length,
          lastEditDueToNav: false,
        })
        setSlashSelectedIndex(0)
        return true
      }
      return false
    },
    [slashContext, inputValue, setInputValue, setSlashSelectedIndex],
  )

  // Click handler for slash menu items - executes command or inserts text
  const handleSlashItemClick = useCallback(
    async (index: number) => {
      const item = slashSuggestionItems[index]
      if (item?.activate) {
        item.activate()
        return
      }
      const selected = slashMatches.find((command) => command.id === item?.id)
      if (!selected) return

      // If the command has insertText, insert it instead of executing
      if (applySlashInsertText(selected)) return

      // Execute the selected slash command immediately
      const commandString = `/${selected.id}`
      setSlashSelectedIndex(0)

      const result = await onSubmitPrompt(commandString, agentMode)
      handleCommandResult(result)
    },
    [
      slashMatches,
      slashSuggestionItems,
      applySlashInsertText,
      setSlashSelectedIndex,
      onSubmitPrompt,
      agentMode,
      handleCommandResult,
    ],
  )

  const inputValueRef = useRef(inputValue)
  const cursorPositionRef = useRef(cursorPosition)
  useEffect(() => {
    inputValueRef.current = inputValue
  }, [inputValue])

  useEffect(() => {
    cursorPositionRef.current = cursorPosition
  }, [cursorPosition])

  const handleReviewOptionSelect = useCallback(
    (reviewText: string) => {
      closeReviewScreen()
      setInputFocused(true)
      // Submit the review request
      onSubmitPrompt(reviewText, agentMode)
        .then((result) => handleCommandResult(result))
        .catch((error) => {
          logger.error({ error }, '[review] Failed to submit review prompt')
          showClipboardMessage('Failed to send review request', {
            durationMs: 3000,
          })
        })
    },
    [
      closeReviewScreen,
      setInputFocused,
      onSubmitPrompt,
      agentMode,
      handleCommandResult,
    ],
  )

  const handleCloseReviewScreen = useCallback(() => {
    closeReviewScreen()
    setInputFocused(true)
  }, [closeReviewScreen, setInputFocused])

  // The panel took the composer's place, so give the keyboard back to it the
  // same way the review screen does.
  const handleCloseQueuePanel = useCallback(() => {
    closeQueuePanel()
    setInputFocused(true)
    inputRef.current?.focus()
  }, [closeQueuePanel, setInputFocused, inputRef])

  // A git panel closes itself (a commit sent, a rewind done): the keyboard
  // goes back to the composer, as when the todo panel closes.
  const gitPanelWasOpen = useRef(false)
  useEffect(() => {
    if (gitPanelWasOpen.current && !gitPanelOpen) {
      setInputFocused(true)
      inputRef.current?.focus()
    }
    gitPanelWasOpen.current = gitPanelOpen
  }, [gitPanelOpen, setInputFocused, inputRef])

  const handleCloseTodoPanel = useCallback(() => {
    setTodoPanelOpen(false)
    setInputFocused(true)
    inputRef.current?.focus()
  }, [setInputFocused, inputRef])

  const handleReviewCustom = useCallback(() => {
    closeReviewScreen()
    setInputMode('review')
    setInputFocused(true)
  }, [closeReviewScreen, setInputMode, setInputFocused])

  // Ensure bracketed paste events target the active chat input
  useEffect(() => {
    if (!askUserState) {
      inputRef.current?.focus()
    }
  }, [askUserState, inputRef])

  const handleSubmit = useCallback(async () => {
    // The store's text, not this render's: an Enter typed right after the last
    // character can come before React renders it, and sent the prompt without it.
    const text = useChatStore.getState().inputValue
    // Update terminal title with truncated user input. A slash command
    // (`/login`) does not say what the chat is about.
    if (text.trim() && !text.trim().startsWith('/')) {
      setTerminalTitle(text)
    }
    const result = await onSubmitPrompt(text, agentMode)
    handleCommandResult(result)
  }, [onSubmitPrompt, agentMode, handleCommandResult])

  const totalMentionMatches = agentMatches.length + fileMatches.length
  const historyNavUpEnabled =
    lastEditDueToNav ||
    (cursorPosition === 0 &&
      ((slashContext.active && slashSelectedIndex === 0) ||
        (mentionContext.active && agentSelectedIndex === 0) ||
        (!slashContext.active && !mentionContext.active)))
  const historyNavDownEnabled =
    lastEditDueToNav ||
    (cursorPosition === inputValue.length &&
      ((slashContext.active &&
        slashSelectedIndex === slashSuggestionItems.length - 1) ||
        (mentionContext.active &&
          agentSelectedIndex === totalMentionMatches - 1) ||
        (!slashContext.active && !mentionContext.active)))

  // Build keyboard state from store values
  const chatKeyboardState: ChatKeyboardState = useMemo(
    () => ({
      ...createDefaultChatKeyboardState(),
      inputMode,
      inputValue,
      cursorPosition,
      isStreaming,
      isWaitingForResponse,
      focusedAgentId,
      slashMenuActive: slashContext.active,
      mentionMenuActive: mentionContext.active,
      slashSelectedIndex,
      agentSelectedIndex,
      slashMatchesLength: slashSuggestionItems.length,
      totalMentionMatches: agentMatches.length + fileMatches.length,
      disableSlashSuggestions:
        getInputModeConfig(inputMode).disableSlashSuggestions,
      historyNavUpEnabled,
      historyNavDownEnabled,
      nextCtrlCWillExit,
      queuePaused,
      queuedCount,
    }),
    [
      inputMode,
      inputValue,
      cursorPosition,
      isStreaming,
      isWaitingForResponse,
      focusedAgentId,
      slashContext.active,
      mentionContext.active,
      slashSelectedIndex,
      agentSelectedIndex,
      slashSuggestionItems.length,
      agentMatches.length,
      fileMatches.length,
      historyNavUpEnabled,
      historyNavDownEnabled,
      nextCtrlCWillExit,
      queuePaused,
      queuedCount,
    ],
  )

  // Keyboard handlers
  const chatKeyboardHandlers: ChatKeyboardHandlers = useMemo(
    () => ({
      onExitInputMode: () => setInputMode('default'),
      onClearInput: () =>
        setInputValue({ text: '', cursorPosition: 0, lastEditDueToNav: false }),
      onBackspaceExitMode: () => setInputMode('default'),
      onInterruptStream: () => {
        stopActiveRun('user-interrupt')
      },
      onSlashMenuDown: () => setSlashSelectedIndex((prev) => prev + 1),
      onSlashMenuUp: () => setSlashSelectedIndex((prev) => prev - 1),
      onSlashMenuSelect: async () => {
        const item = slashSuggestionItems[slashSelectedIndex]
        if (item?.activate) {
          item.activate()
          return
        }
        const selected = slashMatches.find((command) => command.id === item?.id)
        if (!selected) return

        // If the command has insertText, insert it instead of executing
        if (applySlashInsertText(selected)) return

        // Execute the selected slash command immediately
        const commandString = `/${selected.id}`
        setSlashSelectedIndex(0)

        const result = await onSubmitPrompt(commandString, agentMode)

        handleCommandResult(result)
      },
      onSlashMenuComplete: () => {
        // Complete the word without executing - same as clicking on the item
        const item = slashSuggestionItems[slashSelectedIndex]
        // Tab completes commands; only an explicit Enter or click opens an ad.
        if (item?.activate) return
        const selected = slashMatches.find((command) => command.id === item?.id)
        if (!selected || slashContext.startIndex < 0) return

        // If the command has insertText, insert it instead of the command
        if (applySlashInsertText(selected)) return

        const before = inputValue.slice(0, slashContext.startIndex)
        const after = inputValue.slice(
          slashContext.startIndex + 1 + slashContext.query.length,
        )
        const replacement = `/${selected.id} `
        setInputValue({
          text: before + replacement + after,
          cursorPosition: before.length + replacement.length,
          lastEditDueToNav: false,
        })
        setSlashSelectedIndex(0)
      },
      onMentionMenuDown: () => setAgentSelectedIndex((prev) => prev + 1),
      onMentionMenuUp: () => setAgentSelectedIndex((prev) => prev - 1),
      onMentionMenuTab: () => {
        const totalMatches = agentMatches.length + fileMatches.length
        setAgentSelectedIndex((prev) => (prev + 1) % totalMatches)
      },
      onMentionMenuShiftTab: () => {
        const totalMatches = agentMatches.length + fileMatches.length
        setAgentSelectedIndex(
          (prev) => (totalMatches + prev - 1) % totalMatches,
        )
      },
      onMentionMenuSelect: () => {
        if (mentionContext.startIndex < 0) return

        const trySelectAtIndex = (index: number): boolean => {
          let replacement: string
          if (index < agentMatches.length) {
            const selected = agentMatches[index]
            if (!selected) return false
            replacement = `@${selected.id} `
          } else {
            const fileIndex = index - agentMatches.length
            const selectedFile = fileMatches[fileIndex]
            if (!selectedFile) return false
            replacement = `@${selectedFile.filePath} `
          }
          const before = inputValue.slice(0, mentionContext.startIndex)
          const after = inputValue.slice(
            mentionContext.startIndex + 1 + mentionContext.query.length,
          )
          setInputValue({
            text: before + replacement + after,
            cursorPosition: before.length + replacement.length,
            lastEditDueToNav: false,
          })
          setAgentSelectedIndex(0)
          return true
        }

        // Try current selection, fall back to first item
        trySelectAtIndex(agentSelectedIndex) || trySelectAtIndex(0)
      },
      onMentionMenuComplete: () => {
        // Complete the word without executing - same as select for mentions
        if (mentionContext.startIndex < 0) return

        let replacement: string
        const index = agentSelectedIndex
        if (index < agentMatches.length) {
          const selected =
            agentMatches.length > 0
              ? agentMatches[index] || agentMatches[0]
              : undefined
          if (!selected) return
          replacement = `@${selected.id} `
        } else {
          const fileIndex = index - agentMatches.length
          const selectedFile =
            fileMatches.length > 0
              ? fileMatches[fileIndex] || fileMatches[0]
              : undefined
          if (!selectedFile) return
          replacement = `@${selectedFile.filePath} `
        }
        const before = inputValue.slice(0, mentionContext.startIndex)
        const after = inputValue.slice(
          mentionContext.startIndex + 1 + mentionContext.query.length,
        )
        setInputValue({
          text: before + replacement + after,
          cursorPosition: before.length + replacement.length,
          lastEditDueToNav: false,
        })
        setAgentSelectedIndex(0)
      },
      onOpenFileMenuWithTab: () => {
        const safeCursor = Math.max(
          0,
          Math.min(cursorPosition, inputValue.length),
        )
        let wordStart = safeCursor
        while (wordStart > 0 && !/\s/.test(inputValue[wordStart - 1]!)) {
          wordStart--
        }
        if (wordStart < safeCursor) {
          openFileMenuWithTab()
          return true
        }
        return false
      },
      onShowHelp: () => useChatStore.getState().setInputMode('help'),
      onOpenChatHistory: () => useChatHistoryStore.getState().openChatHistory(),
      onHistoryUp: navigateUp,
      onHistoryDown: navigateDown,
      onToggleAgentMode: toggleAgentMode,
      onUnfocusAgent: () => {
        setFocusedAgentId(null)
        setInputFocused(true)
        inputRef.current?.focus()
      },
      onClearQueue: clearQueue,
      onOpenQueuePanel: openQueuePanel,
      onExitAppWarning: () => handleCtrlC(),
      onExitApp: () => handleCtrlC(),
      onBashHistoryUp: navigateUp,
      onBashHistoryDown: navigateDown,
      onPasteImage: () => {
        const placeholderPath = addClipboardPlaceholder()

        // Process the image in the background
        setTimeout(() => {
          const result = readClipboardImage()
          if (!result.success || !result.imagePath) {
            useChatStore.getState().removePendingImage(placeholderPath)
            showClipboardMessage(result.error || 'Failed to paste image', {
              durationMs: 3000,
            })
            return
          }

          const cwd = getProjectRoot() ?? process.cwd()
          addPendingImageFromFile(result.imagePath, cwd, placeholderPath).catch(
            (error) => {
              logger.error({ error }, 'Failed to add pending image from file')
              showClipboardMessage('Failed to add image', { durationMs: 3000 })
            },
          )
        }, 0)
      },
      onPasteImagePath: (imagePath: string) => {
        const cwd = getProjectRoot() ?? process.cwd()
        validateAndAddImage(imagePath, cwd).catch((error) => {
          logger.error({ error, imagePath }, 'Failed to validate and add image')
          showClipboardMessage('Failed to add image', { durationMs: 3000 })
        })
      },
      onPasteFilePath: (filePath: string, isDirectory: boolean) => {
        addPendingFileFromPath(filePath, isDirectory)
      },
      onPasteText: (text: string) => {
        setInputValue((prev) => {
          const before = prev.text.slice(0, prev.cursorPosition)
          const after = prev.text.slice(prev.cursorPosition)
          return {
            text: before + text + after,
            cursorPosition: before.length + text.length,
            lastEditDueToNav: false,
          }
        })
      },
      // An open report scrolls instead of the chat.
      onScrollUp: () => {
        if (!scrollOpenReport('up')) scrollUp()
      },
      onScrollDown: () => {
        if (!scrollOpenReport('down')) scrollDown()
      },
      onToggleAll: handleToggleAll,
    }),
    [
      setInputMode,
      setInputValue,
      setSlashSelectedIndex,
      slashMatches,
      slashSuggestionItems,
      slashSelectedIndex,
      slashContext,
      inputValue,
      applySlashInsertText,
      onSubmitPrompt,
      agentMode,
      handleCommandResult,
      setAgentSelectedIndex,
      agentMatches,
      fileMatches,
      agentSelectedIndex,
      mentionContext,
      cursorPosition,
      openFileMenuWithTab,
      navigateUp,
      navigateDown,
      toggleAgentMode,
      setFocusedAgentId,
      setInputFocused,
      inputRef,
      handleCtrlC,
      clearQueue,
      openQueuePanel,
      scrollUp,
      scrollDown,
      handleToggleAll,
    ],
  )

  // Use the chat keyboard hook
  useChatKeyboard({
    state: chatKeyboardState,
    handlers: chatKeyboardHandlers,
    // `useKeyboard` is a GLOBAL listener, so a surface that takes over the
    // keyboard turns chat's handling off while it is up.
    disabled:
      askUserState !== null ||
      permissionRequest !== null ||
      modelPickerOpen ||
      reviewMode ||
      queuePanelOpen ||
      todoPanelOpen ||
      gitPanelOpen,
  })

  // Sync message block context to zustand store for child components
  const setMessageBlockContext = useMessageBlockStore(
    (state) => state.setContext,
  )
  const setMessageBlockCallbacks = useMessageBlockStore(
    (state) => state.setCallbacks,
  )

  // Update context when values change - useLayoutEffect ensures synchronous updates
  // to prevent message loss during rapid streaming (race condition fix)
  useLayoutEffect(() => {
    setMessageBlockContext({
      readOnly: false,
      theme,
      markdownPalette,
      messageTree,
      isWaitingForResponse,
      timerStartTime,
      availableWidth: messageAvailableWidth,
    })
  }, [
    theme,
    markdownPalette,
    messageTree,
    isWaitingForResponse,
    timerStartTime,
    messageAvailableWidth,
    setMessageBlockContext,
  ])

  // Update callbacks once (they're stable)
  useEffect(() => {
    setMessageBlockCallbacks({
      onToggleCollapsed: handleCollapseToggle,
      onBuildFast: handleBuildFast,
      onBuildMax: handleBuildMax,
      onBuildLite: handleBuildLite,
    })
  }, [
    handleCollapseToggle,
    handleBuildFast,
    handleBuildMax,
    handleBuildLite,
    setMessageBlockCallbacks,
  ])

  const modeConfig = getInputModeConfig(inputMode)
  const hasSlashSuggestions =
    slashContext.active &&
    slashSuggestionItems.length > 0 &&
    !modeConfig.disableSlashSuggestions
  const hasMentionSuggestions =
    !slashContext.active &&
    mentionContext.active &&
    (agentSuggestionItems.length > 0 || fileSuggestionItems.length > 0)
  const hasSuggestionMenu = hasSlashSuggestions || hasMentionSuggestions

  // Show first-time onboarding starter prompts only on a pristine, idle,
  // empty-input default-mode chat — and never while a menu/overlay is up.
  const showOnboardingPrompts =
    showSuggestedPrompts &&
    messages.length === 0 &&
    inputValue.length === 0 &&
    inputMode === 'default' &&
    !hasSuggestionMenu &&
    !isStreaming &&
    !isWaitingForResponse &&
    !reviewMode &&
    askUserState === null

  const inputLayoutMetrics = useMemo(() => {
    // In bash mode, layout is based on the actual input (no ! prefix needed)
    const text = inputValue ?? ''
    const layoutContent = text.length > 0 ? text : ' '
    const safeCursor = Math.max(
      0,
      Math.min(cursorPosition, layoutContent.length),
    )
    const cursorProbe =
      safeCursor >= layoutContent.length
        ? layoutContent
        : layoutContent.slice(0, safeCursor)
    const cols = Math.max(1, inputWidth)
    return computeInputLayoutMetrics({
      layoutContent,
      cursorProbe,
      cols,
      maxHeight: Math.floor(terminalHeight / 2),
    })
  }, [inputValue, cursorPosition, inputWidth, terminalHeight])
  const isMultilineInput = inputLayoutMetrics.heightLines > 1
  const shouldCenterInputVertically = !hasSuggestionMenu && !isMultilineInput
  const statusIndicatorState = getStatusIndicatorState({
    statusMessage,
    streamStatus,
    nextCtrlCWillExit,
    isConnected,
    showReconnectionMessage,
    isRetrying,
    isAskUserActive: askUserState !== null || permissionRequest !== null,
  })
  const hasStatusIndicatorContent = statusIndicatorState.kind !== 'idle'

  const shouldShowStatusLine =
    hasStatusIndicatorContent || shouldShowQueuePreview || !isAtBottom

  return (
    <box
      style={{
        flexDirection: 'column',
        gap: 0,
        flexGrow: 1,
      }}
    >
      <scrollbox
        ref={scrollRef as React.Ref<ScrollBoxRenderable>}
        stickyScroll
        stickyStart="bottom"
        scrollX={false}
        scrollbarOptions={{ visible: false }}
        verticalScrollbarOptions={{
          visible: !isStreaming && !isWaitingForResponse && hasOverflow,
          trackOptions: { width: 1 },
        }}
        {...appliedScrollboxProps}
        style={{
          flexGrow: 1,
          rootOptions: {
            flexGrow: 1,
            padding: 0,
            gap: 0,
            flexDirection: 'row',
            shouldFill: true,
            backgroundColor: 'transparent',
          },
          wrapperOptions: {
            flexGrow: 1,
            border: false,
            shouldFill: true,
            backgroundColor: 'transparent',
            flexDirection: 'column',
          },
          contentOptions: {
            flexDirection: 'column',
            gap: 0,
            shouldFill: true,
            justifyContent: 'flex-end',
            backgroundColor: 'transparent',
            paddingLeft: 1,
            paddingRight: 2,
          },
        }}
      >
        <TopBanner gitRoot={gitRoot} onSwitchToGitRoot={onSwitchToGitRoot} />

        <box
          ref={headerRef as React.Ref<BoxRenderable>}
          style={{ flexDirection: 'column' }}
        >
          <ChatHeader
            projectRoot={getProjectRoot()}
            animationEnabled={isHeaderVisible && inputFocused}
          />
        </box>
        {hiddenMessageCount > 0 && (
          <LoadPreviousButton
            hiddenCount={hiddenMessageCount}
            onLoadMore={handleLoadPreviousMessages}
          />
        )}
        {visibleTopLevelMessages.map((message, idx) => (
          <MessageWithAgents
            key={message.id}
            message={message}
            depth={0}
            isLastMessage={idx === visibleTopLevelMessages.length - 1}
            availableWidth={messageAvailableWidth}
          />
        ))}
        {/* Pending bash messages as ghost messages (only show those not already in history) */}
        {pendingBashMessages
          .filter((msg) => !msg.addedToHistory)
          .map((msg) => (
            <PendingBashMessage key={`pending-bash-${msg.id}`} message={msg} />
          ))}
      </scrollbox>

      <box
        style={{
          flexShrink: 0,
          backgroundColor: 'transparent',
        }}
      >
        {showOnboardingPrompts && !reviewMode && (
          <SuggestedPrompts
            onSelect={handleSelectSuggestedPrompt}
            maxItems={isCompactHeight ? 2 : undefined}
          />
        )}

        {inputMode === 'help' && <HelpBanner />}

        <TodoStrip hidden={todoPanelOpen} />

        {inputMode === 'report' && report && (
          <ReportPanel
            report={report}
            width={terminalWidth}
            maxHeight={Math.floor(terminalHeight / 2)}
          />
        )}

        {shouldShowStatusLine && (
          <StatusBar
            timerStartTime={timerStartTime}
            isAtBottom={isAtBottom}
            scrollToLatest={scrollToLatest}
            statusIndicatorState={statusIndicatorState}
            onStop={chatKeyboardHandlers.onInterruptStream}
          />
        )}

        <SignInPanel width={separatorWidth} />

        {permissionRequest ? (
          <PermissionPrompt
            key={`permission-${permissionRequest.id}`}
            request={permissionRequest}
            width={separatorWidth}
          />
        ) : gitViews.proposal && !gitViews.deferred && !askUserState ? (
          <CommitProposalCard
            proposal={gitViews.proposal}
            width={separatorWidth}
            maxVisibleFiles={isCompactHeight ? 4 : 8}
          />
        ) : gitViews.rewind && !gitViews.deferred && !askUserState ? (
          <RewindPicker
            flow={gitViews.rewind}
            width={separatorWidth}
            maxVisibleRows={isCompactHeight ? 4 : 8}
          />
        ) : gitViews.changesPanelOpen && !askUserState ? (
          <ChangesPanel
            files={gitViews.changes}
            width={separatorWidth}
            maxVisibleRows={isCompactHeight ? 4 : 8}
          />
        ) : modelPickerOpen ? (
          <ModelPicker
            onClose={() => setModelPickerOpen(false)}
            width={separatorWidth}
            maxVisibleRows={isCompactHeight ? 4 : 8}
          />
        ) : reviewMode ? (
          <ReviewScreen
            onSelectOption={handleReviewOptionSelect}
            onCustom={handleReviewCustom}
            onCancel={handleCloseReviewScreen}
          />
        ) : queuePanelOpen && !askUserState ? (
          <QueuePanel
            queuedMessages={queuedMessages}
            onEdit={editQueuedMessage}
            onDelete={removeQueuedMessage}
            onMove={moveQueuedMessage}
            onClose={handleCloseQueuePanel}
            width={separatorWidth}
            maxVisibleRows={isCompactHeight ? 4 : 8}
          />
        ) : todoPanelOpen && !askUserState ? (
          <TodoPanel
            messages={messages}
            onClose={handleCloseTodoPanel}
            maxVisibleRows={isCompactHeight ? 4 : 8}
          />
        ) : (
          <>
            <ChangesSummary files={gitViews.changes} />
            <GitPanelWaiting />
            <ChatInputBar
              inputValue={inputValue}
              cursorPosition={cursorPosition}
              setInputValue={setInputValue}
              inputFocused={inputFocused}
              inputRef={inputRef}
              inputPlaceholder={inputPlaceholder}
              lastEditDueToNav={lastEditDueToNav}
              agentMode={agentMode}
              toggleAgentMode={toggleAgentMode}
              setAgentMode={setAgentMode}
              hasSlashSuggestions={hasSlashSuggestions}
              hasMentionSuggestions={hasMentionSuggestions}
              hasSuggestionMenu={hasSuggestionMenu}
              slashSuggestionItems={slashSuggestionItems}
              agentSuggestionItems={agentSuggestionItems}
              fileSuggestionItems={fileSuggestionItems}
              slashSelectedIndex={slashSelectedIndex}
              agentSelectedIndex={agentSelectedIndex}
              onSlashItemClick={handleSlashItemClick}
              onMentionItemClick={handleMentionItemClick}
              theme={theme}
              terminalHeight={terminalHeight}
              separatorWidth={separatorWidth}
              shouldCenterInputVertically={shouldCenterInputVertically}
              inputBoxTitle={inputBoxTitle}
              onQueuePreviewClick={openQueuePanel}
              isCompactHeight={isCompactHeight}
              isNarrowWidth={isNarrowWidth}
              handleSubmit={handleSubmit}
              onPaste={createPasteHandler({
                text: inputValue,
                cursorPosition,
                onChange: setInputValue,
                onPasteImage: chatKeyboardHandlers.onPasteImage,
                onPasteImagePath: chatKeyboardHandlers.onPasteImagePath,
                onPasteFilePath: chatKeyboardHandlers.onPasteFilePath,
                onPasteLongText: (pastedText) => {
                  const id = crypto.randomUUID()
                  const preview = pastedText.slice(0, 100).replace(/\n/g, ' ')
                  useChatStore.getState().addPendingTextAttachment({
                    id,
                    content: pastedText,
                    preview,
                    charCount: pastedText.length,
                  })
                  // Show temporary status message
                  showClipboardMessage(
                    `${GLYPHS.pastedText} Pasted text (${pastedText.length.toLocaleString()} chars)`,
                    { durationMs: 5000 },
                  )
                },
                cwd: getProjectRoot() ?? process.cwd(),
              })}
              onInterruptStream={chatKeyboardHandlers.onInterruptStream}
            />
          </>
        )}

        <EngineFooter width={separatorWidth} />
      </box>
    </box>
  )
}
