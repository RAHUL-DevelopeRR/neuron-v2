import {createHash} from 'node:crypto';
import {chmodSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync} from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const manifest = JSON.parse(readFileSync(path.join(packageRoot, 'package.json'), 'utf8'));
const assetByPlatform = new Map([
  ['win32-x64', 'neuron-windows-x64.exe'],
  ['linux-x64', 'neuron-linux-x64'],
  ['darwin-arm64', 'neuron-macos-arm64'],
]);
const asset = assetByPlatform.get(`${process.platform}-${process.arch}`);

if (!asset) {
  throw new Error(`NeuronCLI does not yet publish a binary for ${process.platform} ${process.arch}.`);
}

const release = `https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/v${manifest.version}`;
const directory = path.join(packageRoot, 'bin', 'native');
const binary = path.join(directory, process.platform === 'win32' ? 'neuron.exe' : 'neuron');
const temporary = `${binary}.${process.pid}.tmp`;

async function fetchBytes(url) {
  const response = await fetch(url, {redirect: 'follow', signal: AbortSignal.timeout(60_000)});
  if (!response.ok) throw new Error(`Download failed (${response.status}): ${url}`);
  return Buffer.from(await response.arrayBuffer());
}

mkdirSync(directory, {recursive: true});
try {
  const checksumFile = await fetchBytes(`${release}/${asset}.sha256`);
  const expected = checksumFile.toString('ascii').trim().match(/^([a-f0-9]{64})\s+/i)?.[1].toLowerCase();
  if (!expected) throw new Error(`Invalid SHA-256 file for NeuronCLI ${manifest.version}.`);

  const content = await fetchBytes(`${release}/${asset}`);
  const actual = createHash('sha256').update(content).digest('hex');
  if (actual !== expected) throw new Error(`NeuronCLI ${manifest.version} binary failed its SHA-256 check.`);

  writeFileSync(temporary, content, {mode: 0o755});
  if (process.platform !== 'win32') chmodSync(temporary, 0o755);
  renameSync(temporary, binary);
} finally {
  rmSync(temporary, {force: true});
}
