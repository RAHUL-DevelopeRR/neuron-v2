/**
 * Neuron's tool calls in the shapes Freebuff's tool renderers read
 * (src/components/tools/registry.ts). The engine reports a call as its tool
 * name and JSON input, and a result as the tool's raw output (usually the JSON
 * of its Rust output struct); this module turns both into the renderer's
 * tool name, input and `ToolResultOutput[]`. Tools without a renderer keep
 * their own name and the generic tool row.
 */

import { toTodoStatus } from '../utils/todos'

import type { ToolResultOutput } from '@neuron-tui/common/types/messages/content-part'

export interface MappedToolCall {
  /** The renderer's tool name (a Freebuff tool name, or Neuron's own). */
  toolName: string
  input: Record<string, unknown>
}

const TERMINAL_TOOLS = new Set(['bash', 'PowerShell'])

/** Neuron tool → Freebuff renderer. */
export const NEURON_TO_RENDERER_TOOL: Record<string, string> = {
  bash: 'run_terminal_command',
  PowerShell: 'run_terminal_command',
  read_file: 'read_files',
  write_file: 'write_file',
  edit_file: 'str_replace',
  glob_search: 'glob',
  grep_search: 'code_search',
  WebFetch: 'read_url',
  WebSearch: 'web_search',
  TodoWrite: 'write_todos',
  Skill: 'skill',
}

/** Windows verbatim paths (`\\?\C:\...`) read better without the prefix. */
export const displayPath = (value: string): string =>
  value.replace(/^\\\\\?\\(UNC\\)?/, (_match, unc) => (unc ? '\\\\' : ''))

/**
 * A result's file path as the call named it: relative to the workspace (with
 * `/`) when it is inside `cwd`, so a row keeps the path it showed while the
 * model wrote the call. Elsewhere, or without a `cwd`, as {@link displayPath}.
 */
export function workspacePath(value: string, cwd?: string | null): string {
  const path = displayPath(value)
  if (!cwd) return path
  const windows = /^[A-Za-z]:[\\/]/.test(path) || path.includes('\\')
  const normalize = (text: string) => {
    const slashed = text.replace(/\\/g, '/')
    return windows ? slashed.toLowerCase() : slashed
  }
  const root = normalize(displayPath(cwd)).replace(/\/+$/, '') + '/'
  if (!normalize(path).startsWith(root)) return path
  return path.slice(root.length).replace(/\\/g, '/') || path
}

const parseObject = (json: string): Record<string, unknown> | null => {
  try {
    const value = JSON.parse(json)
    return value && typeof value === 'object' && !Array.isArray(value)
      ? (value as Record<string, unknown>)
      : null
  } catch {
    return null
  }
}

const str = (value: unknown): string | undefined =>
  typeof value === 'string' ? value : undefined

export function mapToolCall(name: string, inputJson: string): MappedToolCall {
  const input = parseObject(inputJson) ?? (inputJson ? { input: inputJson } : {})
  const toolName = NEURON_TO_RENDERER_TOOL[name] ?? name
  switch (name) {
    case 'bash':
    case 'PowerShell': {
      const timeoutMs = typeof input.timeout === 'number' ? input.timeout : undefined
      return {
        toolName,
        input: {
          command: str(input.command) ?? '',
          ...(timeoutMs !== undefined && {
            timeout_seconds: Math.round(timeoutMs / 1000),
          }),
        },
      }
    }
    case 'read_file':
      return { toolName, input: { paths: [str(input.path) ?? ''] } }
    case 'write_file':
      return {
        toolName,
        input: { path: str(input.path) ?? '', content: str(input.content) ?? '' },
      }
    case 'edit_file':
      return {
        toolName,
        input: {
          path: str(input.path) ?? '',
          replacements: [
            {
              old: str(input.old_string) ?? '',
              new: str(input.new_string) ?? '',
              allowMultiple: input.replace_all === true,
            },
          ],
        },
      }
    case 'glob_search':
      return {
        toolName,
        input: { pattern: str(input.pattern) ?? '', cwd: str(input.path) ?? '' },
      }
    case 'grep_search':
      return {
        toolName,
        input: {
          pattern: str(input.pattern) ?? '',
          cwd: str(input.path) ?? '',
          ...(str(input.glob) && { flags: `-g ${input.glob}` }),
        },
      }
    case 'WebFetch':
      return { toolName, input: { url: str(input.url) ?? '' } }
    case 'WebSearch':
      return { toolName, input: { query: str(input.query) ?? '' } }
    case 'TodoWrite': {
      const todos = Array.isArray(input.todos) ? input.todos : []
      return {
        toolName,
        input: {
          todos: todos.map((todo) => {
            const item = (todo ?? {}) as Record<string, unknown>
            const activeForm = str(item.activeForm)
            return {
              task: str(item.content) ?? activeForm ?? '',
              completed: item.status === 'completed',
              // The current item is drawn apart, in its present tense.
              status: toTodoStatus(item.status),
              ...(activeForm && { activeForm }),
            }
          }),
        },
      }
    }
    case 'Skill':
      return { toolName, input: { name: str(input.skill) ?? '' } }
    default:
      return { toolName, input }
  }
}

const text = (value: string): ToolResultOutput[] =>
  // formatToolOutput renders `text` items verbatim; the shared schema only
  // names `json` and `media`, so the cast is deliberate.
  [{ type: 'text', text: value } as unknown as ToolResultOutput]

