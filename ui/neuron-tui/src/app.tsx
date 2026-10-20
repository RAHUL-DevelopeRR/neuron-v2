// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the
// Codebuff login modal and auth queries, BYOK selection, and the Freebuff
// model catalog and session gate are gone. Sign-in comes from the engine (P3).
// See ui/neuron-tui/PORTING.md.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useShallow } from 'zustand/react/shallow'

import { Chat } from './chat'
import { ChatHistoryScreen } from './components/chat-history-screen'
import { ChatRuntimeProvider } from './contexts/chat-runtime-context'
import { ProjectPickerScreen } from './components/project-picker-screen'
import { useTerminalFocus } from './hooks/use-terminal-focus'
import { getProjectRoot, startNewChat } from './project-files'
import { useChatHistoryStore } from './state/chat-history-store'
import { stopActiveRun } from './utils/active-run'
import { useChatStore } from './state/chat-store'
import type { TopBannerType } from './types/store'
import { findGitRoot } from './utils/git'

import type { MultilineInputHandle } from './components/multiline-input'
import type { AgentMode } from './utils/constants'
import type { FileTreeNode } from '@codebuff/common/util/file'

interface AppProps {
  initialPrompt: string | null
  fileTree: FileTreeNode[]
  continueChat: boolean
  continueChatId?: string
  initialMode?: AgentMode
  showProjectPicker: boolean
  onProjectChange: (projectPath: string) => void
}

export const App = ({
  initialPrompt,
  fileTree,
  continueChat,
  continueChatId,
  initialMode,
  showProjectPicker,
  onProjectChange,
}: AppProps) => {
  const inputRef = useRef<MultilineInputHandle | null>(null)
  const initialPromptConsumedRef = useRef(false)
  const consumeInitialPrompt = useCallback(() => {
    if (!initialPrompt || initialPromptConsumedRef.current) {
      return null
    }
    initialPromptConsumedRef.current = true
    return initialPrompt
  }, [initialPrompt])
  const {
    setInputFocused,
    setIsFocusSupported,
    resetChatStore,
    activeTopBanner,
    setActiveTopBanner,
    closeTopBanner,
    chatSessionId,
  } = useChatStore(
    useShallow((store) => ({
      setInputFocused: store.setInputFocused,
      setIsFocusSupported: store.setIsFocusSupported,
      resetChatStore: store.reset,
      activeTopBanner: store.activeTopBanner,
      setActiveTopBanner: store.setActiveTopBanner,
      closeTopBanner: store.closeTopBanner,
      chatSessionId: store.chatSessionId,
    })),
  )

  // Wrap in useCallback to prevent re-subscribing on every render
  const handleSupportDetected = useCallback(() => {
    setIsFocusSupported(true)
  }, [setIsFocusSupported])

  // Enable terminal focus detection to stop cursor blinking when window loses focus
  // Cursor starts visible but not blinking; blinking enabled once terminal support confirmed
  useTerminalFocus({
    onFocusChange: setInputFocused,
    onSupportDetected: handleSupportDetected,
  })

  const projectRoot = getProjectRoot()
  const gitRoot = useMemo(
    () => findGitRoot({ cwd: projectRoot }),
    [projectRoot],
  )
  const showGitRootBanner = Boolean(gitRoot && gitRoot !== projectRoot)
  const [gitRootBannerDismissed, setGitRootBannerDismissed] = useState(false)
  const prevTopBannerRef = useRef<TopBannerType | null>(null)

  useEffect(() => {
    setGitRootBannerDismissed(false)
  }, [projectRoot])

  useEffect(() => {
    const prevBanner = prevTopBannerRef.current
    if (
      prevBanner === 'gitRoot' &&
      activeTopBanner === null &&
      showGitRootBanner
    ) {
      setGitRootBannerDismissed(true)
    }
    prevTopBannerRef.current = activeTopBanner
  }, [activeTopBanner, showGitRootBanner])

  useEffect(() => {
    if (!showGitRootBanner) {
      if (activeTopBanner === 'gitRoot') {
        closeTopBanner()
      }
      return
    }
    if (!gitRootBannerDismissed && activeTopBanner === null) {
      setActiveTopBanner('gitRoot')
    }
  }, [
    activeTopBanner,
    closeTopBanner,
    gitRootBannerDismissed,
    setActiveTopBanner,
    showGitRootBanner,
  ])

  const handleSwitchToGitRoot = useCallback(() => {
    if (gitRoot) {
      onProjectChange(gitRoot)
    }
  }, [gitRoot, onProjectChange])

  // Chat history state from store
  const { showChatHistory, closeChatHistory } = useChatHistoryStore()

  // State to track which chat to resume (set when user selects from history)
  const [resumeChatId, setResumeChatId] = useState<string | null>(null)
  const [continueRequested, setContinueRequested] = useState(continueChat)

  const handleResumeChat = useCallback(
    (chatId: string) => {
      // Abort any in-flight run BEFORE resetting the store and switching
      // chats: an orphaned run would keep checkpointing, and its writes could
      // land in the resumed chat's directory, overwriting that transcript.
      stopActiveRun('history-resume')
      closeChatHistory()
      // Reset chat store to clear previous messages before loading the selected chat
      resetChatStore()
      setResumeChatId(chatId)
      setContinueRequested(true)
    },
    [closeChatHistory, resetChatStore],
  )

  const handleNewChat = useCallback(() => {
    stopActiveRun('new-chat')
    closeChatHistory()
    resetChatStore()
    // Rotate the chat id so the new conversation saves to its own directory
    // instead of overwriting the current (possibly resumed) chat's history
    startNewChat()
    setResumeChatId(null)
    setContinueRequested(false)
  }, [closeChatHistory, resetChatStore])

  // Determine effective continueChat values
  const effectiveContinueChat = continueRequested
  const effectiveContinueChatId = resumeChatId ?? continueChatId

  // Render project picker FIRST when at home directory or outside a project.
  if (showProjectPicker) {
    return (
      <ProjectPickerScreen
        onSelectProject={onProjectChange}
        initialPath={projectRoot}
      />
    )
  }

  // Reset the runtime only when the active chat identity changes. View-only
  // routes such as history and session gates keep the same key.
  return (
    <AuthedSurface
      runtimeKey={chatSessionId}
      consumeInitialPrompt={consumeInitialPrompt}
      fileTree={fileTree}
      inputRef={inputRef}
      continueChat={effectiveContinueChat}
      continueChatId={effectiveContinueChatId}
      initialMode={initialMode}
      gitRoot={gitRoot}
      onSwitchToGitRoot={handleSwitchToGitRoot}
      showChatHistory={showChatHistory}
      onSelectChat={handleResumeChat}
      onCancelChatHistory={closeChatHistory}
      onNewChat={handleNewChat}
    />
  )
}

