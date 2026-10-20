/**
 * Neuron's tool calls in the shapes Freebuff's tool renderers read
 * (src/components/tools/registry.ts). The engine reports a call as its tool
 * name and JSON input, and a result as the tool's raw output (usually the JSON
 * of its Rust output struct); this module turns both into the renderer's
 * tool name, input and `ToolResultOutput[]`. Tools without a renderer keep
 * their own name and the generic tool row.
 */

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
            return {
              task: str(item.content) ?? str(item.activeForm) ?? '',
              completed: item.status === 'completed',
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

export function mapToolResult(
  name: string,
  output: string,
  isError: boolean,
): ToolResultOutput[] {
  if (TERMINAL_TOOLS.has(name)) {
    if (isError) return json({ errorMessage: output })
    const parsed = parseObject(output)
    if (parsed && (typeof parsed.stdout === 'string' || typeof parsed.stderr === 'string')) {
      return json({
        stdout: str(parsed.stdout) ?? '',
        stderr: str(parsed.stderr) ?? '',
      })
    }
    return json({ stdout: output })
  }

  if (isError) return text(`Error: ${output}`)
  const parsed = parseObject(output)
  if (!parsed) return text(output)

  switch (name) {
    case 'write_file': {
      const file = displayPath(str(parsed.filePath) ?? '')
      return json({
        message: parsed.type === 'create' ? `Created new file ${file}` : `Updated file ${file}`,
        file,
      })
    }
    case 'edit_file': {
      const file = displayPath(str(parsed.filePath) ?? '')
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
