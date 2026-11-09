// postinstall: download and verify the native binaries for this package
// version: `neuron`, then its full-screen UI, `neuron-tui`.
//
// Transient network failures do not fail `npm install`: the launcher retries
// the download on first run. For `neuron`, a checksum mismatch, a missing
// release asset or an unsupported platform does. Nothing about the UI fails
// the install: without it `neuron` runs its line REPL, and the launcher tries
// again when a terminal session wants it. Set NEURON_SKIP_DOWNLOAD=1 to skip
// both downloads here, or NEURON_NO_TUI=1 to skip the UI's.

import {DEFAULT_DOWNLOAD_BASE, LauncherError, ensureBinary, packageVersion} from '../lib/binary.js';
import {isTruthy} from '../lib/full-screen.js';

const log = (line) => process.stderr.write(`neuron: ${line}\n`);

if (process.env.NEURON_SKIP_DOWNLOAD === '1') {
  log('NEURON_SKIP_DOWNLOAD=1, skipping the binary download; it runs on first use.');
} else {
  let cli = false;
  try {
    await ensureBinary({log});
    cli = true;
  } catch (error) {
    if (error instanceof LauncherError && error.kind === 'network') {
      log(`${error.message}\nThe binary will be downloaded the first time you run neuron.`);
    } else {
      log(error instanceof LauncherError ? error.message : String(error?.stack || error));
      process.exitCode = 1;
    }
  }
  if (cli && !isTruthy(process.env.NEURON_NO_TUI)) {
    try {
      await ensureBinary({log, component: 'ui'});
    } catch (error) {
      const message = error instanceof LauncherError ? error.message : String(error?.message || error);
      log(`The full-screen UI was not installed (${message}). neuron uses its line REPL until it is.`);
    }
  }
}

// npm's `neuron.ps1` shim, which PowerShell prefers, cannot run under the default "Restricted"
// execution policy. npm only shows this with --foreground-scripts or in its log; the README says it
// too. The shim itself is npm's, so it is left alone.
if (process.platform === 'win32') {
  log(
    'On Windows PowerShell, if `neuron` (or `npm`) says "running scripts is disabled on this system", ' +
      'run `neuron.cmd` (and `npm.cmd`) instead, or use Command Prompt. ' +
      'Or install with the PowerShell installer, which needs no scripts enabled: ' +
      `irm ${DEFAULT_DOWNLOAD_BASE}/v${packageVersion()}/install.ps1 | iex`,
  );
}