const json = (value: Record<string, unknown>): ToolResultOutput[] => [
  { type: 'json', value } as ToolResultOutput,
]

/** How a command ended when it did not succeed, for its row's "✗ failed (…)". */
export interface CommandExit {
  /** The exit code of a command that ran to the end. */
  exitCode?: number
  /** Why it stopped otherwise: "timed out", "killed", "interrupted", "blocked: …". */
  stopReason?: string
}

/**
 * The engine's `returnCodeInterpretation` (set only for a command that did not
 * succeed: `exit_code:<n>`, `timeout`, `cancelled`, `killed`, `signal:<n>`,
 * `preflight_blocked:<why>`) and `interrupted` flag, in words.
 */
export function commandExitOf(result: Record<string, unknown>): CommandExit {
  const interpretation = str(result.returnCodeInterpretation)?.trim()
  if (interpretation) {
    const code = /^exit_code:(-?\d+)$/.exec(interpretation)
    if (code) return { exitCode: Number(code[1]) }
    if (interpretation === 'timeout') return { stopReason: 'timed out' }
    if (interpretation === 'cancelled') return { stopReason: 'interrupted' }
    if (interpretation === 'killed') return { stopReason: 'killed' }
    const signal = /^signal:(\d+)$/.exec(interpretation)
    if (signal) return { stopReason: `killed (signal ${signal[1]})` }
    const blocked = /^preflight_blocked(?::(.*))?$/.exec(interpretation)
    if (blocked) {
      const why = blocked[1]?.replace(/_/g, ' ').trim()
      return { stopReason: why ? `blocked: ${why}` : 'blocked' }
    }
    return { stopReason: interpretation.replace(/_/g, ' ') }
  }
  // A plain exit code field, from a result shaped otherwise.
  for (const key of ['exitCode', 'exit_code']) {
    const value = result[key]
    if (typeof value === 'number' && value !== 0) return { exitCode: value }
  }
  if (result.interrupted === true) return { stopReason: 'interrupted' }
  return {}
}

export function mapToolResult(
  name: string,
  output: string,
  isError: boolean,
  /** The session's workspace, so file results read relative to it. */
  cwd?: string | null,
): ToolResultOutput[] {
  if (TERMINAL_TOOLS.has(name)) {
    // A command that ran and failed (non-zero exit, timeout) still has its
    // output to show; the block's error flag marks it failed, and its exit
    // code or why it stopped says how.
    const parsed = parseObject(output)
    if (parsed && (typeof parsed.stdout === 'string' || typeof parsed.stderr === 'string')) {
      return json({
        stdout: str(parsed.stdout) ?? '',
        stderr: str(parsed.stderr) ?? '',
        ...commandExitOf(parsed),
      })
    }
    if (isError) return json({ errorMessage: output })
    return json({ stdout: output })
  }

  if (name === 'Agent') {
    // The sub-agent's final text, or how it ended without one, not its
    // manifest. Its status and note reach the row from the turn
    // (live-tool-blocks.ts).
    const parsed = parseObject(output)
    if (parsed) {
      const shown =
        str(parsed.result) ??
        str(parsed.note) ??
        str(parsed.error) ??
        `Sub-agent ${str(parsed.status) ?? 'finished'}.`
      return text(isError ? `Error: ${shown}` : shown)
    }
  }

  if (isError) return text(`Error: ${output}`)
  const parsed = parseObject(output)
  if (!parsed) return text(output)

  switch (name) {
    case 'write_file': {
      const file = workspacePath(str(parsed.filePath) ?? '', cwd)
      return json({
        message: parsed.type === 'create' ? `Created new file ${file}` : `Updated file ${file}`,
        file,
        // What was written, for the row's summary; a diff when the engine
        // sends one.
        ...(typeof parsed.lines === 'number' && { lines: parsed.lines }),
        ...(typeof parsed.bytes === 'number' && { bytes: parsed.bytes }),
        ...(str(parsed.diff) && { unifiedDiff: parsed.diff }),
      })
    }

    case 'edit_file': {
      const file = workspacePath(str(parsed.filePath) ?? '', cwd)
      return json({
        message: `Updated file ${file}`,
        file,
        ...(str(parsed.diff) && { unifiedDiff: parsed.diff }),
      })
    }
    case 'read_file': {
      const file = (parsed.file ?? {}) as Record<string, unknown>
      return text(str(file.content) ?? output)
    }
    case 'glob_search': {
      const names = Array.isArray(parsed.filenames) ? parsed.filenames : []
      return text(names.map((n) => displayPath(String(n))).join('\n'))
    }
    case 'grep_search': {
      const matches = typeof parsed.numMatches === 'number' ? parsed.numMatches : undefined
      const body =
        str(parsed.content) ??
        (Array.isArray(parsed.filenames)
          ? parsed.filenames.map((n) => displayPath(String(n))).join('\n')
          : '')
      return text(
        matches !== undefined
          ? `Found ${matches} ${matches === 1 ? 'match' : 'matches'}\n${body}`
          : body,
      )
    }
    default:
      return json(parsed)
  }
}

/** The result shown for a call that ended without one (denied, blocked, cut off). */
export const NO_RESULT_MESSAGE =
  'No result: the call was denied, blocked by policy, or did not run.'
