// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: prompts
// and slash commands the UI does not have go to the engine, and a `!cmd`
// prompt is a shell command only when shell commands are implemented
// (USER_SHELL_AVAILABLE); until then it goes to the agent like any prompt.
// An answer to the agent's question drawn as a chat line is marked as one,
// and it and the echo of a failed image path as not sent to the engine, so a
// rewind does not count them as prompts. Enter on an empty input brings up a
// waiting commit card or rewind picker. See ui/neuron-tui/PORTING.md.
import {
  dispatchSkillPrompt,
  findCommand,
  type RouterParams,
  type CommandResult,
} from './command-registry'
import { isSlashCommand, parseCommandInput } from './router-utils'
import {
  buildInterviewPrompt,
  buildPlanPrompt,
  buildReviewPrompt,
} from './prompt-builders'
import { whenEngineStarted } from '../engine/engine-session'
import { revealDeferredGitPanel } from '../engine/git-views'
import { getNeuronEngineClient, runExchange } from '../engine/neuron-engine-client'
import { withQuestionAnswer } from '../engine/questions'
import { statusAddendum } from '../engine/status-report'
import { getProjectRoot } from '../project-files'
import { useChatStore } from '../state/chat-store'
import { useEngineStore } from '../state/engine-store'
import {
  buildBashHistoryMessages,
  createRunTerminalToolResult,
} from '../utils/bash-messages'
import { showClipboardMessage } from '../utils/clipboard'
import { GLYPHS } from '../utils/glyphs'
import { getSystemMessage, getUserMessage } from '../utils/message-history'
import { getSkillByName } from '../utils/skill-registry'
import { pushSteeringMessage } from '../utils/steering-buffer'
import {
  capturePendingAttachments,
  hasProcessingFiles,
  hasProcessingImages,
  validateAndAddImage,
} from '../utils/pending-attachments'
import { USER_SHELL_AVAILABLE, runUserShellCommand } from '../utils/user-shell'

/**
 * Run a bash command with automatic ghost/direct mode selection.
 * Uses ghost mode when streaming or chain in progress, otherwise adds directly to chat history.
 */
export function runBashCommand(command: string) {
  const {
    streamingAgents,
    isChainInProgress,
    setMessages,
    addPendingBashMessage,
    updatePendingBashMessage,
  } = useChatStore.getState()

  const ghost = streamingAgents.size > 0 || isChainInProgress
  const id = crypto.randomUUID()
  const commandCwd = process.cwd()

  if (ghost) {
    // Ghost mode: add to pending messages
    addPendingBashMessage({
      id,
      command,
      stdout: '',
      stderr: '',
      exitCode: 0,
      isRunning: true,
      startTime: Date.now(),
      cwd: commandCwd,
    })
  } else {
    // Direct mode: add to chat history with placeholder output (user + assistant)
    const { assistantMessage } = buildBashHistoryMessages({
      command,
      cwd: commandCwd,
      toolCallId: id,
      output: '...',
    })
    setMessages((prev) => [...prev, assistantMessage])
  }

  runUserShellCommand({ command, cwd: commandCwd })
    .then(({ stdout, stderr, exitCode }) => {
      if (ghost) {
        updatePendingBashMessage(id, {
          stdout,
          stderr,
          exitCode,
          isRunning: false,
        })
      } else {
        const toolResultOutput = createRunTerminalToolResult({
          command,
          cwd: commandCwd,
          stdout: stdout || null,
          stderr: stderr || null,
          exitCode,
        })
        const outputJson = JSON.stringify(toolResultOutput)

        setMessages((prev) =>
          prev.map((msg) => {
            if (!msg.blocks) return msg
            let didUpdate = false
            const blocks = msg.blocks.map((block) => {
              if ('toolCallId' in block && block.toolCallId === id) {
                didUpdate = true
                return { ...block, output: outputJson }
              }
              return block
            })
            return didUpdate ? { ...msg, blocks, isComplete: true } : msg
          }),
        )

        // Also add to pending bash messages so the next user message includes this context for the LLM
        // Mark as already added to history to avoid duplicate UI entries
        addPendingBashMessage({
          id,
          command,
          stdout,
          stderr,
          exitCode,
          isRunning: false,
          cwd: commandCwd,
          addedToHistory: true,
        })
      }
    })
    .catch((error) => {
      const errorMessage =
        error instanceof Error ? error.message : String(error)

      if (ghost) {
        updatePendingBashMessage(id, {
          stdout: '',
          stderr: errorMessage,
          exitCode: 1,
          isRunning: false,
        })
      } else {
        const errorToolResultOutput = createRunTerminalToolResult({
          command,
          cwd: commandCwd,
          stdout: null,
          stderr: null,
          exitCode: 1,
          errorMessage,
        })
        const errorOutputJson = JSON.stringify(errorToolResultOutput)

        setMessages((prev) =>
          prev.map((msg) => {
            if (!msg.blocks) return msg
            let didUpdate = false
            const blocks = msg.blocks.map((block) => {
              if ('toolCallId' in block && block.toolCallId === id) {
                didUpdate = true
                return { ...block, output: errorOutputJson }
              }
              return block
            })
            return didUpdate ? { ...msg, blocks, isComplete: true } : msg
          }),
        )

        // Also add to pending bash messages so the next user message includes this context for the LLM
        // Mark as already added to history to avoid duplicate UI entries
        addPendingBashMessage({
          id,
          command,
          stdout: '',
          stderr: errorMessage,
          exitCode: 1,
          isRunning: false,
          cwd: commandCwd,
          addedToHistory: true,
        })
      }
    })
}

