/**
 * Line diffs for tool cards and permission dialogs: computed from an edit's
 * old and new text before it runs, or read from the unified diff the engine
 * returns after it ran.
 */

export type DiffLineKind = 'context' | 'add' | 'remove' | 'hunk'

export interface DiffLine {
  kind: DiffLineKind
  text: string
  /** Line number in the old text (context and remove lines). */
  oldNumber?: number
  /** Line number in the new text (context and add lines). */
  newNumber?: number
}

export interface Diff {
  lines: DiffLine[]
  added: number
  removed: number
  /** Cut to stay readable; the rest was left out. */
  truncated: boolean
}

/** Past this many cells the LCS table is skipped for a plain remove-then-add. */
const MAX_LCS_CELLS = 4_000_000

const splitLines = (text: string): string[] => {
  if (text === '') return []
  const lines = text.replace(/\r\n/g, '\n').split('\n')
  if (lines[lines.length - 1] === '') lines.pop()
  return lines
}

/**
 * The changed lines between `before` and `after`, with `context` unchanged
 * lines around each change. `firstLine` numbers the first line of `before`
 * (an edit's old string sits somewhere inside its file).
 */
export function diffLines(
  before: string,
  after: string,
  { context = 3, firstLine = 1, maxLines = 400 }: { context?: number; firstLine?: number; maxLines?: number } = {},
): Diff {
  const a = splitLines(before)
  const b = splitLines(after)
  // Common prefix and suffix first: most edits touch a few lines of many.
  let start = 0
  while (start < a.length && start < b.length && a[start] === b[start]) start++
  let endA = a.length
  let endB = b.length
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--
    endB--
  }
  const midA = a.slice(start, endA)
  const midB = b.slice(start, endB)

  type Op = { kind: 'context' | 'add' | 'remove'; text: string }
  const ops: Op[] = a.slice(0, start).map((text) => ({ kind: 'context', text }))
  if (midA.length * midB.length > MAX_LCS_CELLS) {
    for (const text of midA) ops.push({ kind: 'remove', text })
    for (const text of midB) ops.push({ kind: 'add', text })
  } else {
    // lcs[i][j]: the longest common subsequence of midA[i..] and midB[j..].
    const lcs = Array.from({ length: midA.length + 1 }, () => new Uint32Array(midB.length + 1))
    for (let i = midA.length - 1; i >= 0; i--) {
      for (let j = midB.length - 1; j >= 0; j--) {
        lcs[i][j] = midA[i] === midB[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1])
      }
    }
    let i = 0
    let j = 0
    while (i < midA.length || j < midB.length) {
      if (i < midA.length && j < midB.length && midA[i] === midB[j]) {
        ops.push({ kind: 'context', text: midA[i] })
        i++
        j++
      } else if (i < midA.length && (j === midB.length || lcs[i + 1][j] >= lcs[i][j + 1])) {
        // Removals before additions, as unified diffs show them.
        ops.push({ kind: 'remove', text: midA[i++] })
      } else {
        ops.push({ kind: 'add', text: midB[j++] })
      }
    }
  }
  for (const text of a.slice(endA)) ops.push({ kind: 'context', text })

  // Number the lines, then keep only `context` lines around the changes.
  let oldNumber = firstLine
  let newNumber = firstLine
  const numbered: DiffLine[] = ops.map((op) => {
    const line: DiffLine = { kind: op.kind, text: op.text }
    if (op.kind !== 'add') line.oldNumber = oldNumber++
    if (op.kind !== 'remove') line.newNumber = newNumber++
    return line
  })
  const keep = new Array<boolean>(numbered.length).fill(false)
  numbered.forEach((line, index) => {
    if (line.kind === 'context') return
    for (let k = Math.max(0, index - context); k <= Math.min(numbered.length - 1, index + context); k++) keep[k] = true
  })
  const lines: DiffLine[] = []
  let previous = -2
  numbered.forEach((line, index) => {
    if (!keep[index]) return
    if (index !== previous + 1 && lines.length > 0) lines.push({ kind: 'hunk', text: '…' })
    lines.push(line)
    previous = index
  })
  return finish(lines, maxLines)
}

/** Every line of `content` as added: a new file. */
export function additionDiff(content: string, maxLines = 400): Diff {
  const lines = splitLines(content).map((text, index) => ({ kind: 'add' as const, text, newNumber: index + 1 }))
  return finish(lines, maxLines)
}

/**
 * A unified diff as the engine writes it (edit_file's `diff`): headers are
 * dropped, `@@` lines become hunks and number the lines after them.
 */
export function parseUnifiedDiff(text: string, maxLines = 400): Diff {
  const lines: DiffLine[] = []
  let oldNumber = 1
  let newNumber = 1
  for (const raw of text.replace(/\r\n/g, '\n').split('\n')) {
    if (raw.startsWith('--- ') || raw.startsWith('+++ ') || raw.startsWith('diff ') || raw.startsWith('index ')) continue
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/.exec(raw)
    if (hunk) {
      oldNumber = Number(hunk[1])
      newNumber = Number(hunk[2])
      lines.push({ kind: 'hunk', text: raw })
    } else if (raw.startsWith('+')) {
      lines.push({ kind: 'add', text: raw.slice(1), newNumber: newNumber++ })
    } else if (raw.startsWith('-')) {
      lines.push({ kind: 'remove', text: raw.slice(1), oldNumber: oldNumber++ })
    } else if (raw.startsWith(' ')) {
      lines.push({ kind: 'context', text: raw.slice(1), oldNumber: oldNumber++, newNumber: newNumber++ })
    } else if (raw.startsWith('\\')) {
      continue // "\ No newline at end of file"
    }
  }
  return finish(lines, maxLines)
}

function finish(lines: DiffLine[], maxLines: number): Diff {
  const added = lines.filter((line) => line.kind === 'add').length
  const removed = lines.filter((line) => line.kind === 'remove').length
  const truncated = lines.length > maxLines
  return { lines: truncated ? lines.slice(0, maxLines) : lines, added, removed, truncated }
}
