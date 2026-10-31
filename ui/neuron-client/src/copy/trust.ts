/**
 * The words of the folder trust screen, shared by the terminal UI ("Before
 * Neuron works here", T1) and the desktop modal (D9), so both say the same
 * thing. Neuron's own wording: keep it plain, and keep both surfaces on these
 * strings rather than their own.
 *
 * {@link trustPreviewLines} turns the engine's `trust_preview` event into the
 * "This project also wants to turn on" list.
 */

import type { TrustPreviewEvent } from '../protocol'

/** Where "Why this matters" points. */
export const TRUST_DOCS_URL = 'https://zero-x.live/docs/trust'

export const trustCopy = {
  title: 'Before Neuron works here',
  /** What trusting allows, before the list of what the project adds. */
  access: 'Neuron will be able to open, change and run files in this folder.',
  turnsOnHeading: 'This project also wants to turn on:',
  turnsOnNothing: 'This project does not turn on any hooks, MCP servers or permission rules.',
  caution: 'Continue only if you created this project or trust where it came from.',
  whyLabel: 'Why this matters:',
  /** `TRUST_DOCS_URL` as it is shown. */
  whyLink: 'zero-x.live/docs/trust',
  /** The home folder is trusted for one session at a time. */
  sessionOnly: 'This is your home folder, so Neuron trusts it for this session only and asks again next time.',
  allow: 'Allow Neuron in this folder',
  /** The terminal's second choice. */
  quit: 'Quit',
  /** The desktop's second choice. */
  openOther: 'Open a different folder',
  /** The terminal's key hint under the two choices. */
  terminalKeys: '↑↓ or 1–2 to choose · Enter to confirm · Esc to quit',
  /** Shown while the engine restarts with the folder's settings. */
  restarting: 'Starting Neuron with this folder’s settings…',
  /** When recording trust failed; the engine's error follows. */
  failed: 'Neuron could not save that this folder is trusted.',
} as const

/** One choice on the trust screen. `allow` trusts the folder. */
export interface TrustChoice {
  id: 'allow' | 'quit' | 'open_other'
  label: string
}

/** The two choices, in order: the terminal quits, the desktop offers another folder. */
export function trustChoices(surface: 'terminal' | 'desktop'): TrustChoice[] {
  return [
    { id: 'allow', label: trustCopy.allow },
    surface === 'terminal'
      ? { id: 'quit', label: trustCopy.quit }
      : { id: 'open_other', label: trustCopy.openOther },
  ]
}

/**
 * The note beside the folder's path: which folder trust is recorded for.
 * Empty outside git, where it is the folder itself.
 */
export function trustRootNote(preview: Pick<TrustPreviewEvent, 'cwd' | 'root' | 'is_git_root'>): string {
  if (!preview.is_git_root) return ''
  return samePath(preview.root, preview.cwd) ? '(git root: this folder)' : `(git root: ${preview.root})`
}

const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`

/** `path` relative to `base` when it is inside it, with forward slashes. */
function relativeTo(base: string, path: string): string {
  const normal = (value: string) => value.replace(/\\/g, '/').replace(/\/+$/, '')
  const root = normal(base)
  const full = normal(path)
  return full.toLowerCase().startsWith(root.toLowerCase() + '/') ? full.slice(root.length + 1) : full
}

function samePath(left: string, right: string): boolean {
  const normal = (value: string) => value.replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase()
  return normal(left) === normal(right)
}

/** "from .neuron/settings.json", or "from 2 settings files". */
function fromFiles(preview: Pick<TrustPreviewEvent, 'cwd'>, sources: string[]): string {
  const files = [...new Set(sources)]
  if (files.length === 1) return `from ${relativeTo(preview.cwd, files[0]!)}`
  return `from ${files.length} settings files`
}

/**
 * What trusting the folder would turn on, one line each, for the list under
 * {@link trustCopy.turnsOnHeading}. Empty when the project adds nothing (show
 * {@link trustCopy.turnsOnNothing} instead).
 */
export function trustPreviewLines(preview: TrustPreviewEvent): string[] {
  const lines: string[] = []
  if (preview.hooks.length > 0) {
    const verb = preview.hooks.length === 1 ? 'runs' : 'run'
    lines.push(`${plural(preview.hooks.length, 'hook that', 'hooks that')} ${verb} commands around tool calls`)
  }
  if (preview.mcp_servers.length > 0) {
    const names = preview.mcp_servers.map((server) => server.name).join(', ')
    lines.push(`${plural(preview.mcp_servers.length, 'MCP server', 'MCP servers')}: ${names}`)
  }
  if (preview.permission_rules.length > 0) {
    const sources = preview.permission_rules.map((rule) => rule.source)
    lines.push(
      `${plural(preview.permission_rules.length, 'permission rule', 'permission rules')} ${fromFiles(preview, sources)}`,
    )
  }
  if (preview.other_settings.length > 0) {
    lines.push(`Other settings: ${preview.other_settings.join(', ')}`)
  }
  return lines
}
