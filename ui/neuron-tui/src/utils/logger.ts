// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
import { appendFileSync, existsSync, mkdirSync, unlinkSync } from 'fs'
import path, { dirname } from 'path'
import { format as stringFormat } from 'util'

import { pino } from 'pino'

import { getCurrentChatDir, getProjectRoot } from '../project-files'
import { IS_CI, IS_DEV, IS_TEST } from './runtime-env'

/** Name of the per-chat debug log file written in production builds */
export const CHAT_LOG_FILENAME = 'log.jsonl'

export interface LoggerContext {
  clientSessionId?: string
  [key: string]: any // Allow for future extensions
}

export const loggerContext: LoggerContext = {}

let logPath: string | undefined = undefined
let pinoLogger: any = undefined

const loggingLevels = ['info', 'debug', 'warn', 'error', 'fatal'] as const
type LogLevel = (typeof loggingLevels)[number]

/**
 * Safely stringify an object, handling circular references.
 * Replaces circular references with '[Circular]' placeholder.
 */
function safeStringify(obj: unknown): string {
  const seen = new WeakSet()
  return JSON.stringify(obj, (_key, value) => {
    if (typeof value === 'object' && value !== null) {
      if (seen.has(value)) {
        return '[Circular]'
      }
      seen.add(value)
    }
    return value
  })
}

function isEmptyObject(value: any): boolean {
  return (
    value != null &&
    typeof value === 'object' &&
    !Array.isArray(value) &&
    Object.keys(value).length === 0
  )
}

function setLogPath(p: string): void {
  if (p === logPath) return // nothing to do

  logPath = p
  mkdirSync(dirname(p), { recursive: true })

  // ──────────────────────────────────────────────────────────────
  //  pino.destination(..) → SonicBoom stream, no worker thread
  // ──────────────────────────────────────────────────────────────
  const fileStream = pino.destination({
    dest: p, // absolute or relative file path
    mkdir: true, // create parent dirs if they don’t exist
    sync: true, // set true if you *must* block on every write
  })

  pinoLogger = pino(
    {
      level: 'debug',
      formatters: {
        level: (label) => ({ level: label.toUpperCase() }),
      },
      timestamp: () => `,"timestamp":"${new Date().toISOString()}"`,
    },
    fileStream, // <-- no worker thread involved
  )
}

export function clearLogFile(): void {
  const projectRoot = getProjectRoot()
  const debugDir = path.join(projectRoot, 'debug')
  const targets = new Set<string>()

  if (logPath) {
    targets.add(logPath)
  }
  targets.add(path.join(debugDir, 'cli.jsonl'))
  targets.add(path.join(debugDir, 'trace.jsonl'))

  for (const target of targets) {
    try {
      if (existsSync(target)) {
        unlinkSync(target)
      }
    } catch {
      // Ignore errors when clearing logs
    }
  }

  logPath = undefined
  pinoLogger = undefined
}

function writeLog(
  level: LogLevel,
  data: any,
  msg?: string,
  ...args: any[]
): void {
  if (!IS_CI && !IS_TEST) {
    let projectRoot: string | undefined
    try {
      projectRoot = getProjectRoot()
    } catch {
      projectRoot = undefined
    }
    if (projectRoot) {
      const logTarget =
        IS_DEV
          ? path.join(projectRoot, 'debug', 'cli.jsonl')
          : path.join(getCurrentChatDir(), CHAT_LOG_FILENAME)

      setLogPath(logTarget)
    }
  }

  const isStringOnly = typeof data === 'string' && msg === undefined
  const normalizedData = isStringOnly ? undefined : data
  const normalizedMsg = isStringOnly ? (data as string) : msg
  const includeData = normalizedData != null && !isEmptyObject(normalizedData)

  // In dev mode, use appendFileSync for real-time logging (Bun has issues with pino sync)
  // In prod mode, use pino for better performance
  if (IS_DEV && logPath) {
    const logEntry = safeStringify({
      level: level.toUpperCase(),
      timestamp: new Date().toISOString(),
      ...loggerContext,
      ...(includeData ? { data: normalizedData } : {}),
      msg: stringFormat(normalizedMsg ?? '', ...args),
    })
    try {
      appendFileSync(logPath, logEntry + '\n')
    } catch {
      // Ignore write errors
    }
  } else if (pinoLogger !== undefined) {
    const base = { ...loggerContext }
    const obj = includeData ? { ...base, data: normalizedData } : base
    pinoLogger[level](obj, normalizedMsg as any, ...args)
  }
}

/**
 * Wrapper around Pino logger. Writes to a local file only: the chat's
 * log.jsonl, or <project>/debug/cli.jsonl when NEURON_TUI_ENV=dev.
 */
export const logger: Record<LogLevel, pino.LogFn> = Object.fromEntries(
  loggingLevels.map((level) => {
    return [
      level,
      (data: any, msg?: string, ...args: any[]) =>
        writeLog(level, data, msg, ...args),
    ]
  }),
) as Record<LogLevel, pino.LogFn>
