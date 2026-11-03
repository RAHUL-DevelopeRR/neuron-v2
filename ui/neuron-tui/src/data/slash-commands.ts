// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the
// commands are Neuron's (engine commands are forwarded), /bash is not listed
// while shell commands are not implemented (USER_SHELL_AVAILABLE), and nor are
// the /mode:* commands (AGENT_MODES_AVAILABLE) or /image while images don't
// reach the model (IMAGES_REACH_THE_MODEL); the git commands are listed only
// when the engine has their feature.
// See ui/neuron-tui/PORTING.md.
import { IMAGES_REACH_THE_MODEL } from '../engine/attachments'
import { FEATURE_CHECKPOINTS, FEATURE_GIT_VIEWS } from '../engine/protocol'
import { AGENT_MODES, AGENT_MODES_AVAILABLE } from '../utils/constants'
import { USER_SHELL_AVAILABLE } from '../utils/user-shell'

import type { SkillsMap } from '@neuron-tui/common/types/skill'

export interface SlashCommand {
  id: string
  label: string
  description: string
  aliases?: string[]
  /**
   * If true, this command can be invoked without a leading slash when the
   * input matches the command id exactly (no arguments).
   */
  implicitCommand?: boolean
  /**
   * If set, selecting this command inserts this text into the input field
   * instead of executing a command. Useful for agent shortcuts.
   */
  insertText?: string
  /**
   * Run by the engine: the UI has no handler, so the router forwards it as a
   * `slash` command.
   */
  engineCommand?: boolean
  /** Listed only while the engine's `ready.features` has this one. */
  feature?: string
}

// Generate mode commands from the AGENT_MODES constant
const MODE_COMMANDS: SlashCommand[] = (AGENT_MODES_AVAILABLE ? AGENT_MODES : []).map((mode) => ({
  id: `mode:${mode.toLowerCase()}`,
  label: `mode:${mode.toLowerCase()}`,
  description: `Switch to ${mode} mode`,
  aliases: [`model:${mode.toLowerCase()}`],
}))

const ALL_SLASH_COMMANDS: SlashCommand[] = [
  {
    id: 'help',
    label: 'help',
    description: 'Display keyboard shortcuts and tips',
    aliases: ['h', '?'],
    implicitCommand: true,
  },
  // {
  //   id: 'undo',
  //   label: 'undo',
  //   description: 'Undo the last change made by the assistant',
  // },
  // {
  //   id: 'redo',
  //   label: 'redo',
  //   description: 'Redo the most recent undone change',
  // },
  {
    id: 'interview',
    label: 'interview',
    description: 'AI asks a series of questions to flesh out request into a spec',
  },
  {
    id: 'plan',
    label: 'plan',
    description: 'Create a plan for how to implement a request',
  },
  {
    id: 'review',
    label: 'review',
    description: 'Review code changes',
  },
  {
    id: 'queue',
    label: 'queue',
    description: 'Edit, reorder, or delete the messages waiting to be sent',
    aliases: ['queued'],
  },
  {
    id: 'todo',
    label: 'todo',
    description: 'View the current checklist and progress',
    aliases: ['todos'],
  },
  {
    id: 'new',
    label: 'new',
    description: 'Clear the conversation history and start a new chat',
    aliases: ['n', 'clear', 'c', 'reset'],
    implicitCommand: true,
  },
  {
    id: 'history',
    label: 'history',
    description: 'Browse and resume past conversations',
    aliases: ['chats'],
  },
  {
    id: 'copy',
    label: 'copy',
    description: 'Copy the full conversation (messages + tool results) to the clipboard',
    aliases: ['copy-chat'],
  },
  {
    id: 'export',
    label: 'export',
    description: 'Write the full conversation to a file (.md, or .json for raw messages)',
    aliases: ['export-chat'],
  },
  ...(USER_SHELL_AVAILABLE
    ? [
        {
          id: 'bash',
          label: 'bash',
          description: 'Enter bash mode ("!" at beginning enters bash mode)',
          aliases: ['!'],
        },
      ]
    : []),
  ...(IMAGES_REACH_THE_MODEL
    ? [
        {
          id: 'image',
          label: 'image',
          description: 'Attach an image file (or Ctrl+V to paste from clipboard)',
          aliases: ['img', 'attach'],
        },
      ]
    : []),
  {
    id: 'model',
    label: 'model',
    description: 'Pick the model (or /model <name> to switch at once)',
  },
  // The engine runs these; the router forwards commands it has no handler for.
  {
    id: 'login',
    label: 'login',
    engineCommand: true,
    description: 'Sign in to Zero-X in your browser',
  },
  {
    id: 'logout',
    label: 'logout',
    engineCommand: true,
    description: 'Sign out of Zero-X',
  },
  {
    id: 'status',
    label: 'status',
    engineCommand: true,
    description: 'Session status: model, permissions, context',
  },
  {
    id: 'cost',
    label: 'cost',
    engineCommand: true,
    description: 'Token usage for this session',
  },
  // These four answer with a report panel (report-panel.tsx).
  {
    id: 'usage',
    label: 'usage',
    engineCommand: true,
    description: "Today's requests and tokens against your plan, and when they reset",
  },
  {
    id: 'init',
    label: 'init',
    engineCommand: true,
    description: 'Create a starter NEURON.md for this project',
  },
  {
    id: 'memory',
    label: 'memory',
    engineCommand: true,
    description: 'Show the instruction files Neuron loaded',
  },
  {
    id: 'doctor',
    label: 'doctor',
    engineCommand: true,
    description: "Check Neuron's setup in this folder",
  },
  {
    id: 'context',
    label: 'context',
    engineCommand: true,
    description: 'What fills the context window',
  },
  {
    id: 'compact',
    label: 'compact',
    engineCommand: true,
    description: 'Summarize the conversation to free up context',
  },
  {
    id: 'permissions',
    label: 'permissions',
    engineCommand: true,
    description: 'Show or change the permission mode',
  },
  ...MODE_COMMANDS,
  {
    id: 'upgrade',
    label: 'upgrade',
    description: 'Plans and pricing (opens zero-x.live/pricing)',
  },
  {
    id: 'theme:toggle',
    label: 'theme:toggle',
    description: 'Toggle between light and dark mode',
  },
  {
    id: 'exit',
    label: 'exit',
    description: 'Quit the CLI',
    aliases: ['quit', 'q'],
    implicitCommand: true,
  },
]

