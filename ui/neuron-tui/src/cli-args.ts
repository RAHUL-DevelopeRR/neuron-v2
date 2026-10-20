// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the
// Freebuff variant and the login/publish commands, --agent and --trust-agents
// (Codebuff login, agent store and repository .agents loading) are gone, and
// the engine's launch flags (--model, --provider, ...) pass through to it.
// See ui/neuron-tui/PORTING.md.
import { createRequire } from 'module'

import { Command } from 'commander'

import type { EngineLaunchOptions } from './engine/neuron-engine-client'
import type { AgentMode } from './utils/constants'
import { getCliEnv } from './utils/env'

const require = createRequire(import.meta.url)

export type ParsedArgs = {
  initialPrompt: string | null
  clearLogs: boolean
  continue: boolean
  continueId?: string | null
  cwd?: string
  initialMode?: AgentMode
  /** Flags for `neuron engine --stdio`; the engine checks them. */
  engine: Pick<
    EngineLaunchOptions,
    | 'model'
    | 'provider'
    | 'permissionMode'
    | 'allowBroadCwd'
    | 'dangerouslySkipPermissions'
  >
}

export function loadPackageVersion(): string {
  const env = getCliEnv()
  if (env.NEURON_TUI_VERSION) {
    return env.NEURON_TUI_VERSION
  }

  try {
    const pkg = require('../package.json') as { version?: string }
    if (pkg.version) {
      return pkg.version
    }
  } catch {
    // Continue to dev fallback
  }

  return 'dev'
}

export function parseArgs({
  argv = process.argv,
  version = loadPackageVersion(),
}: {
  argv?: string[]
  version?: string
} = {}): ParsedArgs {
  const program = new Command()

  program
    .name('neuron')
    .description('Neuron: an AI coding agent in your terminal')
    .version(version, '-v, --version', 'Print the CLI version')
    .option(
      '--clear-logs',
      'Remove any existing CLI log files before starting',
    )
    .option(
      '--continue [conversation-id]',
      'Continue from a previous conversation (optionally specify a conversation id)',
    )
    .option(
      '--cwd <directory>',
      'Set the working directory (default: current directory)',
    )
    .option('--lite', 'Start in LITE mode')
    .option('--free', 'Start in LITE mode (deprecated alias)')
    .option('--max', 'Start in MAX mode')
    .option('--plan', 'Start in PLAN mode')
    .option('--model <model>', 'Model for the session (an alias like opus, or a full name)')
    .option('--provider <name>', 'Provider to use instead of the configured one')
    .option(
      '--permission-mode <mode>',
      'read-only, workspace-write or danger-full-access',
    )
    .option(
      '--allow-broad-cwd',
      'Run from a very broad folder (such as your home folder)',
    )
    .option(
      '--dangerously-skip-permissions',
      'Run every tool call without asking',
    )
    .helpOption('-h, --help', 'Show this help message')
    .argument('[prompt...]', 'Initial prompt to send to the agent')
    .allowExcessArguments(true)

  program.parse(argv)

  const options = program.opts()
  const args = program.args

  const continueFlag = options.continue

  // Determine initial mode from flags (last flag wins if multiple specified)
  let initialMode: AgentMode | undefined
  if (options.free || options.lite) initialMode = 'LITE'
  if (options.max) initialMode = 'MAX'
  if (options.plan) initialMode = 'PLAN'

  return {
    initialPrompt: args.length > 0 ? args.join(' ') : null,
    clearLogs: options.clearLogs || false,
    continue: Boolean(continueFlag),
    continueId:
      typeof continueFlag === 'string' && continueFlag.trim().length > 0
        ? continueFlag.trim()
        : null,
    cwd: options.cwd,
    initialMode,
    engine: {
      model: options.model,
      provider: options.provider,
      permissionMode: options.permissionMode,
      allowBroadCwd: options.allowBroadCwd || undefined,
      dangerouslySkipPermissions: options.dangerouslySkipPermissions || undefined,
    },
  }
}
