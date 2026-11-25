/**
 * The slash commands the message box offers, and the chat as Markdown or JSON.
 *
 * Ported from Neuron's own TUI (ui/neuron-tui/src/data/slash-commands.ts, the
 * `ALL_SLASH_COMMANDS` and `GIT_VIEW_SLASH_COMMANDS` lists, which derive from
 * Codebuff/Freebuff, Apache-2.0; see ui/neuron-tui/PORTING.md). Kept: the
 * commands that make sense in a window, with the TUI's descriptions reworded and
 * the TUI's feature gates (`feature`) kept. Left out because they only make sense
 * in a terminal or are done another way here: `/bash` (the Terminal tab), `/image`
 * (paste or drop an image), `/queue` (the queue is shown above the box),
 * `/todo` (the side panel), `/theme:toggle` (Settings), `/upgrade` (the account menu),
 * `/review` and `/interview` (prompt templates of the TUI) and `/exit`.
 */

import {
  FEATURE_ACCOUNT,
  FEATURE_CHECKPOINTS,
  FEATURE_GIT_VIEWS,
  FEATURE_REPORTS,
  FEATURE_SESSIONS,
  type Attachment,
  type TranscriptItem,
} from '@neuron/client'

/** The commands this window runs itself. */
export type DesktopCommand = 'new' | 'copy' | 'export' | 'help' | 'model' | 'plan' | 'history'

export interface CommandSpec {
  /** Without the slash. */
  name: string
  summary: string
  /** What follows the name, for the menu: `[md|json]`. */
  args?: string
  /** Typed instead of the name. */
  aliases?: string[]
  /** Listed only while the engine's `ready.features` has this. */
  feature?: string
  /** `desktop`: this window does it. `engine`: it is sent to the engine as a slash command. */
  handler: 'desktop' | 'engine'
  /** Runs at once even while a turn runs; the others wait in the queue. */
  immediate?: boolean
}

export const COMMANDS: CommandSpec[] = [
  { name: 'help', summary: 'Commands and keys', handler: 'desktop', immediate: true, aliases: ['h', '?'] },
  { name: 'new', summary: 'Start a new chat', handler: 'desktop', aliases: ['n', 'clear', 'reset'] },
  { name: 'history', summary: 'Browse and resume past chats', handler: 'desktop', feature: FEATURE_SESSIONS, aliases: ['chats'] },
  { name: 'copy', summary: 'Copy the whole chat as Markdown', handler: 'desktop', immediate: true, aliases: ['copy-chat'] },
  { name: 'export', summary: 'Save the chat to a file', args: '[md|json]', handler: 'desktop', immediate: true, aliases: ['export-chat'] },
  { name: 'model', summary: 'Pick the model', args: '[name]', handler: 'desktop', immediate: true },
  { name: 'plan', summary: 'Plan mode: read-only, plan first', handler: 'desktop' },
  { name: 'status', summary: 'Session, model and context', handler: 'engine' },
  { name: 'context', summary: 'What fills the context window', handler: 'engine' },
  { name: 'cost', summary: 'Tokens used in this chat', handler: 'engine' },
  { name: 'compact', summary: 'Summarise the chat to free up context', handler: 'engine' },
  { name: 'permissions', summary: 'Show or change the permission mode', args: '[mode]', handler: 'engine' },
  { name: 'usage', summary: "Today's use against your plan", handler: 'engine', feature: FEATURE_REPORTS },
  { name: 'init', summary: 'Create a starter NEURON.md for this project', handler: 'engine', feature: FEATURE_REPORTS },
  { name: 'memory', summary: 'The instruction files Neuron loaded', handler: 'engine', feature: FEATURE_REPORTS },
  { name: 'doctor', summary: "Check Neuron's setup in this folder", handler: 'engine', feature: FEATURE_REPORTS },
  { name: 'changes', summary: 'Files changed in this chat', handler: 'engine', feature: FEATURE_GIT_VIEWS },
  { name: 'diff', summary: 'Changes against the last commit', args: '[path]', handler: 'engine', feature: FEATURE_GIT_VIEWS },
  { name: 'commit', summary: 'Draft a commit message', args: '[message]', handler: 'engine', feature: FEATURE_GIT_VIEWS },
  { name: 'rewind', summary: 'Go back to before an earlier prompt', handler: 'engine', feature: FEATURE_CHECKPOINTS },
  { name: 'login', summary: 'Sign in to Zero-X', handler: 'engine', feature: FEATURE_ACCOUNT },
  { name: 'logout', summary: 'Sign out of Zero-X', handler: 'engine', feature: FEATURE_ACCOUNT },
]

