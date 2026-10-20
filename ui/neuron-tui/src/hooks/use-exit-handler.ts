import { useCallback, useEffect, useState } from 'react'

import { getCurrentChatId } from '../project-files'
import { CLI_COMMAND } from '../utils/constants'
import { exitCliCleanly } from '../utils/exit-cleanly'

import type { InputValue } from '../types/store'

interface UseExitHandlerOptions {
  inputValue: string
  setInputValue: (value: InputValue) => void
}

let exitHandlerRegistered = false

function setupExitMessageHandler() {
  if (exitHandlerRegistered) return
  exitHandlerRegistered = true

  process.on('exit', () => {
    try {
      const chatId = getCurrentChatId()
      if (chatId) {
        // This runs synchronously during the exit phase
        // OpenTUI has already cleaned up by this point
        process.stdout.write(
          `\nTo continue this session later, run:\n${CLI_COMMAND} --continue ${chatId}\n`,
        )
      }
    } catch {
      // Silent fail - don't block exit
    }
  })
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
