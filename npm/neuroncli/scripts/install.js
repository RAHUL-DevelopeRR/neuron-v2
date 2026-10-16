// postinstall: download and verify the native binary for this package version.
//
// Transient network failures do not fail `npm install`: the launcher retries
// the download on first run. A checksum mismatch, a missing release asset or
// an unsupported platform does. Set NEURON_SKIP_DOWNLOAD=1 to skip the
// download here (the first run fetches it instead).

import {LauncherError, ensureBinary} from '../lib/binary.js';

const log = (line) => process.stderr.write(`neuron: ${line}\n`);

if (process.env.NEURON_SKIP_DOWNLOAD === '1') {
  log('NEURON_SKIP_DOWNLOAD=1, skipping the binary download; it runs on first use.');
} else {
  try {
    await ensureBinary({log});
  } catch (error) {
    if (error instanceof LauncherError && error.kind === 'network') {
      log(`${error.message}\nThe binary will be downloaded the first time you run neuron.`);
    } else {
      log(error instanceof LauncherError ? error.message : String(error?.stack || error));
      process.exitCode = 1;
    }
  }
}
