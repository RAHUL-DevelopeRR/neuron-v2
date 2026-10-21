// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
import { useCallback, useEffect, useState } from 'react'

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

/**
 * The line printed on the main screen as the UI exits; none for a chat nothing was
 * asked in, which has nothing to continue.
 */
export function continueHint(
  chatId: string | undefined,
  messages: readonly Pick<ChatMessage, 'variant'>[],
): string | null {
  if (!chatId || !messages.some((message) => message.variant === 'user')) {
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
