import assert from 'node:assert/strict';
import {execSync} from 'node:child_process';
import {readdirSync, readFileSync} from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import {test} from 'node:test';
import {setTimeout as delay} from 'node:timers/promises';
import {LauncherError, assetFor, binaryLocations, cacheRoot, downloadBase, downloadBinary, parseChecksum} from '../lib/binary.js';
import {exitCodeForSignal, listenedSignals, shouldForwardSignal} from '../lib/signals.js';
import {PACKAGE_ROOT, REPO_ROOT, sha256, tempDir} from './helpers.js';

const ASSET = 'neuron-linux-x64';

/** An HTTP server on 127.0.0.1 whose handler may stream slowly or stall; returns its URL. */
async function slowServer(t, handler) {
  const server = http.createServer(handler);
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(() => {
    server.closeAllConnections(); // stalled responses would keep close() waiting
    return new Promise((resolve) => server.close(resolve));
  });
  return `http://127.0.0.1:${server.address().port}`;
}

function download(t, base, idleTimeoutMs) {
  const directory = tempDir(t);
  const destination = path.join(directory, 'neuron');
  const done = downloadBinary({destination, url: `${base}/v1.0.0/${ASSET}`, version: '1.0.0', asset: ASSET, idleTimeoutMs});
  return {directory, destination, done};
}

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

test('a slow download succeeds while data keeps arriving', async (t) => {
  const chunks = Array.from({length: 8}, (_, index) => Buffer.alloc(1024, index));
  const data = Buffer.concat(chunks);
  const base = await slowServer(t, async (request, response) => {
    if (request.url.endsWith('.sha256')) {
      response.end(`${sha256(data)}  ${ASSET}
`);
      return;
    }
    response.writeHead(200, {'content-length': data.length});
    for (const chunk of chunks) {
      response.write(chunk);
      await delay(150);
    }
    response.end();
  });
  const started = Date.now();
  const {destination, done} = download(t, base, 500);
  assert.equal(await done, destination);
  assert.ok(Date.now() - started >= 1000, 'the whole transfer took longer than the idle timeout');
  assert.deepEqual(readFileSync(destination), data);
});

test('a download that stops sending data fails as a network error and leaves nothing behind', async (t) => {
  const base = await slowServer(t, (request, response) => {
    if (request.url.endsWith('.sha256')) {
      response.end(`${'a'.repeat(64)}  ${ASSET}
`);
      return;
    }
    response.writeHead(200, {'content-length': 4096});
    response.write(Buffer.alloc(1024)); // and then nothing
  });
  const {directory, done} = download(t, base, 300);
  await assert.rejects(done, (error) => {
    assert.ok(error instanceof LauncherError, error);
    assert.equal(error.kind, 'network');
    assert.match(error.message, /was interrupted: timed out \(no data received for 0\.3 s\)/);
    return true;
  });
  assert.deepEqual(readdirSync(directory), []);
});

test('a server that never answers times out with an actionable message', async (t) => {
  const base = await slowServer(t, () => {}); // accepts the connection, never responds
  const {done} = download(t, base, 300);
  await assert.rejects(done, (error) => {
    assert.ok(error instanceof LauncherError, error);
    assert.equal(error.kind, 'network');
    assert.match(error.message, /Could not download Neuron 1\.0\.0 .*timed out/);
    assert.match(error.message, /NEURON_BINARY_PATH/);
    return true;
  });
});
