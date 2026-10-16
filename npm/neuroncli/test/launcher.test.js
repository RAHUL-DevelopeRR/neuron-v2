import assert from 'node:assert/strict';
import {chmodSync, existsSync, readdirSync, readFileSync} from 'node:fs';
import path from 'node:path';
import {test} from 'node:test';
import {
  VERSION,
  canBuildDummy,
  cleanEnv,
  copyPackage,
  currentAsset,
  dummyBinary,
  nativeBinaryPath,
  releaseRoutes,
  runNode,
  sha256,
  startServer,
  tempDir,
  unusedUrl,
  writeExecutable,
} from './helpers.js';

const skip = canBuildDummy ? false : 'needs csc.exe to build a Windows dummy binary';
const posixOnly = process.platform === 'win32' ? 'POSIX signals only' : false;
const asset = currentAsset();

function setup(t) {
  const root = tempDir(t);
  const packageRoot = copyPackage(path.join(root, 'package'));
  return {
    root,
    packageRoot,
    cache: path.join(root, 'cache'),
    launcher: path.join(packageRoot, 'bin', 'neuron.js'),
    postinstall: path.join(packageRoot, 'scripts', 'install.js'),
  };
}

function environment(context, base, extra = {}) {
  return cleanEnv({NEURON_DOWNLOAD_BASE: base, NEURON_CACHE_DIR: context.cache, ...extra});
}

function assertNoStack(stderr) {
  assert.doesNotMatch(stderr, /^\s+at /m, stderr);
}

test('first run without postinstall downloads, verifies and runs the binary', {skip}, async (t) => {
  const context = setup(t);
  const data = dummyBinary();
  const server = await startServer(t, releaseRoutes({asset, data}));
  const result = await runNode(context.launcher, ['--version'], {env: environment(context, server.url)});
  assert.equal(result.code, 0, result.stderr);
  assert.match(result.stdout, new RegExp(`neuron-dummy ${VERSION}`));
  assert.match(result.stderr, /Downloading Neuron/);
  assert.deepEqual(server.requests, [`/v${VERSION}/${asset}.sha256`, `/v${VERSION}/${asset}`]);
  const binary = nativeBinaryPath(context.packageRoot);
  assert.equal(readFileSync(`${binary}.sha256`, 'ascii'), `${sha256(data)}  ${asset}\n`);
  assert.deepEqual(readdirSync(path.dirname(binary)).sort(), [path.basename(binary), `${path.basename(binary)}.sha256`]);

  const offline = await runNode(context.launcher, ['--version'], {env: environment(context, await unusedUrl())});
  assert.equal(offline.code, 0, offline.stderr);
  assert.doesNotMatch(offline.stderr, /Downloading/);
});

test('arguments and the exit code are forwarded', {skip}, async (t) => {
  const context = setup(t);
  const server = await startServer(t, releaseRoutes({asset, data: dummyBinary()}));
  const result = await runNode(context.launcher, ['prompt', 'two words'], {
    env: environment(context, server.url, {NEURON_DUMMY_EXIT: '7'}),
  });
  assert.equal(result.code, 7, result.stderr);
  assert.match(result.stdout, /neuron-dummy args: prompt two words/);
});

test('a checksum mismatch installs nothing', {skip}, async (t) => {
  const context = setup(t);
  const server = await startServer(t, releaseRoutes({asset, data: dummyBinary(), checksum: `${'0'.repeat(64)}  ${asset}\n`}));
  const result = await runNode(context.launcher, ['--version'], {env: environment(context, server.url)});
  assert.equal(result.code, 1);
  assert.match(result.stderr, /failed its SHA-256 check/);
  assertNoStack(result.stderr);
  const native = path.dirname(nativeBinaryPath(context.packageRoot));
  assert.deepEqual(existsSync(native) ? readdirSync(native) : [], []);
});

test('an offline first run explains how to recover', async (t) => {
  const context = setup(t);
  const result = await runNode(context.launcher, ['--version'], {env: environment(context, await unusedUrl())});
  assert.equal(result.code, 1);
  assert.match(result.stderr, /Could not download Neuron/);
  assert.match(result.stderr, /NEURON_BINARY_PATH/);
  assertNoStack(result.stderr);
});

