#!/usr/bin/env node
// npm entry point: runs the native Neuron binary for this package version,
// downloading and verifying it on first use when postinstall did not run
// (for example `npm install --ignore-scripts`).
//
// The launcher sets process.exitCode instead of calling process.exit():
// exiting while fetch handles are still closing aborts Node on Windows.

import {spawn} from 'node:child_process';
import {existsSync, statSync} from 'node:fs';
import {createRequire} from 'node:module';
import path from 'node:path';
import {LauncherError, ensureBinary} from '../lib/binary.js';
import {exitCodeForSignal, isInteractive, listenedSignals, shouldForwardSignal} from '../lib/signals.js';

function report(error) {
  const message = error instanceof LauncherError ? error.message : error?.stack || String(error);
  process.stderr.write(`neuron: ${message}\n`);
}

async function resolveBinary() {
  const override = process.env.NEURON_BINARY_PATH;
  if (override) {
    const candidate = path.resolve(override);
    if (!existsSync(candidate) || !statSync(candidate).isFile()) {
      throw new LauncherError('NEURON_BINARY_PATH must point to a native Neuron binary.');
    }
    return candidate;
  }
  return ensureBinary({log: (line) => process.stderr.write(`${line}\n`)});
}

function childEnvironment() {
  const require = createRequire(import.meta.url);
  const env = {...process.env, NEURON_MCP_NODE: process.execPath};
  // Default MCP integrations installed with this package; skip any that failed to install.
  try {
    env.NEURON_CODEBASE_MCP_ENTRY = require.resolve('codebase-memory-mcp/bin.js');
  } catch {
    // not installed
  }
  try {
    env.NEURON_PLAYWRIGHT_MCP_ENTRY = path.join(path.dirname(require.resolve('@playwright/mcp/package.json')), 'cli.js');
  } catch {
    // not installed
  }
  return env;
}

async function main() {
  let binary;
  try {
    binary = await resolveBinary();
  } catch (error) {
    report(error);
    process.exitCode = 1;
    return;
  }

  const child = spawn(binary, process.argv.slice(2), {stdio: 'inherit', env: childEnvironment()});
  const signals = listenedSignals();
  const relay = (signal) => {
    if (!shouldForwardSignal(signal, {interactive: isInteractive()})) return;
    try {
      child.kill(signal);
    } catch {
      // the child already exited
    }
  };
  const stopRelaying = () => {
    for (const signal of signals) process.removeListener(signal, relay);
  };
  for (const signal of signals) process.on(signal, relay);

  child.on('error', (error) => {
    stopRelaying();
    report(new LauncherError(`could not start ${binary}: ${error.message}`));
    process.exitCode = 1;
  });

  child.on('exit', (code, signal) => {
    stopRelaying();
    if (signal) {
      process.exitCode = exitCodeForSignal(signal);
      // Exit the way the child did so shells and supervisors see the signal.
      if (process.platform !== 'win32') process.kill(process.pid, signal);
      return;
    }
    process.exitCode = code ?? 1;
  });
}

await main();
