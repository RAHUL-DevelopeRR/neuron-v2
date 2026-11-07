// Locates, downloads and verifies the native binaries for this package: the
// `neuron` CLI and engine, and `neuron-tui`, its full-screen UI (the "ui"
// component), which `neuron` starts in place of its line REPL on a terminal.
//
// Layout (shared with the Python launcher's cache):
//   <package>/bin/native/neuron[.exe] (+ .sha256)        written by postinstall
//   <package>/bin/native/neuron-tui[.exe] (+ .sha256)
//   <cache>/neuroncli/<version>/<asset> (+ .sha256)      fallback when the package
//                                                         directory is read-only
// A binary counts as installed only when its .sha256 sidecar exists; the
// sidecar is written after the download passed its SHA-256 check against both
// the per-file checksum and the release's SHA256SUMS. A `.verified` stamp
// ("<size> <mtime ns>") records which file that was; when the binary's size or
// mtime no longer matches the stamp it is hashed again before it may run.

import {createHash} from 'node:crypto';
import {chmodSync, createWriteStream, existsSync, mkdirSync, readFileSync, renameSync, rmSync, statSync, writeFileSync} from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {Readable, Transform} from 'node:stream';
import {pipeline} from 'node:stream/promises';
import {fileURLToPath} from 'node:url';

export const REPOSITORY = 'https://github.com/RAHUL-DevelopeRR/neuron-v2';
export const DEFAULT_DOWNLOAD_BASE = `${REPOSITORY}/releases/download`;
export const PACKAGE_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

// A download fails only when no data arrives for this long (connecting, waiting for
// the response, or between body chunks), however slow the link is overall.
export const IDLE_TIMEOUT_MS = 60_000;
const SUPPORTED = 'Linux x64/arm64, macOS arm64/x64 and Windows x64';

const ASSETS = new Map([
  ['win32-x64', 'neuron-windows-x64.exe'],
  // Windows 11 on ARM runs the x64 build under emulation.
  ['win32-arm64', 'neuron-windows-x64.exe'],
  ['linux-x64', 'neuron-linux-x64'],
  ['linux-arm64', 'neuron-linux-arm64'],
  ['darwin-arm64', 'neuron-macos-arm64'],
  ['darwin-x64', 'neuron-macos-x64'],
]);

// The UI's asset for each platform is the CLI's with `neuron-tui-` for `neuron-`.
export const COMPONENTS = {
  neuron: {file: 'neuron', label: 'Neuron', assetPrefix: 'neuron-'},
  ui: {file: 'neuron-tui', label: "Neuron's full-screen UI", assetPrefix: 'neuron-tui-'},
};

function component(name) {
  const found = COMPONENTS[name];
  if (!found) throw new LauncherError(`Unknown component: ${name}`);
  return found;
}

/**
 * An error whose message is meant for the user (printed without a stack).
 * `kind` is 'network' for transient download failures that a later run may
 * not hit, otherwise 'missing', 'integrity', 'platform', 'config' or 'filesystem'.
 */
export class LauncherError extends Error {
  constructor(message, {cause, kind = 'config'} = {}) {
    super(message, cause === undefined ? undefined : {cause});
    this.name = 'LauncherError';
    this.kind = kind;
  }
}

export function packageVersion(root = PACKAGE_ROOT) {
  return JSON.parse(readFileSync(path.join(root, 'package.json'), 'utf8')).version;
}

function isMuslLinux() {
  try {
    return !process.report.getReport().header.glibcVersionRuntime;
  } catch {
    return false;
  }
}

export function assetFor(
  platform = process.platform,
  arch = process.arch,
  {musl = platform === 'linux' && isMuslLinux(), component: name = 'neuron'} = {},
) {
  const cliAsset = ASSETS.get(`${platform}-${arch}`);
  const asset = cliAsset && component(name).assetPrefix + cliAsset.slice('neuron-'.length);
  if (!asset) {
    throw new LauncherError(
      `Neuron does not publish a binary for ${platform} ${arch}. Supported: ${SUPPORTED}. ` +
        `Build from source (${REPOSITORY}) and set NEURON_BINARY_PATH to the built binary.`,
      {kind: 'platform'},
    );
  }
  if (musl) {
    throw new LauncherError('musl-based Linux (for example Alpine) is not supported; the Linux binaries need glibc.', {
      kind: 'platform',
    });
  }
  return asset;
}