/** What the connected engine supports. */
export function availableCommands(features: readonly string[]): CommandSpec[] {
  return COMMANDS.filter((command) => !command.feature || features.includes(command.feature))
}

/** The command a typed name means (a name or an alias), if the engine supports it. */
export function findCommand(name: string, features: readonly string[]): CommandSpec | undefined {
  const wanted = name.toLowerCase()
  return availableCommands(features).find((command) => command.name === wanted || command.aliases?.includes(wanted))
}

/** `/name args`, split; `null` for anything else. */
export function parseSlash(text: string): { name: string; args: string } | null {
  const match = /^\/(\S+)\s*([\s\S]*)$/.exec(text.trim())
  return match ? { name: match[1], args: match[2].trim() } : null
}

/** The commands whose name or alias starts with what is typed after the slash. */
export function matchCommands(features: readonly string[], typed: string): CommandSpec[] {
  const wanted = typed.toLowerCase()
  return availableCommands(features).filter(
    (command) => command.name.startsWith(wanted) || command.aliases?.some((alias) => alias.startsWith(wanted)),
  )
}

// ---------------------------------------------------------------- the chat as text

/** A code fence longer than any run of backticks inside `text`. */
function fence(text: string, language = ''): string {
  const longest = Math.max(2, ...(text.match(/`+/g) ?? []).map((run) => run.length))
  const ticks = '`'.repeat(longest + 1)
  return `${ticks}${language}\n${text}\n${ticks}`
}

function attachmentLines(attachments: Attachment[]): string {
  return attachments
    .map((attachment) => `- ${attachment.path}${attachment.kind === 'folder' ? ' (folder)' : attachment.kind === 'image' ? ' (image)' : ''}`)
    .join('\n')
}

function prettyInput(input: string): string {
  try {
    return JSON.stringify(JSON.parse(input), null, 2)
  } catch {
    return input
  }
}

/** The whole chat as Markdown: messages, reasoning, tool calls with their results, and notices. */
export function transcriptMarkdown(items: TranscriptItem[], title = 'Neuron chat'): string {
  const parts: string[] = [`# ${title.replace(/\s+/g, ' ').trim() || 'Neuron chat'}`]
  for (const item of items) {
    switch (item.kind) {
      case 'user':
        parts.push(`## You\n\n${item.text}${item.attachments.length ? `\n\nAttached:\n${attachmentLines(item.attachments)}` : ''}`)
        break
      case 'assistant':
        parts.push(`## Neuron\n\n${item.text}`)
        break
      case 'reasoning':
        if (item.text.trim()) parts.push(`### Reasoning\n\n${item.text.trim().replace(/^/gm, '> ')}`)
        break
      case 'tool': {
        const input = prettyInput(item.input)
        parts.push(
          `### Tool: ${item.name}\n\n${fence(input, 'json')}${item.output ? `\n\nResult:\n\n${fence(item.output)}` : ''}`,
        )
        break
      }
      case 'question':
        parts.push(`**Question:** ${item.text}`)
        break
      case 'error':
        parts.push(`**Error:** ${item.text}`)
        break
      case 'notice':
        parts.push(`_${item.text}_`)
        break
      case 'report':
        parts.push(`### ${item.title}`)
        break
    }
  }
  return `${parts.join('\n\n')}\n`
}

/** The chat as JSON, without what only the screen needs. */
export function transcriptJson(items: TranscriptItem[]): string {
  const plain = items.map((item) => {
    if (item.kind !== 'tool') return item
    const { view: _view, live: _live, progress: _progress, ...rest } = item
    return rest
  })
  return JSON.stringify({ exportedAt: new Date().toISOString(), items: plain }, null, 2)
}

/** A file name for an export: the chat's title, made plain. */
export function exportName(title: string, format: 'md' | 'json'): string {
  const stem = title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 40)
  return `${stem || 'neuron-chat'}.${format}`
}
