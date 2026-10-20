// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the test
// env no longer loads Codebuff's server env (packages/internal, Infisical) or
// its service placeholders; the SDK-build probe and the re-render log parsers
// (used only by the SDK perf test) are gone.
// See ui/neuron-tui/PORTING.md.
import { execSync } from 'child_process'

/**
 * Check if tmux is available and usable on the system.
 * This checks both that tmux is installed AND that it can actually run
 * (e.g., the tmux server socket directory exists and is accessible).
 *
 * Returns false on CI unless CODEBUFF_RUN_TMUX_TESTS=1 explicitly opts into
 * running deterministic tmux tests in a known terminal environment.
 */
export function isTmuxAvailable(): boolean {
  // Skip on CI - tmux integration tests need a real terminal environment
  if (
    (process.env.CI === 'true' || process.env.CI === '1') &&
    process.env.CODEBUFF_RUN_TMUX_TESTS !== '1'
  ) {
    return false
  }

  try {
    // First check if tmux is installed
    execSync('which tmux', { stdio: 'pipe' })
    // Then verify tmux can actually run by creating and killing a test session
    // This will fail if tmux server can't start (e.g., no socket directory on CI)
    execSync('tmux new-session -d -s __codebuff_tmux_check__ && tmux kill-session -t __codebuff_tmux_check__', {
      stdio: 'pipe',
      timeout: 5000,
    })
    return true
  } catch {
    return false
  }
}

/**
 * Sleep utility for async delays
 */
export const sleep = (ms: number) =>
  new Promise((resolve) => setTimeout(resolve, ms))

/** Runtime flags the UI reads under test; the preload sets the same. */
function ensureCliEnvDefaults(): void {
  if (!process.env.NODE_ENV) {
    process.env.NODE_ENV = 'test'
  }
  if (!process.env.BUN_ENV) {
    process.env.BUN_ENV = 'test'
  }
  if (process.env.CI !== 'true' && process.env.CI !== '1') {
    process.env.CI = 'true'
  }
}

export function ensureCliTestEnv(): void {
  ensureCliEnvDefaults()
}

/** The environment to hand a spawned UI process in tests. */
export function getDefaultCliEnv(): Record<string, string> {
  ensureCliEnvDefaults()
  return Object.fromEntries(
    Object.entries(process.env).filter(
      (entry): entry is [string, string] => typeof entry[1] === 'string',
    ),
  )
}