function checkedBase(name, value) {
  const base = value.replace(/\/+$/, '');
  let url;
  try {
    url = new URL(base);
  } catch {
    throw new LauncherError(`${name} is not a valid URL: ${base}`);
  }
  const loopback = ['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname);
  if (url.protocol === 'https:' || (url.protocol === 'http:' && loopback)) return base;
  throw new LauncherError(`${name} must be an HTTPS URL (or loopback HTTP): ${base}`);
}

export function downloadBase(env = process.env) {
  return checkedBase('NEURON_DOWNLOAD_BASE', env.NEURON_DOWNLOAD_BASE || DEFAULT_DOWNLOAD_BASE);
}

/**
 * Where SHA256SUMS comes from: the GitHub release by default, even when
 * NEURON_DOWNLOAD_BASE points at a mirror, so a mirror cannot vouch for itself.
 */
export function checksumsBase(env = process.env) {
  return checkedBase('NEURON_CHECKSUMS_BASE', env.NEURON_CHECKSUMS_BASE || DEFAULT_DOWNLOAD_BASE);
}

export function cacheRoot(env = process.env, platform = process.platform) {
  if (env.NEURON_CACHE_DIR) return path.resolve(env.NEURON_CACHE_DIR);
  if (platform === 'win32') {
    return path.join(env.LOCALAPPDATA || path.join(os.homedir(), 'AppData', 'Local'), 'neuroncli');
  }
  if (platform === 'darwin') return path.join(os.homedir(), 'Library', 'Caches', 'neuroncli');
  return path.join(env.XDG_CACHE_HOME || path.join(os.homedir(), '.cache'), 'neuroncli');
}

/** Candidate install locations, in preference order. */
export function binaryLocations({
  root = PACKAGE_ROOT,
  version,
  asset,
  env = process.env,
  platform = process.platform,
  component: componentName = 'neuron',
}) {
  const {file} = component(componentName);
  const name = platform === 'win32' ? `${file}.exe` : file;
  return [path.join(root, 'bin', 'native', name), path.join(cacheRoot(env, platform), version, asset)];
}

export function parseChecksum(text) {
  const match = /^\s*([0-9a-fA-F]{64})(?:\s|$)/.exec(text);
  return match ? match[1].toLowerCase() : null;
}

/** The hash of `asset` in a SHA256SUMS listing ("<hash>  <name>" or "<hash> *<name>"), or null. */
export function parseSums(text, asset) {
  for (const line of text.split(/\r?\n/)) {
    const match = /^\s*([0-9a-fA-F]{64})\s+\*?(.+?)\s*$/.exec(line);
    if (match && match[2] === asset) return match[1].toLowerCase();
  }
  return null;
}

function recordedChecksum(binary) {
  try {
    return parseChecksum(readFileSync(`${binary}.sha256`, 'ascii'));
  } catch {
    return null;
  }
}

function stampOf(binary) {
  const {size, mtimeNs} = statSync(binary, {bigint: true});
  return `${size} ${mtimeNs}\n`;
}

function writeStamp(binary, stamp = stampOf(binary)) {
  try {
    writeFileSync(`${binary}.verified`, stamp);
  } catch {
    // A read-only location only costs a re-hash on the next launch.
  }
}

/**
 * 'missing' (no binary or no sidecar), 'ok', or 'changed' (its size or mtime differs from
 * when it was verified and its hash no longer matches the sidecar). A changed file whose hash
 * still matches is stamped again, so the next launch skips the hashing.
 */
export function installedState(binary) {
  if (!existsSync(binary)) return 'missing';
  const expected = recordedChecksum(binary);
  if (expected === null) return 'missing';
  try {
    const stamp = stampOf(binary);
    try {
      if (readFileSync(`${binary}.verified`, 'ascii') === stamp) return 'ok';
    } catch {
      // No stamp yet: hash below.
    }
    if (createHash('sha256').update(readFileSync(binary)).digest('hex') !== expected) return 'changed';
    writeStamp(binary, stamp);
    return 'ok';
  } catch {
    return 'changed';
  }
}

export function isVerified(binary) {
  return installedState(binary) === 'ok';
}

/** The first location holding a verified binary, null if none; throws when one changed after verification. */
function findInstalled(locations, label = 'Neuron') {
  for (const location of locations) {
    const state = installedState(location);
    if (state === 'changed') {
      throw new LauncherError(
        `${label} at ${location} does not match its recorded SHA-256 (${location}.sha256), so it was not run. ` +
          `Delete ${location}, ${location}.sha256 and ${location}.verified to download a fresh, verified copy.`,
        {kind: 'integrity'},
      );
    }
    if (state === 'ok') return location;
  }
  return null;
}

function describeFailure(error) {
  if (error?.name === 'TimeoutError') return `timed out (${error.message})`;
  const cause = error?.cause;
  return cause?.code || cause?.message || error?.message || String(error);
}

/**
 * An abort signal that fires when `touch()` has not been called for `ms`
 * milliseconds. Call `clear()` when the transfer is over.
 */
export function idleTimeout(ms) {
  const controller = new AbortController();
  let timer;
  const touch = () => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      controller.abort(new DOMException(`no data received for ${ms / 1000} s`, 'TimeoutError'));
    }, ms);
  };
  touch();
  return {signal: controller.signal, touch, clear: () => clearTimeout(timer)};
}

