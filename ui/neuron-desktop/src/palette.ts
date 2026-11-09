/** The command palette's commands, and how a query ranks them. */

export interface PaletteCommand {
  id: string
  title: string
  /** A heading: `Chats`, `Sessions`, `Actions`. */
  group: string
  keys?: string
  run: () => void
}

/** How well `query` fits `text`: a start beats a word start beats a substring beats scattered letters; 0 is no fit. */
function score(text: string, query: string): number {
  const at = text.indexOf(query)
  if (at === 0) return 4
  if (at > 0) return /[\s\-_/.]/.test(text[at - 1]) ? 3 : 2
  let next = 0
  for (const ch of text) if (ch === query[next]) next++
  return next === query.length ? 1 : 0
}

/** The commands that fit `query`, best first; with no query, all of them in the order given. Several words must each fit. */
export function rankCommands(commands: readonly PaletteCommand[], query: string): PaletteCommand[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean)
  if (words.length === 0) return [...commands]
  const ranked: Array<{ command: PaletteCommand; score: number; index: number }> = []
  commands.forEach((command, index) => {
    const title = command.title.toLowerCase()
    const group = command.group.toLowerCase()
    let total = 0
    for (const word of words) {
      const best = Math.max(score(title, word), score(group, word) / 2)
      if (best === 0) return
      total += best
    }
    ranked.push({ command, score: total, index })
  })
  return ranked.sort((a, b) => b.score - a.score || a.index - b.index).map((entry) => entry.command)
}
