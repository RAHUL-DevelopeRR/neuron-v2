#!/usr/bin/env node

/**
 * NeuronCLI — Universal Launcher
 *
 * This is the npm bin entry point. It:
 * 1. Locates the bundled native binary (neuron.exe / neuron)
 * 2. Forwards all CLI arguments to the native engine
 * 3. Handles --tui flag to launch the React/Ink TUI instead
 *
 * Install: npm install -g @anthropic-ai/neuron
 * Usage:   neuron [args]          — launches native CLI
 *          neuron --tui           — launches React TUI preview
 *          neuron --version       — shows version
 *          neuron auth status     — shows auth state
 */

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { platform, arch, env } from "node:process";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const ROOT = resolve(__dirname, "..");

// ── Locate the native binary ─────────────────────────────────

function goPlatform() {
  if (platform === "win32") return "windows";
  if (platform === "darwin") return "darwin";
  return platform;
}

function goArch() {
  if (arch === "x64") return "amd64";
  return arch;
}

function findBinary() {
  const binaryName = platform === "win32" ? "neuron.exe" : "neuron";
  const platformBinary = `neuron-${goPlatform()}-${goArch()}${platform === "win32" ? ".exe" : ""}`;

  // 1. Check bundled platform-specific location (inside npm package).
  const bundledPlatform = join(ROOT, "neuron_cli", "bin", platformBinary);
  if (existsSync(bundledPlatform)) return bundledPlatform;

  // 2. Check legacy bundled location.
  const bundled = join(ROOT, "neuron_cli", binaryName);
  if (existsSync(bundled)) return bundled;

  // 3. Check Go build output (development).
  const goBuild = join(ROOT, "go", binaryName);
  if (existsSync(goBuild)) return goBuild;

  // 4. Check Rust build output (development).
  const devBuild = join(ROOT, "rust", "target", "release", binaryName);
  if (existsSync(devBuild)) return devBuild;

  // 5. Check debug build.
  const debugBuild = join(ROOT, "rust", "target", "debug", binaryName);
  if (existsSync(debugBuild)) return debugBuild;

  // 6. Check PATH.
  const pathDirs = (env.PATH || "").split(platform === "win32" ? ";" : ":");
  for (const dir of pathDirs) {
    const candidate = join(dir, binaryName);
    if (existsSync(candidate)) return candidate;
  }

  console.error(`
  ✗ NeuronCLI binary not found.

  The native binary '${binaryName}' was not found in:
    - ${join(ROOT, "neuron_cli", "bin")}
    - ${join(ROOT, "neuron_cli")}
    - ${join(ROOT, "go")}
    - ${join(ROOT, "rust", "target", "release")}
    - System PATH

  To fix:
    pip install neuroncli        (includes pre-built binary)
    cd go && go build -o ${binaryName} main.go   (build from source)
  `);
  process.exit(1);
}

// ── Main ─────────────────────────────────────────────────────

const args = process.argv.slice(2);

// --tui flag → launch React TUI instead of native CLI
if (args.includes("--tui")) {
  const tuiArgs = args.filter((a) => a !== "--tui");

  // Dynamic import of tsx to run the React TUI
  try {
    const tuiEntry = join(ROOT, "ui", "react-tui", "src", "index.tsx");
    if (existsSync(tuiEntry)) {
      const child = spawn("npx", ["tsx", tuiEntry, ...tuiArgs], {
        stdio: "inherit",
        shell: true,
        env: { ...env },
      });
      child.on("exit", (code) => process.exit(code || 0));
    } else {
      console.error("  ✗ React TUI not found. Run: npm run build:tui");
      process.exit(1);
    }
  } catch (err) {
    console.error("  ✗ Failed to launch TUI:", err.message);
    process.exit(1);
  }
} else {
  // Default: launch native binary
  const binary = findBinary();
  const child = spawn(binary, args, {
    stdio: "inherit",
    env: { ...env },
  });

  child.on("error", (err) => {
    console.error(`  ✗ Failed to start NeuronCLI: ${err.message}`);
    process.exit(1);
  });

  child.on("exit", (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
    } else {
      process.exit(code || 0);
    }
  });
}
