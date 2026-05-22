#!/usr/bin/env node

/**
 * NeuronCLI — Universal Launcher
 *
 * This is the npm bin entry point. It:
 * 1. Locates the bundled Rust binary (neuron.exe / neuron)
 * 2. Forwards all CLI arguments to the Rust engine
 * 3. Handles --tui flag to launch the React/Ink TUI instead
 *
 * Install: npm install -g @anthropic-ai/neuron
 * Usage:   neuron [args]          — launches Rust CLI
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

// ── Locate the Rust binary ───────────────────────────────────

function findBinary() {
  const binaryName = platform === "win32" ? "neuron.exe" : "neuron";

  // 1. Check bundled location (inside npm package)
  const bundled = join(ROOT, "neuron_cli", binaryName);
  if (existsSync(bundled)) return bundled;

  // 2. Check build output (development)
  const devBuild = join(ROOT, "rust", "target", "release", binaryName);
  if (existsSync(devBuild)) return devBuild;

  // 3. Check debug build
  const debugBuild = join(ROOT, "rust", "target", "debug", binaryName);
  if (existsSync(debugBuild)) return debugBuild;

  // 4. Check PATH
  const pathDirs = (env.PATH || "").split(platform === "win32" ? ";" : ":");
  for (const dir of pathDirs) {
    const candidate = join(dir, binaryName);
    if (existsSync(candidate)) return candidate;
  }

  console.error(`
  ✗ NeuronCLI binary not found.

  The Rust binary '${binaryName}' was not found in:
    • ${join(ROOT, "neuron_cli")}
    • ${join(ROOT, "rust", "target", "release")}
    • System PATH

  To fix:
    pip install neuroncli        (includes pre-built binary)
    cd rust && cargo build --release   (build from source)
  `);
  process.exit(1);
}

// ── Main ─────────────────────────────────────────────────────

const args = process.argv.slice(2);

// --tui flag → launch React TUI instead of Rust CLI
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
  // Default: launch Rust binary
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
