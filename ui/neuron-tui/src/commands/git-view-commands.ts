/**
 * `/diff`, `/commit`, `/changes` and `/rewind`, the UI's own while the engine
 * lists the feature each needs (`git_views`, `checkpoints` for `/rewind`);
 * `/mcp` with `mcp_toggle` and `/research` with `research`.
 * Without it they are not found here, and the router forwards them to the
 * engine as before (an older engine answers with a notice, or not at all).
 * `/commit` and `/rewind` are refused while a prompt's turn runs.
 */

import {
  requestCheckpoints,
  requestCommit,
  requestDiff,
} from '../engine/git-views'
import { forwardMcpSlash, openMcpPanel } from '../engine/mcp-panel'
import {
  FEATURE_CHECKPOINTS,
  FEATURE_GIT_VIEWS,
  FEATURE_MCP_TOGGLE,
  FEATURE_RESEARCH,
} from '../engine/protocol'
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

/** How `/research` is used, shown when it has no question. */
export const RESEARCH_USAGE =
  'Usage: /research [--depth quick|standard|deep] [--budget TOKENS] <question>'

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
  {
    // `/mcp` alone opens the servers panel; with arguments it is the engine's.
    name: 'mcp',
    aliases: [],
    acceptsArgs: true,
    feature: FEATURE_MCP_TOGGLE,
    handler: (params, args) => {
      if (args.trim()) {
        echo(params)
        forwardMcpSlash(args)
        return
      }
      params.saveToHistory(params.inputValue.trim())
      clearInput(params)
      openMcpPanel()
    },
  },
  {
    // Sent to the engine as a prompt, not a slash command, so the run is a turn:
    // its searches and reads show as tool rows and its plan as the task list.
    name: 'research',
    aliases: [],
    acceptsArgs: true,
    feature: FEATURE_RESEARCH,
    handler: (params, args) => {
      if (!args.trim()) {
        echo(params)
        params.setMessages((prev) => [...prev, getSystemMessage(RESEARCH_USAGE)])
        return
      }
      const typed = params.inputValue.trim()
      params.saveToHistory(typed)
      clearInput(params)
      if (turnRunning(params)) {
        params.addToQueue(typed)
        return
      }
      void params.sendMessage({ content: typed, agentMode: params.agentMode })
      setTimeout(() => params.scrollToLatest(), 0)
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
