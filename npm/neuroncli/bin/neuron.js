#!/usr/bin/env node
import {existsSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {createRequire} from 'node:module';

const directory = path.dirname(fileURLToPath(import.meta.url));
const binary = path.join(directory, 'native', process.platform === 'win32' ? 'neuron.exe' : 'neuron');

if (!existsSync(binary)) {
  console.error('NeuronCLI binary is missing. Reinstall @zero-x.live/neuron to download the matching release.');
  process.exit(1);
}

const require = createRequire(import.meta.url);
const env = {
  ...process.env,
  NEURON_MCP_NODE: process.execPath,
  NEURON_CODEBASE_MCP_ENTRY: require.resolve('codebase-memory-mcp/bin.js'),
  NEURON_PLAYWRIGHT_MCP_ENTRY: path.join(path.dirname(require.resolve('@playwright/mcp/package.json')), 'cli.js'),
};
const result = spawnSync(binary, process.argv.slice(2), {stdio: 'inherit', env});
if (result.error) {
  console.error(`Could not start NeuronCLI: ${result.error.message}`);
  process.exitCode = 1;
} else {
  process.exitCode = result.status ?? 1;
}
