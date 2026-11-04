import assert from 'node:assert/strict';
import test from 'node:test';

import {startFailureMessage} from '../lib/start-error.js';

const error = (code, message = `spawn ${code}`) => Object.assign(new Error(message), {code});
const present = () => true;
const missing = () => false;

test('a binary Windows refuses to start points at the antivirus', () => {
  for (const code of ['UNKNOWN', 'EPERM', 'EACCES']) {
    const message = startFailureMessage('C:\\npm\\neuron.exe', error(code), {platform: 'win32', exists: present});
    assert.match(message, new RegExp(`Windows would not start C:\\\\npm\\\\neuron\\.exe \\(${code}\\)`));
    assert.match(message, /Protection history/);
    assert.match(message, /npm install --global @zero-x\.live\/neuron/);
  }
});

test('a missing binary says so and how to get it back', () => {
  const windows = startFailureMessage('C:\\npm\\neuron.exe', error('ENOENT'), {platform: 'win32', exists: missing});
  assert.match(windows, /the file is missing\. Antivirus software/);
  const linux = startFailureMessage('/usr/lib/neuron', error('ENOENT'), {platform: 'linux', exists: missing});
  assert.match(linux, /the file is missing\. Something removed it\. Reinstall/);
});

test('other failures keep the system message', () => {
  const message = startFailureMessage('/usr/lib/neuron', error('ENOEXEC', 'spawn ENOEXEC'), {platform: 'linux', exists: present});
  assert.equal(message, 'could not start /usr/lib/neuron: spawn ENOEXEC');
});
