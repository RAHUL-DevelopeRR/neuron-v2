/**
 * L5: problems with the engine itself (not the gateway). The app's Rust side
 * prefixes its error strings with `[engine:<code>]` (src-tauri/src/diagnostics.rs);
 * this reads the prefix, and builds the plain-text support report.
 */

export type EngineProblemCode = 'missing' | 'quarantined' | 'denied' | 'crashed' | 'failed' | 'restart_failed'

export interface EngineProblem {
  code: EngineProblemCode
  /** The text after the prefix: what happened and what to do. */
  message: string
  /** `crashed`: the exit code as text (`3`, or `unknown`). */
  exitCode?: string
  /** `crashed`: the last line the engine wrote to stderr. */
  lastMessage?: string
}

/** The codes the host sends, and the one the webview adds. */
const KNOWN_CODES: readonly string[] = ['missing', 'quarantined', 'denied', 'crashed', 'failed', 'restart_failed']

const PREFIX = /^\[engine:([a-z_]+)\]\s*([\s\S]*)$/

/** A host error string as a problem; `null` when it carries no (known) code. */
export function parseEngineProblem(raw: string | null | undefined): EngineProblem | null {
  const match = raw ? PREFIX.exec(raw.trim()) : null
  if (!match || !KNOWN_CODES.includes(match[1])) return null
  const problem: EngineProblem = { code: match[1] as EngineProblemCode, message: match[2] }
  if (problem.code === 'crashed') {
    const exit = /\(exit code ([^)]+)\)/.exec(match[2])
    const last = /Last message: ([\s\S]*)$/.exec(match[2])
    if (exit) problem.exitCode = exit[1]
    if (last) problem.lastMessage = last[1]
  }
  return problem
}

/**
 * The problem for an error string. After a restart, an unnamed failure (or the
 * generic `failed`) is `restart_failed`; a named reason (blocked, missing...) stays.
 */
export function engineProblemFor(raw: string | null | undefined, options: { restart?: boolean } = {}): EngineProblem | null {
  const parsed = parseEngineProblem(raw)
  if (!options.restart) return parsed
  if (parsed && parsed.code !== 'failed') return parsed
  return { code: 'restart_failed', message: parsed?.message ?? (raw ?? '').trim() }
}

/** Takes emails and key-like strings out of free text (an engine error can quote them). */
export function scrub(text: string): string {
  return text
    .replace(/[A-Za-z]:[\\/]Users[\\/][^\\/\s"']+/gi, '~')
    .replace(/(?<![\w.-])\/(?:Users|home)\/[^/\s"']+/g, '~')
    .replace(/\b([a-z][a-z0-9+.-]*:\/\/)[^\s/@:]+(?::[^\s/@]*)?@/gi, '$1[removed]@')
    .replace(/\bBearer\s+[\w.~+/=-]{8,}/gi, 'Bearer [removed]')
    .replace(/\b([\w-]*(?:key|token|secret|password|passwd|authorization))(\s*[=:]\s*)("[^"]*"|'[^']*'|\S+)/gi, '$1$2[removed]')
    .replace(/\b(?:AKIA|ASIA)[0-9A-Z]{16}\b/g, '[removed]')
    .replace(/[\w.+-]+@[\w-]+(?:\.[\w-]+)+/g, '[email]')
    .replace(/\b(?:sk|pk|acct)[-_][\w-]{4,}/gi, '[removed]')
    .replace(/\b[A-Za-z0-9_-]{32,}\b/g, '[removed]')
}

export interface DiagnosticsInput {
  appVersion: string | null
  /** The host's lines (OS, engine source and path); `null` in a browser build. */
  hostReport: string | null
  engine: { version: string | null; protocol: number | null; features: string[] }
  connection: string
  lastError: string | null
  /** `manual` (the releases page), `signed` (in-app updates) or `none`. */
  updates: 'manual' | 'signed' | 'none'
}

/**
 * The plain-text report Settings and the problem cards copy. Built from its
 * input only: no account, no environment, no file contents.
 */
export function buildDiagnostics(input: DiagnosticsInput): string {
  const lines = [`Neuron Desktop ${input.appVersion ?? 'dev'}`]
  lines.push(input.hostReport?.trim() || `App version: ${input.appVersion ?? 'dev'}`)
  lines.push(
    `Engine version: ${input.engine.version ?? 'not connected'}`,
    `Engine protocol: ${input.engine.protocol ?? 'unknown'}`,
    `Engine features: ${input.engine.features.join(', ') || 'none'}`,
    `Connection: ${input.connection}`,
    `Last engine error: ${input.lastError || 'none'}`,
    `Updates: ${input.updates}`,
  )
  // The whole report, the host's lines included, can quote a path, a URL or a key.
  return scrub(lines.join('\n'))
}
