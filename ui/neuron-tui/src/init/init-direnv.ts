// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
/**
 * Direnv initialization - loads environment variables from .envrc at CLI startup.
 */

import { spawnSync } from 'child_process'
import fs from 'fs'
import os from 'os'
import path from 'path'

import { logger } from '../utils/logger'

/**
 * Search up the directory tree for .envrc, stopping at git root.
 * @internal
 */
export function findEnvrcDirectory(startDir: string): string | null {
  let currentDir = path.resolve(startDir)
  const root = path.parse(currentDir).root

  while (currentDir !== root) {
    // Read directory entries once and check for both .envrc and .git
    let entries: string[]
    try {
      entries = fs.readdirSync(currentDir)
    } catch {
      // Directory not readable - stop searching
      break
    }

    const hasEnvrc = entries.includes('.envrc')
    const hasGit = entries.includes('.git')

    if (hasEnvrc) {
      return currentDir
    }

    // If this is a git root and no .envrc found, stop searching
    if (hasGit) {
      break
    }

    const parentDir = path.dirname(currentDir)
    if (parentDir === currentDir) break
    currentDir = parentDir
  }

  return null
}

/** @internal */
export function isDirenvAvailable(): boolean {
  if (os.platform() === 'win32') {
    return false
  }

  try {
    const result = spawnSync('sh', ['-c', 'command -v direnv'], {
      encoding: 'utf-8',
      timeout: 2000,
    })
    return result.status === 0 && result.stdout.trim().length > 0
  } catch {
    return false
  }
}

/** @internal */
export function getDirenvExport(envrcDir: string): Record<string, string | null> | null {
  try {
    const result = spawnSync('direnv', ['export', 'json'], {
      cwd: envrcDir,
      encoding: 'utf-8',
      timeout: 10000,
      env: { ...process.env, DIRENV_LOG_FORMAT: '' },
    })

    if (result.status !== 0) {
      if (result.stderr?.includes('is blocked')) {
        logger.warn(
          'direnv: .envrc is blocked. Run `direnv allow` to enable.',
        )
      }
      return null
    }

    const output = result.stdout.trim()
    if (!output) {
      return null
    }

    const envVars = JSON.parse(output) as Record<string, string | null>
    return envVars
  } catch (error) {
    logger.debug(
      { error: error instanceof Error ? error.message : String(error) },
      'Failed to run direnv export',
    )
    return null
  }
}

/**
 * Variables a repository's `.envrc` may NOT set on the UI's own process.
 *
 * The direnv import exists so the agent's terminal commands see the user's
 * project environment. But it runs at startup, into the same `process.env`
 * the UI reads its configuration from and starts the engine with. So a cloned
 * repo could otherwise:
 * - run its own program as the engine (`NEURON_BIN`), or as an MCP server
 *   (`NEURON_MCP_NODE`, `NEURON_*_MCP_ENTRY`);
 * - send the Zero-X session to another host (`NEURON_API_BASE`), swap the
 *   login (`NEURON_TOKEN`), or trust its own workspace
 *   (`NEURON_TRUST_WORKSPACE`);
 * - route prompts, and the code in them, to an account or endpoint it controls
 *   (`ANTHROPIC_API_KEY`, `OPENAI_BASE_URL`, ...);
 * - point the engine at a config home it wrote (`CLAW_CONFIG_HOME`, `HOME`,
 *   `USERPROFILE`), whose settings load as the user's own, always trusted,
 *   with hooks, MCP servers, full access and `trustedRoots`;
 * - add git config at global or command scope (`GIT_CONFIG_GLOBAL`,
 *   `GIT_CONFIG_COUNT`, `XDG_CONFIG_HOME`, `HOME`, ...), such as an fsmonitor
 *   that a read-only `git status` would run without asking;
 * - preload code into Node or Bun (`NODE_OPTIONS`), or make a MITM proxy
 *   trusted (`NODE_EXTRA_CA_CERTS`, `NODE_TLS_REJECT_UNAUTHORIZED`).
 * `direnv allow` is consent to the repo's env for the repo's tooling, not to
 * reconfigure the agent that is about to run in it. Anything matching here is
 * dropped from the import and named in one warning; the user's real shell
 * environment is untouched, so setting these deliberately still works. (A
 * `PATH` change is imported, but the engine binary is looked up on the PATH
 * from before the import: see engine/startup-env.ts.)
 *
 * Prefix matches are deliberate: every Neuron knob (the engine's and this
 * UI's `NEURON_TUI_*`) and every provider's lives under one of these
 * prefixes, so a new knob needs no new entry here to be protected.
 * @internal
 */