/**
 * Add a completed bash command result to the chat message history.
 * Note: This is UI-only; we no longer send these commands to the AI context.
 */
export function addBashMessageToHistory(params: {
  command: string
  stdout: string
  stderr: string | null
  exitCode: number
  cwd: string
  setMessages: RouterParams['setMessages']
}) {
  const { command, stdout, stderr, exitCode, cwd, setMessages } = params
  const toolResultOutput = createRunTerminalToolResult({
    command,
    cwd,
    stdout: stdout || null,
    stderr: stderr ?? null,
    exitCode,
  })
  const toolCallId = crypto.randomUUID()
  const outputJson = JSON.stringify(toolResultOutput)
  const { assistantMessage } = buildBashHistoryMessages({
    command,
    cwd,
    toolCallId,
    output: outputJson,
    isComplete: true,
  })

  setMessages((prev) => [...prev, assistantMessage])
}

export async function routeUserPrompt(
  params: RouterParams,
): Promise<CommandResult> {
  const {
    agentMode,
    inputRef,
    inputValue,
    isChainInProgressRef,
    isStreaming,
    streamMessageIdRef,
    addToQueue,
    hasQueuedMessages,
    saveToHistory,
    scrollToLatest,
    sendMessage,
    setInputFocused,
    setInputValue,
    setMessages,
  } = params

  const inputMode = useChatStore.getState().inputMode
  const setInputMode = useChatStore.getState().setInputMode
  const pendingAttachments = useChatStore.getState().pendingAttachments

  const trimmed = inputValue.trim()
  // Allow empty messages if there are pending attachments (images or text).
  // Skill mode also accepts an empty submit: it means "run the skill as-is".
  const hasAttachments = pendingAttachments.length > 0
  // Enter on an empty input brings up a commit card or rewind picker that
  // waits behind it (engine/git-views.ts).
  if (!trimmed && !hasAttachments && inputMode === 'default' && revealDeferredGitPanel()) return
  if (!trimmed && !hasAttachments && inputMode !== 'skill') return

  // Handle bash mode commands
  if (inputMode === 'bash') {
    const commandWithBang = '!' + trimmed
    saveToHistory(commandWithBang)
    setInputValue({ text: '', cursorPosition: 0, lastEditDueToNav: false })
    setInputMode('default')
    setInputFocused(true)
    inputRef.current?.focus()

    runBashCommand(trimmed)
    return
  }

  // Handle plan mode input
  if (inputMode === 'plan') {
    if (!trimmed) return
    saveToHistory(trimmed)
    setInputValue({ text: '', cursorPosition: 0, lastEditDueToNav: false })
    setInputMode('default')
    setInputFocused(true)
    inputRef.current?.focus()

    sendMessage({ content: buildPlanPrompt(trimmed), agentMode })
    setTimeout(() => {
      scrollToLatest()
    }, 0)
    return
  }

  // Handle interview mode input
  if (inputMode === 'interview') {
    if (!trimmed) return
    saveToHistory(trimmed)
    setInputValue({ text: '', cursorPosition: 0, lastEditDueToNav: false })
    setInputMode('default')
    setInputFocused(true)
    inputRef.current?.focus()

    sendMessage({ content: buildInterviewPrompt(trimmed), agentMode })
    setTimeout(() => {
      scrollToLatest()
    }, 0)
    return
  }

  // Handle skill mode input: the user picked a skill (bare /skill:<name>)
  // and is now adding instructions. Empty input runs the skill without any.
  if (inputMode === 'skill') {
    const skillName = useChatStore.getState().pendingSkillName
    const skill = skillName ? getSkillByName(skillName) : undefined

    if (!skill) {
      // Mode without a resolvable skill (state got out of sync): explain,
      // and leave the user's typed text in the composer rather than
      // destroying it — only the mode is reset.
      setInputMode('default')
      setInputFocused(true)
      inputRef.current?.focus()
      setMessages((prev) => [
        ...prev,
        getSystemMessage(`Skill not found: ${skillName ?? '(unknown)'}`),
      ])
      return
    }

    setInputValue({ text: '', cursorPosition: 0, lastEditDueToNav: false })
    setInputMode('default')
    setInputFocused(true)
    inputRef.current?.focus()

    if (trimmed) {
      saveToHistory(trimmed)
    }
    dispatchSkillPrompt(params, skill, trimmed)
    return
  }

  // Handle review mode input
  if (inputMode === 'review') {
    if (!trimmed) return
    saveToHistory(trimmed)
    setInputValue({ text: '', cursorPosition: 0, lastEditDueToNav: false })
    setInputMode('default')
    setInputFocused(true)
    inputRef.current?.focus()

    sendMessage({ content: buildReviewPrompt('custom', trimmed), agentMode })
    setTimeout(() => {
      scrollToLatest()
    }, 0)
    return
  }

  // Handle bash commands from queue (starts with '!')
  if (USER_SHELL_AVAILABLE && trimmed.startsWith('!') && trimmed.length > 1) {
    const command = trimmed.slice(1)
    saveToHistory(trimmed)
    setInputValue({ text: '', cursorPosition: 0, lastEditDueToNav: false })
    runBashCommand(command)
    return
  }

  // Handle image mode input
  if (inputMode === 'image') {
    const imagePath = trimmed
    const projectRoot = getProjectRoot()

    // Validate and add the image (handles path resolution, format check, and processing)
    const result = await validateAndAddImage(imagePath, projectRoot)
    if (!result.success) {
      setMessages((prev) => [
        ...prev,
        // Never sent: no prompt for a rewind to count.
        { ...getUserMessage(trimmed), metadata: { sentToEngine: false } },
        getSystemMessage(`${GLYPHS.error} ${result.error}`),
      ])
    }

    // Note: No system message added here - the PendingImagesBanner shows attached images
    saveToHistory(trimmed)
    setInputValue({ text: '', cursorPosition: 0, lastEditDueToNav: false })
    setInputMode('default')
    return
  }

  // Handle slash commands or configured slashless exact commands.
  const parsedCommand = parseCommandInput(trimmed)
  if (parsedCommand) {
    const commandDef = findCommand(parsedCommand.command)
    if (commandDef) {
      // The command handler (via defineCommand/defineCommandWithArgs factories)
      // is responsible for validating and handling args
      return await commandDef.handler(params, parsedCommand.args)
    }
  }

  // Regular message or unknown slash command - send to agent

  // Block sending if attachments are still processing
  if (hasProcessingImages() || hasProcessingFiles()) {
    showClipboardMessage('processing attachments...', {
      durationMs: 2000,
    })
    return
  }

  saveToHistory(trimmed)
  setInputValue({ text: '', cursorPosition: 0, lastEditDueToNav: false })

  // An engine command typed while the engine boots (`/login` right at start)
  // waits for it; prompts wait in sendMessage, behind their user bubble.
  if (isSlashCommand(trimmed)) await whenEngineStarted()
  const engine = getNeuronEngineClient()
  const engineReady = engine.connection === 'ready'

  // A slash command the UI does not have is the engine's (/login, /compact,
  // /status, /permissions, ...). It runs after any turn in progress, and its
  // report comes back as a notice.
  if (isSlashCommand(trimmed) && engineReady) {
    const [command = '', ...args] = trimmed.slice(1).trim().split(/\s+/)
    setMessages((prev) => [...prev, getUserMessage(trimmed)])
    runExchange(engine, () => engine.slash(command, args.join(' ')), () => {})
      .then(() => {
        // The engine's report has no account and no folder trust: the UI
        // adds them from the engine's status (engine/status-report.ts).
        if (command.toLowerCase() !== 'status') return
        const addendum = statusAddendum(useEngineStore.getState().status)
        if (addendum) setMessages((prev) => [...prev, getSystemMessage(addendum)])
      })
      .catch((error) =>
        setMessages((prev) => [
          ...prev,
          getSystemMessage(error instanceof Error ? error.message : String(error)),
        ]),
      )
    return
  }

  // The engine asked a question (AskUserQuestion) and takes the next submit
  // as its answer, even while the turn is still running.
  const { pendingQuestion, pendingQuestionTarget } = useEngineStore.getState()
  if (pendingQuestion && engineReady && trimmed) {
    useEngineStore.getState().setPendingQuestion(null)
    // Into the question's block in the turn's message, so it reads above the
    // agent's reply to it; a chat line when the question is one too.
    setMessages(
      (prev) =>
        (pendingQuestionTarget &&
          withQuestionAnswer(prev, pendingQuestionTarget, trimmed)) || [
          ...prev,
          { ...getUserMessage(trimmed), metadata: { answersQuestion: true, sentToEngine: false } },
        ],
    )
    try {
      engine.submit(trimmed)
    } catch (error) {
      setMessages((prev) => [
        ...prev,
        getSystemMessage(error instanceof Error ? error.message : String(error)),
      ])
    }
    setInputFocused(true)
    inputRef.current?.focus()
    return
  }

  if (
    isStreaming ||
    streamMessageIdRef.current ||
    isChainInProgressRef.current
  ) {
    // Steer the running turn when possible: plain text is handed to the
    // active run and injected at its next step boundary, so the user can
    // redirect the agent without waiting the turn out. Falls back to the
    // queue for anything the steering hook can't carry faithfully:
    // attachments (strings only), a slash command (queued today so it can
    // error/execute after the turn), pending `!` bash output (only
    // prepareUserMessage folds it into the message that referenced it), a
    // non-empty queue (steering would deliver this text ahead of earlier
    // submissions), and the window where no run is accepting steering.
    const canSteer =
      !hasAttachments &&
      !isSlashCommand(trimmed) &&
      useChatStore.getState().pendingBashMessages.length === 0 &&
      !hasQueuedMessages?.()
    if (canSteer) {
      // Echo the bubble now, so the submit is visible immediately, and hand
      // its id to the buffer: if the run ends before draining this entry,
      // use-send-message retracts the bubble and requeues the text (which
      // mints its own bubble at dequeue) — no invisible message, no dupe.
      const steeredMessage = getUserMessage(trimmed)
      if (
        pushSteeringMessage({ messageId: steeredMessage.id, text: trimmed })
      ) {
        setMessages((prev) => [...prev, steeredMessage])
        setInputFocused(true)
        inputRef.current?.focus()
        return
      }
    }
    const pendingAttachmentsForQueue = capturePendingAttachments()
    // Pass a copy of pending attachments to the queue
    addToQueue(trimmed, pendingAttachmentsForQueue)

    setInputFocused(true)
    inputRef.current?.focus()
    return
  }

  // Unknown slash command and no engine to run it - show error
  if (isSlashCommand(trimmed)) {
    setMessages((prev) => [
      ...prev,
      getUserMessage(trimmed),
      getSystemMessage(`Command not found: ${JSON.stringify(trimmed)}`),
    ])
    return
  }

  sendMessage({ content: trimmed, agentMode })

  setTimeout(() => {
    scrollToLatest()
  }, 0)

  return
}
