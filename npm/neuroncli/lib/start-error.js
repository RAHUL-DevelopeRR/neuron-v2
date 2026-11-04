// Why the native binary did not start, in words a user can act on. On Windows an
// antivirus that blocks or quarantines neuron.exe makes spawn() fail with UNKNOWN
// (errno -4094) or EPERM, or leaves the binary missing on the next run.

import {existsSync} from 'node:fs';

const BLOCKED_CODES = new Set(['UNKNOWN', 'EPERM', 'EACCES']);

export function startFailureMessage(binary, error, {platform = process.platform, exists = existsSync} = {}) {
  const code = error?.code || 'error';
  const reinstall = 'npm install --global @zero-x.live/neuron';
  if (!exists(binary)) {
    const culprit = platform === 'win32' ? 'Antivirus software (such as Microsoft Defender) may have removed it' : 'Something removed it';
    return `could not start ${binary}: the file is missing. ${culprit}. Reinstall with \`${reinstall}\`.`;
  }
  if (platform === 'win32' && BLOCKED_CODES.has(code)) {
    return (
      `Windows would not start ${binary} (${code}). Antivirus software may have blocked it: check ` +
      'Windows Security > Virus & threat protection > Protection history. If it was quarantined, ' +
      `restore or allow it there, or reinstall with \`${reinstall}\`.`
    );
  }
  return `could not start ${binary}: ${error?.message || code}`;
}