async function request(url, version, idle) {
  let response;
  try {
    response = await fetch(url, {redirect: 'follow', signal: idle.signal});
    idle.touch();
  } catch (error) {
    throw new LauncherError(
      `Could not download Neuron ${version} from ${url}: ${describeFailure(error)}. ` +
        'The first run needs access to GitHub Releases; later runs work offline. Check your network or proxy ' +
        '(Node 24+: set NODE_USE_ENV_PROXY=1 with HTTPS_PROXY), or set NEURON_BINARY_PATH to a local neuron binary.',
      {cause: error, kind: 'network'},
    );
  }
  if (response.status === 404) {
    throw new LauncherError(
      `Neuron ${version} has no release asset at ${url} (HTTP 404). ` +
        `The release may not be published yet; see ${REPOSITORY}/releases.`,
      {kind: 'missing'},
    );
  }
  if (!response.ok) {
    throw new LauncherError(`Downloading ${url} failed with HTTP ${response.status}. Try again later.`, {kind: 'network'});
  }
  return response;
}

function replace(temporary, destination) {
  try {
    renameSync(temporary, destination);
  } catch (error) {
    // Windows cannot replace a running executable; another process may have
    // installed the same verified binary in the meantime.
    if (isVerified(destination)) return;
    throw error;
  }
}

async function fetchText(url, version, idleTimeoutMs) {
  const idle = idleTimeout(idleTimeoutMs);
  try {
    const response = await request(url, version, idle);
    try {
      return await response.text();
    } catch (error) {
      throw new LauncherError(`Downloading ${url} was interrupted: ${describeFailure(error)}. Try again.`, {
        cause: error,
        kind: 'network',
      });
    }
  } finally {
    idle.clear();
  }
}

/**
 * Downloads `url` to `destination` after checking it against `url.sha256` and against the
 * release's SHA256SUMS at `sumsUrl` (required; every failure to get or match it is fatal).
 * `idleTimeoutMs` bounds each wait for data, not the whole transfer.
 */
