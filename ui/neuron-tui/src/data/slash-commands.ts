// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
import { AGENT_MODES } from '../utils/constants'

import type { SkillsMap } from '@codebuff/common/types/skill'

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
}

// Generate mode commands from the AGENT_MODES constant
const MODE_COMMANDS: SlashCommand[] = AGENT_MODES.map((mode) => ({
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
  {
    id: 'bash',
    label: 'bash',
    description: 'Enter bash mode ("!" at beginning enters bash mode)',
    aliases: ['!'],
  },
  {
    id: 'image',
    label: 'image',
    description: 'Attach an image file (or Ctrl+V to paste from clipboard)',
    aliases: ['img', 'attach'],
  },
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
 */
export function getSlashCommandsWithSkills(skills: SkillsMap): SlashCommand[] {
  const skillCommands: SlashCommand[] = Object.values(skills).map((skill) => ({
    id: `skill:${skill.name}`,
    label: `skill:${skill.name}`,
    description: truncateDescription(skill.description),
    insertText: `/skill:${skill.name} `,
  }))

  const commands = [...SLASH_COMMANDS, ...skillCommands]

  return commands
}