export const STEERING_ENV_VAR_PATTERNS: readonly RegExp[] = [
  /^NEURON_/,
  // The providers the engine can use: credentials, endpoints, models.
  /^(ANTHROPIC|OPENAI|AZURE_OPENAI|XAI|GROK|DASHSCOPE|OPENROUTER|MISTRAL|CLAWD)_/,
  // The engine's compatibility knobs (`CLAW_CONFIG_HOME` picks its settings home).
  /^CLAW_/,
  // More compatibility knobs the engine reads: `RUSTY_CLAUDE_PERMISSION_MODE` sets its
  // default permission mode (full access included); `CLAUDE_CONFIG_DIR` and
  // `CLAUDE_CONFIG_HOME` add config, skill and agent roots.
  /^RUSTY_CLAUDE_/,
  /^CLAUDE_/,
  // Home and config directories: the engine's user settings (~/.neuron) and git's
  // global config (~/.gitconfig, $XDG_CONFIG_HOME/git/config) live under them.
  /^HOME$/,
  /^USERPROFILE$/,
  /^XDG_CONFIG_HOME$/,
  // Git config at global, system or command scope (GIT_CONFIG_GLOBAL, GIT_CONFIG_COUNT, ...).
  /^GIT_CONFIG/,
  /^NEXT_PUBLIC_/,
  /^OVERRIDE_(TARGET|PLATFORM|ARCH)$/,
  /^NODE_OPTIONS$/,
  /^NODE_EXTRA_CA_CERTS$/,
  /^NODE_TLS_REJECT_UNAUTHORIZED$/,
  /^SSL_CERT_(FILE|DIR)$/,
  /^BUN_(CONFIG|OPTIONS|INSTALL)/,
  /^LD_(PRELOAD|LIBRARY_PATH)$/,
  /^DYLD_/,
]

/** @internal */
export function isSteeringEnvVar(key: string): boolean {
  return STEERING_ENV_VAR_PATTERNS.some((re) => re.test(key))
}

/** Load direnv environment into process.env. Safe to call even if direnv is not installed. */
export function initializeDirenv(): void {
  if (!isDirenvAvailable()) {
    return
  }

  const envrcDir = findEnvrcDirectory(process.cwd())
  if (!envrcDir) {
    return
  }

  const envVars = getDirenvExport(envrcDir)
  if (!envVars) {
    return
  }
  let appliedCount = 0
  const dropped: string[] = []
  for (const [key, value] of Object.entries(envVars)) {
    if (isSteeringEnvVar(key)) {
      dropped.push(key)
      continue
    }
    if (value === null) {
      delete process.env[key]
    } else {
      process.env[key] = value
    }
    appliedCount++
  }

  if (dropped.length > 0) {
    logger.warn(
      { envrcDir, dropped },
      `direnv: ignored ${dropped.length} variable(s) from .envrc that would reconfigure the CLI itself (${dropped.join(', ')}). Set them in your shell instead.`,
    )
  }

  if (appliedCount > 0) {
    logger.debug(
      { envrcDir, variableCount: appliedCount },
      'Loaded environment variables from direnv',
    )
  }
}
