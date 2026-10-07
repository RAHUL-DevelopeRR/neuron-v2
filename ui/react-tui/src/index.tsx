#!/usr/bin/env node
import {spawnSync} from 'node:child_process';
import {existsSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import path from 'node:path';

// Use the native terminal runtime so prompts, permissions and tools share one state.
const root = fileURLToPath(new URL('../../../', import.meta.url));
const name = process.platform === 'win32' ? 'neuron.exe' : 'neuron';
const candidates = [
  process.env.NEURON_BINARY,
  path.join(root, 'rust', 'target', 'release', name),
  path.join(root, 'rust', 'target', 'debug', name),
].filter((value): value is string => Boolean(value));
const binary = candidates.find(value => existsSync(value)) ?? name;
const result = spawnSync(binary, process.argv.slice(2), {stdio: 'inherit'});
if (result.error) {
  console.error(`Could not start Neuron: ${result.error.message}. Build from rust/ or set NEURON_BINARY.`);
  process.exitCode = 1;
} else {
  process.exitCode = result.status ?? 1;
}
