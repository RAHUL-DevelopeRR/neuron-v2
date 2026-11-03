/**
 * How a client shows a tool call: what kind of call it is, a one-line title,
 * the diff it makes (before it runs, from its input; after, from the engine's
 * result) and the output worth showing. Neuron's tools report their results as
 * the JSON of their Rust output structs (runtime/src/file_ops.rs, tools/src/lib.rs).
 */

import { additionDiff, diffLines, parseUnifiedDiff, type Diff } from './diff'

export type ToolKind = 'terminal' | 'read' | 'write' | 'edit' | 'search' | 'web' | 'todo' | 'agent' | 'skill' | 'mcp' | 'other'

export interface ToolTodo {
  text: string
  status: 'pending' | 'in_progress' | 'completed'
}

export interface ToolView {
  kind: ToolKind
  /** A short verb phrase: "Run", "Edit", "Read", ... */
  verb: string
  /** What it acts on: a command, a path, a pattern, a URL. */
  target: string
  /** For write and edit calls. */
  diff?: Diff
  /** Text output worth showing (stdout, a file's content, matches). */
  output?: string
  todos?: ToolTodo[]
  /** For `mcp__server__tool` calls. */
  mcp?: { server: string; tool: string }
}

const parseObject = (json: string): Record<string, unknown> | null => {
  try {
    const value = JSON.parse(json)
    return value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : null
  } catch {
    return null
  }
}

const str = (value: unknown): string => (typeof value === 'string' ? value : '')

/** Windows verbatim paths (`\\?\C:\...`) read better without the prefix. */
export const displayPath = (value: string): string =>
  value.replace(/^\\\\\?\\(UNC\\)?/, (_match, unc) => (unc ? '\\\\' : ''))

/** `path` relative to `cwd` when it is inside it. */
export function relativeTo(cwd: string | null | undefined, path: string): string {
  const clean = displayPath(path)
  if (!cwd) return clean
  const base = displayPath(cwd).replace(/[\\/]+$/, '')
  // Windows paths (a drive letter) compare without case.
  const ignoreCase = /^[a-zA-Z]:[\\/]/.test(base)
  const head = clean.slice(0, base.length)
  const matches = ignoreCase ? head.toLowerCase() === base.toLowerCase() : head === base
  if (matches && /^[\\/]/.test(clean.slice(base.length))) return clean.slice(base.length + 1)
  return clean
}

export function mcpToolName(name: string): { server: string; tool: string } | null {
  const match = /^mcp__(.+?)__(.+)$/.exec(name)
  return match ? { server: match[1], tool: match[2] } : null
}

/** The view of a call from its name and input, before any result. */
export function toolView(name: string, inputJson: string, cwd?: string | null): ToolView {
  const input = parseObject(inputJson) ?? {}
  const path = (key = 'path') => relativeTo(cwd, str(input[key]) || str(input.file_path))
  switch (name) {
    case 'bash':
    case 'PowerShell':
      return { kind: 'terminal', verb: name === 'bash' ? 'Run' : 'Run (PowerShell)', target: str(input.command) }
    case 'read_file':
      return { kind: 'read', verb: 'Read', target: path() }
    case 'write_file':
      return { kind: 'write', verb: 'Write', target: path(), diff: additionDiff(str(input.content)) }
    case 'edit_file':
      return {
        kind: 'edit',
        verb: 'Edit',
        target: path(),
        diff: diffLines(str(input.old_string), str(input.new_string)),
      }
    case 'glob_search':
      return { kind: 'search', verb: 'Find files', target: str(input.pattern) }
    case 'grep_search':
      return { kind: 'search', verb: 'Search', target: str(input.pattern) }
    case 'list_directory':
      return { kind: 'read', verb: 'List', target: path() }
    case 'WebFetch':
      return { kind: 'web', verb: 'Fetch', target: str(input.url) }
    case 'WebSearch':
      return { kind: 'web', verb: 'Search the web', target: str(input.query) }
    case 'TodoWrite': {
      const todos = Array.isArray(input.todos) ? input.todos : []
      return {
        kind: 'todo',
        verb: 'Update tasks',
        target: `${todos.length} task${todos.length === 1 ? '' : 's'}`,
        todos: todos.map((todo) => {
          const item = (todo ?? {}) as Record<string, unknown>
          const status = item.status === 'completed' || item.status === 'in_progress' ? item.status : 'pending'
          return { text: str(item.content) || str(item.activeForm), status }
        }),
      }
    }
    case 'Agent':
      return { kind: 'agent', verb: 'Agent', target: str(input.description) || str(input.subagent_type) }
    case 'Skill':
      return { kind: 'skill', verb: 'Skill', target: str(input.skill) }
    default: {
      const mcp = mcpToolName(name)
      if (mcp) return { kind: 'mcp', verb: `${mcp.server}`, target: mcp.tool, mcp }
      return { kind: 'other', verb: name, target: summarizeInput(input) }
    }
  }
}

function summarizeInput(input: Record<string, unknown>): string {
  for (const key of ['command', 'path', 'file_path', 'query', 'url', 'pattern', 'name', 'description']) {
    if (typeof input[key] === 'string' && input[key]) return input[key] as string
  }
  const text = JSON.stringify(input)
  return text === '{}' ? '' : text.length > 120 ? `${text.slice(0, 117)}…` : text
}

const MAX_OUTPUT_CHARS = 20_000

const clip = (text: string): string =>
  text.length > MAX_OUTPUT_CHARS ? `${text.slice(0, MAX_OUTPUT_CHARS)}\n… (${text.length - MAX_OUTPUT_CHARS} more characters)` : text

/** The view with the call's result folded in. */
export function withResult(view: ToolView, name: string, output: string, isError: boolean): ToolView {
  if (isError) return { ...view, output: clip(output) }
  const result = parseObject(output)
  if (!result) return { ...view, output: clip(output) }
  switch (name) {
    case 'bash':
    case 'PowerShell': {
      const parts = [str(result.stdout), str(result.stderr)].filter(Boolean)
      return { ...view, output: clip(parts.join('\n')) }
    }
    case 'read_file': {
      const file = (result.file ?? {}) as Record<string, unknown>
      return { ...view, output: clip(str(file.content)) }
    }
    case 'edit_file': {
      const diff = str(result.diff)
      return diff ? { ...view, diff: parseUnifiedDiff(diff) } : view
    }
    case 'write_file':
      return { ...view, output: str(result.summary) }
    case 'glob_search':
    case 'grep_search': {
      if (typeof result.content === 'string' && result.content) return { ...view, output: clip(result.content) }
      const files = Array.isArray(result.filenames) ? (result.filenames as unknown[]).map((f) => displayPath(str(f))) : []
      const count = typeof result.numMatches === 'number' ? `${result.numMatches} matches in ` : ''
      return { ...view, output: clip(`${count}${files.length} file${files.length === 1 ? '' : 's'}\n${files.join('\n')}`) }
    }
    default:
      return { ...view, output: clip(output) }
  }
}
