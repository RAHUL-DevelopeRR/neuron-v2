/**
 * `/diff`, `/commit`, `/changes` and `/rewind`, the UI's own while the engine
 * lists the feature each needs (`git_views`, `checkpoints` for `/rewind`).
 * Without it they are not found here, and the router forwards them to the
 * engine as before (an older engine answers with a notice, or not at all).
 */

import {
  requestCheckpoints,
  requestCommit,
  requestDiff,
} from '../engine/git-views'
import { FEATURE_CHECKPOINTS, FEATURE_GIT_VIEWS } from '../engine/protocol'
import { useGitViewsStore } from '../state/git-views-store'
import { getUserMessage } from '../utils/message-history'

import type { CommandDefinition, RouterParams } from './command-registry'

export type GitViewCommand = CommandDefinition & {
  /** The engine feature (`ready.features`) the command needs. */
  feature: string
}

const clearInput = (params: RouterParams) => {
  params.setInputValue({ text: '', cursorPosition: 0, lastEditDueToNav: false })
}

/** Keeps the typed command in history and in the chat, like an engine command. */
const echo = (params: RouterParams) => {
  const typed = params.inputValue.trim()
  params.saveToHistory(typed)
  clearInput(params)
  params.setMessages((prev) => [...prev, getUserMessage(typed)])
}

export const GIT_VIEW_COMMANDS: GitViewCommand[] = [
  {
    name: 'diff',
    aliases: [],
    acceptsArgs: true,
    feature: FEATURE_GIT_VIEWS,
    handler: (params, args) => {
      echo(params)
      requestDiff(args)
    },
  },
  {
    // Without a message only a proposal: the card commits on Enter.
    name: 'commit',
    aliases: [],
    acceptsArgs: true,
    feature: FEATURE_GIT_VIEWS,
    handler: (params, args) => {
      echo(params)
      requestCommit(args)
    },
  },
  {
    name: 'changes',
    aliases: [],
    acceptsArgs: false,
    feature: FEATURE_GIT_VIEWS,
    handler: (params) => {
      params.saveToHistory(params.inputValue.trim())
      clearInput(params)
      useGitViewsStore.getState().openChangesPanel()
    },
  },
  {
    name: 'rewind',
    aliases: [],
    acceptsArgs: true,
    feature: FEATURE_CHECKPOINTS,
    handler: (params, args) => {
      echo(params)
      requestCheckpoints(args)
    },
  },
]

/** The command named `name`, when the engine (`features`) has what it needs. */
export function findGitViewCommand(
  name: string,
  features: readonly string[] = useGitViewsStore.getState().features,
): CommandDefinition | undefined {
  const lower = name.toLowerCase()
  return GIT_VIEW_COMMANDS.find(
    (command) =>
      features.includes(command.feature) &&
      (command.name === lower || command.aliases.includes(lower)),
  )
}
