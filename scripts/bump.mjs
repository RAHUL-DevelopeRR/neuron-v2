#!/usr/bin/env node

/**
 * NeuronCLI — Unified Version Bumper
 *
 * Bumps version across ALL 4 locations in one command:
 *   1. rust/Cargo.toml         (workspace version)
 *   2. rust/crates/.../main.rs (VERSION const)
 *   3. package.json            (npm version)
 *   4. pyproject.toml          (PyPI version)
 *
 * Usage:
 *   node scripts/bump.mjs patch   →  6.2.3  → 6.2.4
 *   node scripts/bump.mjs minor   →  6.2.3  → 6.3.0
 *   node scripts/bump.mjs major   →  6.2.3  → 7.0.0
 *   node scripts/bump.mjs 6.3.0   →  sets exact version
 */

import { readFileSync, writeFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { execSync } from "node:child_process";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(__dirname, "..");

// ── Files to patch ──────────────────────────────────────────
const FILES = [
  {
    path: "package.json",
    pattern: /"version":\s*"[\d.]+"/,
    replace: (v) => `"version": "${v}"`,
  },
  {
    path: "pyproject.toml",
    pattern: /^version\s*=\s*"[\d.]+"/m,
    replace: (v) => `version = "${v}"`,
  },
  {
    path: "rust/Cargo.toml",
    pattern: /^version\s*=\s*"[\d.]+"/m,
    replace: (v) => `version = "${v}"`,
  },
  {
    path: "rust/crates/rusty-claude-cli/src/main.rs",
    pattern: /const VERSION:\s*&str\s*=\s*"[\d.]+"/,
    replace: (v) => `const VERSION: &str = "${v}"`,
  },
];

// ── Read current version from package.json ──────────────────
function currentVersion() {
  const pkg = JSON.parse(readFileSync(resolve(ROOT, "package.json"), "utf8"));
  return pkg.version;
}

// ── Bump logic ──────────────────────────────────────────────
function bumpVersion(current, type) {
  const [major, minor, patch] = current.split(".").map(Number);
  switch (type) {
    case "patch":
      return `${major}.${minor}.${patch + 1}`;
    case "minor":
      return `${major}.${minor + 1}.0`;
    case "major":
      return `${major + 1}.0.0`;
    default:
      // Exact version string
      if (/^\d+\.\d+\.\d+$/.test(type)) return type;
      console.error(`  ✗ Invalid bump type: ${type}`);
      console.error(`  Usage: node scripts/bump.mjs [patch|minor|major|X.Y.Z]`);
      process.exit(1);
  }
}

// ── Main ────────────────────────────────────────────────────
const type = process.argv[2];
if (!type) {
  console.error("  Usage: node scripts/bump.mjs [patch|minor|major|X.Y.Z]");
  process.exit(1);
}

const current = currentVersion();
const next = bumpVersion(current, type);

console.log(`\n  ⬆ Bumping ${current} → ${next}\n`);

for (const file of FILES) {
  const fullPath = resolve(ROOT, file.path);
  try {
    let content = readFileSync(fullPath, "utf8");
    if (file.pattern.test(content)) {
      content = content.replace(file.pattern, file.replace(next));
      writeFileSync(fullPath, content);
      console.log(`  ✓ ${file.path}`);
    } else {
      console.log(`  ⚠ ${file.path} — pattern not found, skipped`);
    }
  } catch (err) {
    console.log(`  ✗ ${file.path} — ${err.message}`);
  }
}

console.log(`\n  ✓ All files bumped to ${next}`);
console.log(`\n  Next steps:`);
console.log(`    cargo build --release`);
console.log(`    copy neuron.exe → neuron_cli/`);
console.log(`    npm publish --access public`);
console.log(`    python -m build && twine upload dist/*${next}*`);
console.log();