test('an unpublished release reports HTTP 404', async (t) => {
  const context = setup(t);
  const server = await startServer(t);
  const result = await runNode(context.launcher, ['--version'], {env: environment(context, server.url)});
  assert.equal(result.code, 1);
  assert.match(result.stderr, /HTTP 404/);
});

test('NEURON_BINARY_PATH must name an existing binary', async (t) => {
  const context = setup(t);
  const result = await runNode(context.launcher, [], {
    env: environment(context, await unusedUrl(), {NEURON_BINARY_PATH: path.join(context.root, 'missing')}),
  });
  assert.equal(result.code, 1);
  assert.match(result.stderr, /NEURON_BINARY_PATH must point to a native Neuron binary/);
});

test('postinstall verifies the download and tolerates network failures', {skip}, async (t) => {
  const context = setup(t);
  const offline = await runNode(context.postinstall, [], {env: environment(context, await unusedUrl())});
  assert.equal(offline.code, 0, offline.stderr);
  assert.match(offline.stderr, /downloaded the first time you run neuron/);

  const skipped = await runNode(context.postinstall, [], {env: environment(context, await unusedUrl(), {NEURON_SKIP_DOWNLOAD: '1'})});
  assert.equal(skipped.code, 0);
  assert.match(skipped.stderr, /skipping the binary download/);

  const bad = await startServer(t, releaseRoutes({asset, data: dummyBinary(), checksum: `${'f'.repeat(64)}  ${asset}\n`}));
  const mismatch = await runNode(context.postinstall, [], {env: environment(context, bad.url)});
  assert.equal(mismatch.code, 1);
  assert.match(mismatch.stderr, /failed its SHA-256 check/);

  const good = await startServer(t, releaseRoutes({asset, data: dummyBinary()}));
  const installed = await runNode(context.postinstall, [], {env: environment(context, good.url)});
  assert.equal(installed.code, 0, installed.stderr);
  assert.ok(existsSync(`${nativeBinaryPath(context.packageRoot)}.sha256`));
});

const canMakeReadOnly = process.platform !== 'win32' && process.getuid?.() !== 0;

test('a read-only package directory falls back to the user cache', {skip: canMakeReadOnly ? false : 'needs a non-root POSIX user'}, async (t) => {
  const context = setup(t);
  const bin = path.join(context.packageRoot, 'bin');
  const server = await startServer(t, releaseRoutes({asset, data: dummyBinary()}));
  chmodSync(bin, 0o555);
  let result;
  try {
    result = await runNode(context.launcher, ['--version'], {env: environment(context, server.url)});
  } finally {
    chmodSync(bin, 0o755); // the temporary directory cleanup needs write access
  }
  assert.equal(result.code, 0, result.stderr);
  assert.ok(!existsSync(path.join(bin, 'native')));
  assert.ok(existsSync(path.join(context.cache, VERSION, `${asset}.sha256`)));
});

async function runUntilReady(context, mode, signal) {
  const dummy = writeExecutable(path.join(context.root, 'dummy-neuron'), dummyBinary());
  let sent = false;
  return runNode(context.launcher, [], {
    env: environment(context, await unusedUrl(), {NEURON_BINARY_PATH: dummy, NEURON_DUMMY_MODE: mode}),
    onStdout: (stdout, child) => {
      if (!sent && signal && stdout.includes('ready')) {
        sent = true;
        child.kill(signal);
      }
    },
  });
}

test('SIGTERM is forwarded and the child exit code is returned', {skip: posixOnly}, async (t) => {
  const result = await runUntilReady(setup(t), 'trap', 'SIGTERM');
  assert.match(result.stdout, /got-TERM/);
  assert.equal(result.code, 42);
});

test('SIGINT from a supervisor (no terminal) is forwarded', {skip: posixOnly}, async (t) => {
  const result = await runUntilReady(setup(t), 'trap', 'SIGINT');
  assert.match(result.stdout, /got-INT/);
  assert.equal(result.code, 43);
});

test('a child killed by a signal makes the launcher exit the same way', {skip: posixOnly}, async (t) => {
  const result = await runUntilReady(setup(t), 'self-terminate', null);
  assert.equal(result.signal, 'SIGTERM');
});
