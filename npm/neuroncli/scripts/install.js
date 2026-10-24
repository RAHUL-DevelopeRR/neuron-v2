// postinstall: download and verify the native binaries for this package
// version: `neuron`, then its full-screen UI, `neuron-tui`.
//
// Transient network failures do not fail `npm install`: the launcher retries
// the download on first run. For `neuron`, a checksum mismatch, a missing
// release asset or an unsupported platform does. Nothing about the UI fails
// the install: without it `neuron` runs its line REPL, and the launcher tries
// again when a terminal session wants it. Set NEURON_SKIP_DOWNLOAD=1 to skip
// both downloads here, or NEURON_NO_TUI=1 to skip the UI's.

import {LauncherError, ensureBinary} from '../lib/binary.js';
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
