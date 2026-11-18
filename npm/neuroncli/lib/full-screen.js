// Whether `neuron <args>` will open Neuron's full-screen UI, so the launcher
// knows to have the `neuron-tui` binary ready. The native `neuron` makes the
// final call (it starts the UI only in place of its line REPL); this mirrors
// it closely enough to avoid fetching the UI for a prompt or a subcommand.
// It also tells the UI how this copy was installed (withInstallMethod).

// The flags whose value is the next argument (args.rs VALUE_FLAGS, plus --provider).
const VALUE_FLAGS = new Set([
  '--model',
  '--provider',
  '--output-format',
  '--permission-mode',
  '--base-commit',
  '--reasoning-effort',
  '--allowedTools',
  '--allowed-tools',
]);

export function isTruthy(value) {
  const normalized = String(value ?? '').trim().toLowerCase();
  return normalized !== '' && normalized !== '0' && normalized !== 'false' && normalized !== 'no';
}

/** On a terminal, not turned off (`--no-tui`, NEURON_NO_TUI), and only options: no prompt or subcommand. */
export function wantsFullScreenUi(argv, {env = process.env, interactive}) {
  if (!interactive || isTruthy(env.NEURON_NO_TUI)) return false;
  for (let index = 0; index < argv.length; index++) {
    const arg = argv[index];
    if (arg === '--no-tui' || arg === '--help' || arg === '-h' || arg === '--version' || arg === '-V') return false;
    if (VALUE_FLAGS.has(arg)) {
      index++;
      continue;
    }
    if (arg.startsWith('-') && arg !== '-' && arg !== '-p' && arg !== '--') continue;
    return false;
  }
  return true;
}

/**
 * `env` with NEURON_INSTALL_METHOD=npm, so the full-screen UI names the npm
 * update command: its binary may sit in the cache the pip launcher shares
 * (when the package folder is read-only), where its path alone reads as pip.
 * A value the user set is kept; `env` itself is not changed.
 */
export function withInstallMethod(env) {
  if (String(env.NEURON_INSTALL_METHOD ?? '').trim()) return {...env};
  return {...env, NEURON_INSTALL_METHOD: 'npm'};
}