export const SLASH_COMMANDS = ALL_SLASH_COMMANDS

/**
 * The git commands (commands/git-view-commands.ts), listed after /model when
 * the engine has their feature; an older engine never shows them.
 */
export const GIT_VIEW_SLASH_COMMANDS: SlashCommand[] = [
  {
    id: 'changes',
    label: 'changes',
    description: 'Files the agent changed this session, with a diff for each',
    feature: FEATURE_GIT_VIEWS,
  },
  {
    id: 'diff',
    label: 'diff',
    description: 'Show the changes against HEAD (or /diff <path> for one file)',
    feature: FEATURE_GIT_VIEWS,
  },
  {
    id: 'commit',
    label: 'commit',
    description: 'Draft a commit message to review, then commit (or /commit <message>)',
    feature: FEATURE_GIT_VIEWS,
  },
  {
    id: 'rewind',
    label: 'rewind',
    description: 'Go back to before an earlier prompt: code, chat or both',
    feature: FEATURE_CHECKPOINTS,
  },
]

/** The slash commands with the git ones the engine (`features`) has. */
export function slashCommandsFor(features: readonly string[] = []): SlashCommand[] {
  const gitCommands = GIT_VIEW_SLASH_COMMANDS.filter(
    (command) => command.feature && features.includes(command.feature),
  )
  if (gitCommands.length === 0) return SLASH_COMMANDS
  const at = SLASH_COMMANDS.findIndex((command) => command.id === 'model') + 1
  return [...SLASH_COMMANDS.slice(0, at), ...gitCommands, ...SLASH_COMMANDS.slice(at)]
}

export const SLASHLESS_COMMAND_IDS = new Set(
  SLASH_COMMANDS.filter((cmd) => cmd.implicitCommand).map((cmd) =>
    cmd.id.toLowerCase(),
  ),
)

/** Maximum description length for skill commands in the slash menu */
const SKILL_MENU_DESCRIPTION_MAX_LENGTH = 50

function truncateDescription(description: string): string {
  if (description.length <= SKILL_MENU_DESCRIPTION_MAX_LENGTH) {
    return description
  }
  return description.slice(0, SKILL_MENU_DESCRIPTION_MAX_LENGTH - 1) + '…'
}

/**
 * Returns SLASH_COMMANDS merged with skill commands.
 * Skills become slash commands that users can invoke directly.
 * `features`: the engine's, which decide the git commands.
 */
export function getSlashCommandsWithSkills(
  skills: SkillsMap,
  features: readonly string[] = [],
): SlashCommand[] {
  const skillCommands: SlashCommand[] = Object.values(skills).map((skill) => ({
    id: `skill:${skill.name}`,
    label: `skill:${skill.name}`,
    description: truncateDescription(skill.description),
    insertText: `/skill:${skill.name} `,
  }))

  const commands = [...slashCommandsFor(features), ...skillCommands]

  return commands
}
