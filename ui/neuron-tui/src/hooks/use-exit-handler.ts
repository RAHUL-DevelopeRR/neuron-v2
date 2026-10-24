// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the exit
// prints the --continue hint for a chat the engine answered a prompt in.
// See ui/neuron-tui/PORTING.md.
import { useCallback, useEffect, useState } from 'react'

import { isSlashCommand } from '../commands/router-utils'
import { getCurrentChatId } from '../project-files'
import { useChatStore } from '../state/chat-store'
import { CLI_COMMAND } from '../utils/constants'
import { exitCliCleanly, registerExitMessage } from '../utils/exit-cleanly'

import type { ChatMessage } from '../types/chat'
import type { InputValue } from '../types/store'

interface UseExitHandlerOptions {
  inputValue: string
  setInputValue: (value: InputValue) => void
}

type HintMessage = Pick<ChatMessage, 'variant' | 'content'> &
  Partial<Pick<ChatMessage, 'id' | 'blocks'>>

/**
 * A prompt the agent was asked. Slash commands (`/status`, `/model x`) are
 * kept as user messages too, but they leave the engine's session empty: a
 * chat of only those reopens empty.
 */
const isPrompt = (message: HintMessage): boolean =>
  message.variant === 'user' && !isSlashCommand(message.content)

/**
 * The agent answered: text, reasoning, a tool call or a question. Not a
 * notice (`sys-` lines), and not a turn's message that holds only its error,
 * which is what a prompt the engine refused (signed out) leaves.
 */
const isReply = (message: HintMessage): boolean => {
  if (message.variant !== 'ai' || message.id?.startsWith('sys-')) return false
  if (message.content.trim()) return true
  return (message.blocks ?? []).some((block) =>
    block.type === 'text' ? block.content.trim() !== '' : block.type !== 'mode-divider',
  )
}

/** Whether a prompt got a reply, before the next prompt was sent. */
function hasAnsweredPrompt(messages: readonly HintMessage[]): boolean {
  let asked = false
  for (const message of messages) {
    if (message.variant === 'user') {
      asked = isPrompt(message)
    } else if (asked && isReply(message)) {
      return true
    }
  }
  return false
}

/**
 * The line printed on the main screen as the UI exits; none for a chat the
 * engine has nothing of (nothing was asked, or every prompt was refused), so
 * there is nothing to continue.
 */
export function continueHint(
  chatId: string | undefined,
  messages: readonly HintMessage[],
): string | null {
  if (!chatId || !hasAnsweredPrompt(messages)) {
    return null
  }
  return `\nTo continue this session later, run:\n${CLI_COMMAND} --continue ${chatId}\n`
}

function setupExitMessageHandler() {
  // exitCliCleanly prints it once the screen is restored (see exit-cleanly.ts).
  registerExitMessage(() =>
    continueHint(getCurrentChatId(), useChatStore.getState().messages),
  )
}

export const useExitHandler = ({
  inputValue,
  setInputValue,
}: UseExitHandlerOptions) => {
  const [nextCtrlCWillExit, setNextCtrlCWillExit] = useState(false)

  useEffect(() => {
    setupExitMessageHandler()
  }, [])

  const handleCtrlC = useCallback(() => {
    if (inputValue) {
      setInputValue({ text: '', cursorPosition: 0, lastEditDueToNav: false })
      return true
    }

    if (!nextCtrlCWillExit) {
      setNextCtrlCWillExit(true)
      setTimeout(() => {
        setNextCtrlCWillExit(false)
      }, 2000)
      return true
    }

    void exitCliCleanly()
    return true
  }, [inputValue, setInputValue, nextCtrlCWillExit])

  return { handleCtrlC, nextCtrlCWillExit }
}
