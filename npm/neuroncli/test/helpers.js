// Test helpers: a fake GitHub release server, dummy native binaries and an
// installed-style copy of this package.

import {execFileSync, spawn} from 'node:child_process';
import {createHash} from 'node:crypto';
import {chmodSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync} from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

export const PACKAGE_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const REPO_ROOT = path.resolve(PACKAGE_ROOT, '..', '..');
export const VERSION = JSON.parse(readFileSync(path.join(PACKAGE_ROOT, 'package.json'), 'utf8')).version;

const POSIX_DUMMY = `#!/bin/sh
case "\${NEURON_DUMMY_MODE:-}" in
    trap)
        trap 'echo got-TERM; exit 42' TERM
        trap 'echo got-INT; exit 43' INT
        echo ready
        while :; do sleep 1; done
        ;;
    self-terminate)
        kill -TERM $$
        ;;
esac
if [ "\${1:-}" = "--version" ]; then
    echo "neuron-dummy @VERSION@"
else
    echo "neuron-dummy args: $*"
fi
exit "\${NEURON_DUMMY_EXIT:-0}"
`;

const WINDOWS_DUMMY = `using System;
class NeuronDummy {
    static int Main(string[] args) {
        if (args.Length > 0 && args[0] == "--version") {
            Console.WriteLine("neuron-dummy @VERSION@");
        } else {
            Console.WriteLine("neuron-dummy args: " + string.Join(" ", args));
        }
        int code;
        return int.TryParse(Environment.GetEnvironmentVariable("NEURON_DUMMY_EXIT"), out code) ? code : 0;
    }
}
`;

function cscPath() {
  const windir = process.env.WINDIR || 'C:\\Windows';
  for (const framework of ['Framework64', 'Framework']) {
    const candidate = path.join(windir, 'Microsoft.NET', framework, 'v4.0.30319', 'csc.exe');
    if (existsSync(candidate)) return candidate;
  }
  return null;
}

/** Whether a runnable dummy binary can be built on this machine. */
export const canBuildDummy = process.platform !== 'win32' || cscPath() !== null;

const compiled = new Map();

/** Bytes of a dummy native binary that prints `neuron-dummy <version>` for --version. */
export function dummyBinary(version = VERSION) {
  if (process.platform !== 'win32') return Buffer.from(POSIX_DUMMY.replace('@VERSION@', version));
  if (!compiled.has(version)) {
    const directory = mkdtempSync(path.join(os.tmpdir(), 'neuron-dummy-'));
    try {
      const source = path.join(directory, 'dummy.cs');
      const output = path.join(directory, 'dummy.exe');
      writeFileSync(source, WINDOWS_DUMMY.replace('@VERSION@', version));
      execFileSync(cscPath(), ['/nologo', '/target:exe', `/out:${output}`, source], {stdio: 'ignore'});
      compiled.set(version, readFileSync(output));
    } finally {
      rmSync(directory, {recursive: true, force: true});
    }
  }
  return compiled.get(version);
}

export function writeExecutable(file, data) {
  mkdirSync(path.dirname(file), {recursive: true});
  writeFileSync(file, data);
  if (process.platform !== 'win32') chmodSync(file, 0o755);
  return file;
}

export function sha256(data) {
  return createHash('sha256').update(data).digest('hex');
}

/** A temporary directory removed after the test. */
export function tempDir(t) {
  const directory = mkdtempSync(path.join(os.tmpdir(), 'neuron-npm-'));
  t.after(() => rmSync(directory, {recursive: true, force: true}));
  return directory;
}

/** Copies the launcher files into `target`, like an installed package without a binary. */
export function copyPackage(target) {
  for (const entry of ['bin', 'lib', 'scripts', 'package.json']) {
    cpSync(path.join(PACKAGE_ROOT, entry), path.join(target, entry), {
      recursive: true,
      filter: (source) => path.basename(source) !== 'native',
    });
  }
  return target;
}

export function releaseRoutes({version = VERSION, asset, data, checksum}) {
  return new Map([
    [`/v${version}/${asset}`, {body: data}],
    [`/v${version}/${asset}.sha256`, {body: checksum ?? `${sha256(data)}  ${asset}\n`}],
  ]);
}

/** Starts an HTTP server on 127.0.0.1 serving `routes` (path -> {status, body}). */
export async function startServer(t, routes = new Map()) {
  const requests = [];
  const server = http.createServer((request, response) => {
    requests.push(request.url);
    const route = routes.get(request.url);
    if (!route) {
      response.writeHead(404, {'content-type': 'text/plain'});
      response.end('Not Found');
      return;
    }
    response.writeHead(route.status ?? 200, {'content-type': 'application/octet-stream'});
    response.end(route.body);
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const close = () => new Promise((resolve) => server.close(resolve));
  t.after(close);
  return {url: `http://127.0.0.1:${server.address().port}`, requests, routes, close};
}

/** A loopback URL on a port nothing listens on. */
export async function unusedUrl() {
  const server = http.createServer();
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const {port} = server.address();
  await new Promise((resolve) => server.close(resolve));
  return `http://127.0.0.1:${port}`;
}

export function cleanEnv(extra = {}) {
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('NEURON_')));
  return {...env, ...extra};
}

/** Runs a Node script asynchronously (so in-process servers keep serving). */
export function runNode(script, args, {env, onStdout} = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [script, ...args], {env, stdio: ['ignore', 'pipe', 'pipe']});
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
      onStdout?.(stdout, child);
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    child.on('error', reject);
    child.on('close', (code, signal) => resolve({code, signal, stdout, stderr}));
  });
}

export function nativeBinaryPath(packageRoot) {
  return path.join(packageRoot, 'bin', 'native', process.platform === 'win32' ? 'neuron.exe' : 'neuron');
}

export function currentAsset() {
  const assets = {
    'win32-x64': 'neuron-windows-x64.exe',
    'win32-arm64': 'neuron-windows-x64.exe',
    'linux-x64': 'neuron-linux-x64',
    'linux-arm64': 'neuron-linux-arm64',
    'darwin-arm64': 'neuron-macos-arm64',
    'darwin-x64': 'neuron-macos-x64',
  };
  return assets[`${process.platform}-${process.arch}`];
}