export async function downloadBinary({
  destination,
  url,
  sumsUrl,
  version,
  asset,
  platform = process.platform,
  log = () => {},
  idleTimeoutMs = IDLE_TIMEOUT_MS,
  label = 'Neuron',
}) {
  if (!sumsUrl) throw new LauncherError(`No SHA256SUMS location for ${asset}; refusing to install it unchecked.`, {kind: 'integrity'});
  mkdirSync(path.dirname(destination), {recursive: true});
  log(`Downloading ${label} ${version} (${asset}) to ${path.dirname(destination)} ...`);
  const listed = parseSums(await fetchText(sumsUrl, version, idleTimeoutMs), asset);
  if (!listed) {
    throw new LauncherError(`The release SHA256SUMS (${sumsUrl}) is empty or does not list ${asset}. Nothing was installed.`, {
      kind: 'integrity',
    });
  }
  const expected = parseChecksum(await fetchText(`${url}.sha256`, version, idleTimeoutMs));
  if (!expected) throw new LauncherError(`${url}.sha256 does not contain a SHA-256 checksum.`, {kind: 'integrity'});

  const temporary = `${destination}.${process.pid}.${Date.now()}.tmp`;
  const sidecarTemporary = `${temporary}.sha256`;
  const idle = idleTimeout(idleTimeoutMs);
  try {
    const response = await request(url, version, idle);
    const hash = createHash('sha256');
    const tap = new Transform({
      transform(chunk, _encoding, callback) {
        idle.touch();
        hash.update(chunk);
        callback(null, chunk);
      },
    });
    try {
      await pipeline(Readable.fromWeb(response.body), tap, createWriteStream(temporary, {mode: 0o755}));
    } catch (error) {
      if (error?.code && ['EACCES', 'EPERM', 'EROFS', 'ENOSPC'].includes(error.code)) throw error;
      throw new LauncherError(`Downloading ${url} was interrupted: ${describeFailure(error)}. Try again.`, {
        cause: error,
        kind: 'network',
      });
    }
    const actual = hash.digest('hex');
    if (actual !== expected) {
      throw new LauncherError(
        `Neuron ${version} (${asset}) failed its SHA-256 check: expected ${expected}, got ${actual}. ` +
          'The download was discarded; nothing was installed.',
        {kind: 'integrity'},
      );
    }
    if (actual !== listed) {
      throw new LauncherError(
        `Neuron ${version} (${asset}) is not what the release SHA256SUMS (${sumsUrl}) lists: it lists ${listed}, got ${actual}. ` +
          'The download was discarded; nothing was installed.',
        {kind: 'integrity'},
      );
    }
    if (platform !== 'win32') chmodSync(temporary, 0o755);
    replace(temporary, destination);
    writeFileSync(sidecarTemporary, `${expected}  ${asset}\n`);
    replace(sidecarTemporary, `${destination}.sha256`);
    writeStamp(destination);
    return destination;
  } finally {
    idle.clear();
    rmSync(temporary, {force: true});
    rmSync(sidecarTemporary, {force: true});
  }
}

function isPermissionError(error) {
  return ['EACCES', 'EPERM', 'EROFS'].includes(error?.code);
}

/** The verified installed binary of `component`, or null (nothing is downloaded). */
export function installedBinary({
  root = PACKAGE_ROOT,
  env = process.env,
  platform = process.platform,
  arch = process.arch,
  component: componentName = 'neuron',
} = {}) {
  const version = packageVersion(root);
  const asset = assetFor(platform, arch, {component: componentName});
  return findInstalled(binaryLocations({root, version, asset, env, platform, component: componentName}));
}

/**
 * Returns a verified binary path, downloading the release asset on demand.
 * `log` receives progress lines (the launcher prints them to stderr).
 * `component` is 'neuron' (the default) or 'ui'.
 */
export async function ensureBinary({
  root = PACKAGE_ROOT,
  env = process.env,
  platform = process.platform,
  arch = process.arch,
  log = () => {},
  component: componentName = 'neuron',
} = {}) {
  const version = packageVersion(root);
  const asset = assetFor(platform, arch, {component: componentName});
  const locations = binaryLocations({root, version, asset, env, platform, component: componentName});
  const installed = findInstalled(locations, component(componentName).label);
  if (installed) return installed;

  const {label} = component(componentName);
  const url = `${downloadBase(env)}/v${version}/${asset}`;
  const sumsUrl = `${checksumsBase(env)}/v${version}/SHA256SUMS`;
  let permissionError;
  for (const destination of locations) {
    try {
      return await downloadBinary({destination, url, sumsUrl, version, asset, platform, log, label});
    } catch (error) {
      if (!isPermissionError(error)) throw error;
      permissionError = error;
    }
  }
  throw new LauncherError(
    `Cannot write the Neuron binary to ${locations.map((location) => path.dirname(location)).join(' or ')}: ` +
      `${permissionError.message}. Set NEURON_CACHE_DIR to a writable directory.`,
    {cause: permissionError, kind: 'filesystem'},
  );
}
