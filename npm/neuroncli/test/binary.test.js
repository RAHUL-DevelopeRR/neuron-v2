import assert from 'node:assert/strict';
import {execSync} from 'node:child_process';
import {readFileSync} from 'node:fs';
import path from 'node:path';
import {test} from 'node:test';
import {LauncherError, assetFor, binaryLocations, cacheRoot, downloadBase, parseChecksum} from '../lib/binary.js';
import {exitCodeForSignal, listenedSignals, shouldForwardSignal} from '../lib/signals.js';
import {PACKAGE_ROOT, REPO_ROOT} from './helpers.js';

test('every published platform maps to its release asset', () => {
  const cases = [
    ['win32', 'x64', 'neuron-windows-x64.exe'],
    ['win32', 'arm64', 'neuron-windows-x64.exe'],
    ['linux', 'x64', 'neuron-linux-x64'],
    ['linux', 'arm64', 'neuron-linux-arm64'],
    ['darwin', 'arm64', 'neuron-macos-arm64'],
    ['darwin', 'x64', 'neuron-macos-x64'],
  ];
  for (const [platform, arch, asset] of cases) {
    assert.equal(assetFor(platform, arch, {musl: false}), asset, `${platform}-${arch}`);
  }
  assert.throws(() => assetFor('freebsd', 'x64'), (error) => error instanceof LauncherError && error.kind === 'platform');
  assert.throws(() => assetFor('linux', 'x64', {musl: true}), /musl/);
});

test('download base must be HTTPS or loopback HTTP', () => {
  assert.equal(downloadBase({}), 'https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download');
  for (const base of ['https://mirror.example/dl/', 'http://127.0.0.1:9', 'http://localhost:1/x', 'http://[::1]:8000']) {
    assert.equal(downloadBase({NEURON_DOWNLOAD_BASE: base}), base.replace(/\/+$/, ''));
  }
  for (const base of ['http://mirror.example/dl', 'file:///tmp/x', 'not a url']) {
    assert.throws(() => downloadBase({NEURON_DOWNLOAD_BASE: base}), LauncherError, base);
  }
});

test('checksum files use the "<sha256>  <name>" release format', () => {
  const digest = 'a'.repeat(64);
  assert.equal(parseChecksum(`${digest}  neuron-linux-x64\n`), digest);
  assert.equal(parseChecksum(`${digest.toUpperCase()}\n`), digest);
  assert.equal(parseChecksum('not a checksum'), null);
  assert.equal(parseChecksum(`${digest}0  neuron`), null);
});

test('the cache layout matches the Python launcher', () => {
  assert.equal(cacheRoot({NEURON_CACHE_DIR: path.resolve('custom')}), path.resolve('custom'));
  assert.equal(cacheRoot({LOCALAPPDATA: 'C:\\Users\\me\\AppData\\Local'}, 'win32'), path.join('C:\\Users\\me\\AppData\\Local', 'neuroncli'));
  assert.equal(cacheRoot({XDG_CACHE_HOME: '/var/cache/me'}, 'linux'), path.join('/var/cache/me', 'neuroncli'));
  const [packaged, cached] = binaryLocations({
    root: '/pkg',
    version: '6.3.0',
    asset: 'neuron-linux-x64',
    env: {NEURON_CACHE_DIR: '/cache'},
    platform: 'linux',
  });
  assert.equal(packaged, path.join('/pkg', 'bin', 'native', 'neuron'));
  assert.equal(cached, path.join(path.resolve('/cache'), '6.3.0', 'neuron-linux-x64'));
});

test('signals are relayed only when the child would not receive them itself', () => {
  assert.deepEqual(listenedSignals('linux'), ['SIGINT', 'SIGTERM', 'SIGHUP']);
  assert.deepEqual(listenedSignals('win32'), ['SIGINT', 'SIGBREAK', 'SIGHUP']);
  for (const signal of listenedSignals('win32')) {
    assert.equal(shouldForwardSignal(signal, {platform: 'win32', interactive: false}), false, signal);
  }
  assert.equal(shouldForwardSignal('SIGINT', {platform: 'linux', interactive: true}), false);
  assert.equal(shouldForwardSignal('SIGINT', {platform: 'linux', interactive: false}), true);
  assert.equal(shouldForwardSignal('SIGTERM', {platform: 'darwin', interactive: true}), true);
  assert.equal(shouldForwardSignal('SIGHUP', {platform: 'linux', interactive: true}), true);
  assert.equal(exitCodeForSignal('SIGINT'), 130);
  assert.equal(exitCodeForSignal('SIGTERM'), 143);
});

test('the package ships LICENSE and NOTICE identical to the repository copies', () => {
  for (const name of ['LICENSE', 'NOTICE']) {
    const normalize = (text) => text.replace(/\r\n/g, '\n');
    assert.equal(
      normalize(readFileSync(path.join(PACKAGE_ROOT, name), 'utf8')),
      normalize(readFileSync(path.join(REPO_ROOT, name), 'utf8')),
      `${name} drifted from the repository root`,
    );
  }
});

test('npm pack includes the launcher, license files and nothing from test/', () => {
  const output = execSync('npm pack --dry-run --json --ignore-scripts', {cwd: PACKAGE_ROOT, encoding: 'utf8'});
  const files = JSON.parse(output)[0].files.map((file) => file.path.replace(/\\/g, '/')).sort();
  for (const expected of ['LICENSE', 'NOTICE', 'README.md', 'bin/neuron.js', 'lib/binary.js', 'lib/signals.js', 'package.json', 'scripts/install.js']) {
    assert.ok(files.includes(expected), `${expected} missing from ${files.join(', ')}`);
  }
  assert.ok(!files.some((file) => file.startsWith('test/') || file.startsWith('bin/native')), files.join(', '));
});
