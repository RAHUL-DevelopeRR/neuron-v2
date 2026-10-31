/**
 * Git views as text: a diff's hunks and notes, and what a rewind did, for the
 * transcript cell (components/git-view-cell.tsx), the rewind picker's preview
 * and `/copy` / `/export`.
 */

import { lineCounts, rewindModeLabel, statusMark } from '../state/git-views-store'
import { GLYPHS } from './glyphs'

import type { DiffFile, RewoundEvent } from '../engine/protocol'
import type { GitView } from '../types/git-view'

/** One hunk of a file's patch: its `@@` line (if any) and the hunk with it. */
export interface PatchHunk {
  header: string | null
  /** The hunk's lines, `@@` line first, as DiffViewer takes them. */
  text: string
}

/** A patch from its first `@@` line on, split into hunks. */
export function splitHunks(patch: string): PatchHunk[] {
  const hunks: PatchHunk[] = []
  let current: string[] = []
  let header: string | null = null
  const flush = () => {
    if (current.length > 0 && current.some((line) => line.trim() !== '')) {
      hunks.push({ header, text: current.join('\n') })
    }
    current = []
  }
  for (const line of patch.replace(/\r\n/g, '\n').split('\n')) {
    if (line.startsWith('@@')) {
      flush()
      header = line
    }
    current.push(line)
  }
  flush()
  return hunks
}

/** What a file of a diff shows instead of hunks, if anything. */
export function diffFileNote(file: DiffFile, truncated: boolean): string | null {
  if (file.binary) return 'binary file'
  if (file.patch.trim()) return null
  if (truncated) return 'diff left out to keep this short'
  return file.status === 'renamed' ? 'renamed, no line changes' : null
}

export type RewoundTone = 'normal' | 'muted' | 'warning'
export interface RewoundLine {
  text: string
  tone: RewoundTone
}

const plural = (count: number, one: string, many = `${one}s`) =>
  `${count} ${count === 1 ? one : many}`

const pathList = (paths: readonly string[], max = 6) =>
  paths.length > max
    ? `${paths.slice(0, max).join(', ')} and ${paths.length - max} more`
    : paths.join(', ')

/**
 * What a rewind did, line by line; a dry run says what it would do. `prompt`
 * says the prompt went back into the input.
 */
export function rewoundLines(result: Omit<RewoundEvent, 'type'>): RewoundLine[] {
  const dry = result.dry_run
  const lines: RewoundLine[] = [
    {
      text: `${dry ? 'Rewind preview' : 'Rewound'}: checkpoint ${result.checkpoint}, ${rewindModeLabel(result.mode).toLowerCase()}`,
      tone: 'normal',
    },
  ]
  if (result.restored.length > 0) {
    lines.push({
      text: `${dry ? 'Would restore' : 'Restored'} ${plural(result.restored.length, 'file')}: ${pathList(result.restored)}`,
      tone: 'muted',
    })
  }
  if (result.removed.length > 0) {
    lines.push({
      text: `${dry ? 'Would remove' : 'Removed'} ${plural(result.removed.length, 'file')} that did not exist then: ${pathList(result.removed)}`,
      tone: 'muted',
    })
  }
  if (result.mode !== 'chat_only' && result.restored.length + result.removed.length === 0) {
    lines.push({ text: 'No files to put back.', tone: 'muted' })
  }
  for (const file of result.not_restored) {
    lines.push({
      text: `${dry ? 'Cannot restore' : 'Not restored'} ${file.path}: ${file.reason}`,
      tone: 'warning',
    })
  }
  if (result.shell_commands.length > 0) {
    lines.push({
      text: `${GLYPHS.warning} ${plural(result.shell_commands.length, 'shell command')} ran since then; ${dry ? 'a rewind does' : 'the rewind did'} not undo what ${result.shell_commands.length === 1 ? 'it' : 'they'} did: ${pathList(result.shell_commands, 3)}`,
      tone: 'warning',
    })
  }
  if (result.removed_turns > 0) {
    const prompts = plural(result.removed_turns, 'prompt')
    lines.push({
      text: dry
        ? `Would take ${prompts} out of the conversation.`
        : `Took ${prompts} out of the conversation${result.prompt ? '; the prompt is back in the input' : ''}.`,
      tone: 'muted',
    })
  }
  return lines
}

/** A git view as Markdown: a fenced diff, or the rewind's lines. */
export function gitViewMarkdown(view: GitView): string {
  if (view.kind === 'rewound') {
    const [first, ...rest] = rewoundLines(view).map((line) => line.text)
    return [`**${first}**`, ...rest].join('\n')
  }
  const { added, removed } = lineCounts(view.files)
  const parts = [`**${view.title}** (${plural(view.files.length, 'file')} +${added} -${removed})`]
  for (const file of view.files) {
    const note = diffFileNote(file, view.truncated)
    const heading = `${statusMark(file.status)} ${file.path} +${file.added} -${file.removed}`
    parts.push(note ? `${heading} (${note})` : heading)
    if (!file.binary && file.patch.trim()) {
      parts.push('```diff\n' + file.patch.replace(/\r\n/g, '\n').trimEnd() + '\n```')
    }
  }
  if (view.truncated) parts.push('_Some files or hunks were left out._')
  return parts.join('\n\n')
}
