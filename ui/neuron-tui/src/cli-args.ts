// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the
// Freebuff variant and the login/publish commands, --agent and --trust-agents
// (Codebuff login, agent store and repository .agents loading) are gone.
// See ui/neuron-tui/PORTING.md.
import { createRequire } from 'module'

import { Command } from 'commander'

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
}

export function loadPackageVersion(): string {
  const env = getCliEnv()
  if (env.CODEBUFF_CLI_VERSION) {
    return env.CODEBUFF_CLI_VERSION
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
    .name('codebuff')
    .description('Codebuff CLI - AI-powered coding assistant')
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
  }
}
