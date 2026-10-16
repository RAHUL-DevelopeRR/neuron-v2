// Signal relay between the npm launcher and the native binary.

import os from 'node:os';

/** Signals the launcher listens for while the native binary runs. */
export function listenedSignals(platform = process.platform) {
  return platform === 'win32' ? ['SIGINT', 'SIGBREAK', 'SIGHUP'] : ['SIGINT', 'SIGTERM', 'SIGHUP'];
}

/**
 * Whether the launcher should pass `signal` on to the child.
 *
 * - Windows: never. Ctrl+C / Ctrl+Break reach every process on the console,
 *   the child included, and `child.kill()` would terminate it outright.
 * - POSIX SIGINT from a terminal: the terminal already delivered it to the
 *   whole foreground process group, child included. Forwarding would deliver
 *   it twice, and Neuron treats a second Ctrl+C during a turn as force-quit.
 * - Everything else (SIGTERM, SIGHUP, SIGINT sent to the launcher's PID by a
 *   supervisor) is forwarded.
 */
export function shouldForwardSignal(signal, {platform = process.platform, interactive = false} = {}) {
  if (platform === 'win32') return false;
  if (signal === 'SIGINT' && interactive) return false;
  return true;
}

export function isInteractive(stdio = process) {
  return Boolean(stdio.stdin?.isTTY || stdio.stdout?.isTTY || stdio.stderr?.isTTY);
}

/** Conventional shell exit status for a process terminated by `signal`. */
export function exitCodeForSignal(signal) {
  return 128 + (os.constants.signals[signal] ?? 1);
}
