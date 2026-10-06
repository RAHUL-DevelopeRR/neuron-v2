#!/usr/bin/env node
import {existsSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

const directory = path.dirname(fileURLToPath(import.meta.url));
const binary = path.join(directory, 'native', process.platform === 'win32' ? 'neuron.exe' : 'neuron');

if (!existsSync(binary)) {
  console.error('NeuronCLI binary is missing. Reinstall @zero-x.live/neuron to download the matching release.');
  process.exit(1);
}

const result = spawnSync(binary, process.argv.slice(2), {stdio: 'inherit'});
if (result.error) {
  console.error(`Could not start NeuronCLI: ${result.error.message}`);
  process.exitCode = 1;
} else {
  process.exitCode = result.status ?? 1;
}
