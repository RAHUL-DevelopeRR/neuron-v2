/**
 * `/diff`, `/commit`, `/changes` and `/rewind`, the UI's own while the engine
 * lists the feature each needs (`git_views`, `checkpoints` for `/rewind`).
 * Without it they are not found here, and the router forwards them to the
 * engine as before (an older engine answers with a notice, or not at all).
 * `/commit` and `/rewind` are refused while a prompt's turn runs.
 */

import {
  requestCheckpoints,
  requestCommit,
  requestDiff,
} from '../engine/git-views'
import { FEATURE_CHECKPOINTS, FEATURE_GIT_VIEWS } from '../engine/protocol'
import { useGitViewsStore } from '../state/git-views-store'
import { getSystemMessage, getUserMessage } from '../utils/message-history'

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

/** A prompt's turn runs, or prompts wait in the queue for one. */
const turnRunning = (params: RouterParams) =>
  params.isStreaming || params.isChainInProgressRef.current || params.hasQueuedMessages?.() === true

/**
 * `/commit` and `/rewind` wait for no turn: one that ran first would change
 * what they act on, and their card would come up while the user types the
 * next prompt. Said in the chat; `true` when refused.
 */
const refusedDuringTurn = (params: RouterParams, command: string): boolean => {
  if (!turnRunning(params)) return false
  echo(params)
  params.setMessages((prev) => [
    ...prev,
    getSystemMessage(`Wait for the running turn to end (or stop it), then run /${command} again.`),
  ])
  return true
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
      if (refusedDuringTurn(params, 'commit')) return
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
      if (refusedDuringTurn(params, 'rewind')) return
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
