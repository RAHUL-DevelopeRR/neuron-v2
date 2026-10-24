// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: with
// --continue the continued chat is the current one from the start, so its
// directory is reused instead of a new one made. See ui/neuron-tui/PORTING.md.
import os from 'os'

import { initializeThemeStore } from '../hooks/use-theme'
import { continueChatAtStart, setProjectRoot } from '../project-files'
import { initTimestampFormatter } from '../utils/helpers'
import { shouldShowProjectPicker } from '../utils/project-picker'
import { enableManualThemeRefresh } from '../utils/theme-system'
import { initializeDirenv } from './init-direnv'

export async function initializeApp(params: {
  cwd?: string
  /** `--continue [id]`: the chat to continue (`null` for the most recent). */
  continueChat?: { chatId: string | null } | null
}): Promise<void> {
  if (params.cwd) {
    process.chdir(params.cwd)
  }
  const baseCwd = process.cwd()
  setProjectRoot(baseCwd)
  // Before anything logs: the first log line creates the current chat's
  // directory. Not from the home folder, where a project is picked first.
  if (params.continueChat && !shouldShowProjectPicker(baseCwd, os.homedir())) {
    continueChatAtStart(params.continueChat.chatId)
  }

  // Initialize direnv environment before anything else
  initializeDirenv()

  initializeThemeStore()
  enableManualThemeRefresh()
  initTimestampFormatter()
}
