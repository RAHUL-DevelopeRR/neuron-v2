#!/usr/bin/env node

/**
 * NeuronCLI universal npm launcher.
 *
 * Resolution order:
 * 1. Optional platform package, e.g. @zero-x/neuron-win32-x64.
 * 2. Local development builds.
 * 3. GitHub Release binary cache, verified against checksums.txt.
 */

import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { createWriteStream, existsSync, mkdirSync, readFileSync, chmodSync, renameSync } from "node:fs";
import { tmpdir, homedir } from "node:os";
import { resolve, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { platform, arch, env, exit } from "node:process";
import { get as httpsGet } from "node:https";
import { createRequire } from "node:module";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const ROOT = resolve(__dirname, "..");
const require = createRequire(import.meta.url);
const pkg = JSON.parse(readFileSync(join(ROOT, "package.json"), "utf8"));

const RELEASE_REPO = env.NEURON_RELEASE_REPOSITORY || "RAHUL-DevelopeRR/neuron-v2";

function truthy(value) {
  if (!value) return false;
  const normalized = String(value).trim().toLowerCase();
  return normalized !== "0" && normalized !== "false" && normalized !== "no";
}

function tuiEnvironment() {
  const next = { ...env };
  if (truthy(next.NEURON_NO_COLOR)) return next;
  delete next.NO_COLOR;
  next.CLICOLOR = "1";
  next.CLICOLOR_FORCE = "1";
  next.FORCE_COLOR = "3";
  next.COLORTERM = "truecolor";
  if (!next.TERM || String(next.TERM).toLowerCase() === "dumb" || platform === "win32") {
    next.TERM = "xterm-256color";
  }
  next.NEURON_INSTALL_SOURCE ||= "npm";
  return next;
}

function goPlatform() {
  if (platform === "win32") return "windows";
  if (platform === "darwin") return "darwin";
  return platform;
}

function goArch() {
  if (arch === "x64") return "amd64";
  return arch;
}

function npmArch() {
  if (arch === "x64") return "x64";
  if (arch === "arm64") return "arm64";
  return arch;
}

function platformPackageName() {
  return `@zero-x/neuron-${platform}-${npmArch()}`;
}

function binaryName() {
  return platform === "win32" ? "neuron.exe" : "neuron";
}

function releaseBinaryName() {
  return `neuron-${goPlatform()}-${goArch()}${platform === "win32" ? ".exe" : ""}`;
}

function findPlatformPackageBinary() {
  try {
    const packageJSON = require.resolve(`${platformPackageName()}/package.json`);
    const candidate = join(dirname(packageJSON), "bin", binaryName());
    if (existsSync(candidate)) return candidate;
  } catch {
    // Optional package not installed on this platform.
  }
  return "";
}

function findGoBinary() {
  const candidate = join(ROOT, "go", binaryName());
  return existsSync(candidate) ? candidate : "";
}

function findLegacyBinary() {
  const candidates = [
    join(ROOT, "neuron_cli", "bin", releaseBinaryName()),
    join(ROOT, "neuron_cli", binaryName()),
    join(ROOT, "rust", "target", "release", binaryName()),
    join(ROOT, "rust", "target", "debug", binaryName()),
  ];
  for (const candidate of candidates) {
    if (existsSync(candidate)) return candidate;
  }
  return "";
}

function cacheDir() {
  const base =
    env.XDG_CACHE_HOME ||
    (platform === "win32"
      ? env.LOCALAPPDATA || join(homedir(), "AppData", "Local")
      : join(homedir(), ".cache"));
  return join(base, "neuroncli", "bin", pkg.version, `${platform}-${npmArch()}`);
}

function releaseURL(asset) {
  return `https://github.com/${RELEASE_REPO}/releases/download/v${pkg.version}/${asset}`;
}

function download(url, target) {
  return new Promise((resolveDownload, reject) => {
    mkdirSync(dirname(target), { recursive: true });
    const request = httpsGet(url, (response) => {
      if ([301, 302, 303, 307, 308].includes(response.statusCode || 0) && response.headers.location) {
        response.resume();
        download(response.headers.location, target).then(resolveDownload, reject);
        return;
      }
      if ((response.statusCode || 0) >= 400) {
        response.resume();
        reject(new Error(`download failed: ${response.statusCode} ${response.statusMessage}`));
        return;
      }
      const file = createWriteStream(target);
      response.pipe(file);
      file.on("finish", () => file.close(resolveDownload));
      file.on("error", reject);
    });
    request.on("error", reject);
  });
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

async function verifyChecksum(asset, path) {
  const checksums = join(tmpdir(), `neuroncli-${pkg.version}-checksums.txt`);
  await download(releaseURL("checksums.txt"), checksums);
  const line = readFileSync(checksums, "utf8")
    .split(/\r?\n/)
    .find((entry) => entry.includes(asset));
  if (!line) throw new Error(`checksum entry for ${asset} not found`);
  const expected = line.trim().split(/\s+/)[0].toLowerCase();
  const actual = sha256(path);
  if (actual !== expected) {
    throw new Error(`checksum mismatch for ${asset}`);
  }
}

async function downloadReleaseBinary() {
  if (truthy(env.NEURON_DISABLE_DOWNLOAD)) return "";
  const dir = cacheDir();
  const target = join(dir, binaryName());
  if (existsSync(target)) return target;
  const asset = releaseBinaryName();
  const tmp = join(dir, `${asset}.download`);
  await download(releaseURL(asset), tmp);
  await verifyChecksum(asset, tmp);
  renameSync(tmp, target);
  if (platform !== "win32") chmodSync(target, 0o755);
  return target;
}

async function findBinary() {
  return findPlatformPackageBinary() || findGoBinary() || findLegacyBinary() || (await downloadReleaseBinary());
}

function hasGoSource() {
  return existsSync(join(ROOT, "go", "go.mod")) && existsSync(join(ROOT, "go", "main.go"));
}

async function run() {
  const args = process.argv.slice(2);
  if (args.includes("--tui")) {
    const tuiArgs = args.filter((a) => a !== "--tui");
    const tuiEntry = join(ROOT, "ui", "react-tui", "src", "index.tsx");
    if (!existsSync(tuiEntry)) {
      console.error("Neuron React TUI source is not included in this package.");
      exit(1);
    }
    const child = spawn("npx", ["tsx", tuiEntry, ...tuiArgs], {
      stdio: "inherit",
      shell: true,
      env: tuiEnvironment(),
    });
    child.on("exit", (code) => exit(code || 0));
    return;
  }

  const localBinary = findPlatformPackageBinary() || findGoBinary();
  if (!localBinary && hasGoSource() && !truthy(env.NEURON_DISABLE_GO_RUN)) {
    const child = spawn("go", ["run", ".", ...args], {
      cwd: join(ROOT, "go"),
      stdio: "inherit",
      env: tuiEnvironment(),
    });
    child.on("exit", (code, signal) => {
      if (signal) process.kill(process.pid, signal);
      exit(code || 0);
    });
    child.on("error", (err) => {
      console.error(`Failed to run local Go source: ${err.message}`);
      exit(1);
    });
    return;
  }

  const binary = localBinary || findLegacyBinary() || (await downloadReleaseBinary());
  if (!binary) {
    console.error(`
NeuronCLI native binary was not found for ${platform}/${arch}.

Tried:
  - ${platformPackageName()}
  - local Go/Rust development builds
  - GitHub Release asset ${releaseBinaryName()}

Run:
  neuron doctor
  npm install -g @zero-x/neuron
`);
    exit(1);
  }

  const child = spawn(binary, args, { stdio: "inherit", env: tuiEnvironment() });
  child.on("error", (err) => {
    console.error(`Failed to start NeuronCLI: ${err.message}`);
    exit(1);
  });
  child.on("exit", (code, signal) => {
    if (signal) process.kill(process.pid, signal);
    exit(code || 0);
  });
}

run().catch((err) => {
  console.error(`NeuronCLI launcher error: ${err.message}`);
  exit(1);
});