interface AuthedSurfaceProps {
  runtimeKey: string
  consumeInitialPrompt: () => string | null
  fileTree: FileTreeNode[]
  inputRef: React.MutableRefObject<MultilineInputHandle | null>
  continueChat: boolean
  continueChatId: string | undefined
  initialMode: AgentMode | undefined
  gitRoot: string | null | undefined
  onSwitchToGitRoot: () => void
  showChatHistory: boolean
  onSelectChat: (chatId: string) => void
  onCancelChatHistory: () => void
  onNewChat: () => void
}

/** Owns the chat runtime for one chat identity. */
const AuthedSurface = (props: AuthedSurfaceProps) => {
  return (
    <ChatRuntimeProvider
      key={props.runtimeKey}
      inputRef={props.inputRef}
      continueChat={props.continueChat}
      continueChatId={props.continueChatId}
    >
      <AuthedSurfaceRoutes {...props} />
    </ChatRuntimeProvider>
  )
}

export const AuthedSurfaceRoutes = ({
  consumeInitialPrompt,
  fileTree,
  inputRef,
  initialMode,
  gitRoot,
  onSwitchToGitRoot,
  showChatHistory,
  onSelectChat,
  onCancelChatHistory,
  onNewChat,
}: AuthedSurfaceProps) => {
  // Keep history inside the runtime so browsing never tears down a running
  // session.
  if (showChatHistory) {
    return (
      <ChatHistoryScreen
        onSelectChat={onSelectChat}
        onCancel={onCancelChatHistory}
        onNewChat={onNewChat}
      />
    )
  }

  return (
    <Chat
      consumeInitialPrompt={consumeInitialPrompt}
      fileTree={fileTree}
      inputRef={inputRef}
      initialMode={initialMode}
      gitRoot={gitRoot}
      onSwitchToGitRoot={onSwitchToGitRoot}
    />
  )
}
