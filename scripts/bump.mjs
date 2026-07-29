#!/usr/bin/env node

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(__dirname, "..");

const files = [
  ["package.json", /"version":\s*"[\d.]+"/, (v) => `"version": "${v}"`],
  ["package.json", /"@zero-x\/neuron-darwin-arm64":\s*"[\d.]+"/, (v) => `"@zero-x/neuron-darwin-arm64": "${v}"`],
  ["package.json", /"@zero-x\/neuron-darwin-x64":\s*"[\d.]+"/, (v) => `"@zero-x/neuron-darwin-x64": "${v}"`],
  ["package.json", /"@zero-x\/neuron-linux-arm64":\s*"[\d.]+"/, (v) => `"@zero-x/neuron-linux-arm64": "${v}"`],
  ["package.json", /"@zero-x\/neuron-linux-x64":\s*"[\d.]+"/, (v) => `"@zero-x/neuron-linux-x64": "${v}"`],
  ["package.json", /"@zero-x\/neuron-win32-x64":\s*"[\d.]+"/, (v) => `"@zero-x/neuron-win32-x64": "${v}"`],
  ["packaging/npm/platforms/darwin-arm64/package.json", /"version":\s*"[\d.]+"/, (v) => `"version": "${v}"`],
  ["packaging/npm/platforms/darwin-x64/package.json", /"version":\s*"[\d.]+"/, (v) => `"version": "${v}"`],
  ["packaging/npm/platforms/linux-arm64/package.json", /"version":\s*"[\d.]+"/, (v) => `"version": "${v}"`],
  ["packaging/npm/platforms/linux-x64/package.json", /"version":\s*"[\d.]+"/, (v) => `"version": "${v}"`],
  ["packaging/npm/platforms/win32-x64/package.json", /"version":\s*"[\d.]+"/, (v) => `"version": "${v}"`],
  ["pyproject.toml", /^version\s*=\s*"[\d.]+"/m, (v) => `version = "${v}"`],
  ["go/internal/version/version.go", /var Version = "[\d.]+"/, (v) => `var Version = "${v}"`],
  ["rust/Cargo.toml", /^version\s*=\s*"[\d.]+"/m, (v) => `version = "${v}"`],
  ["rust/crates/rusty-claude-cli/src/main.rs", /const VERSION:\s*&str\s*=\s*"[\d.]+"/, (v) => `const VERSION: &str = "${v}"`],
];

function currentVersion() {
  return JSON.parse(readFileSync(resolve(ROOT, "package.json"), "utf8")).version;
}

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
      if (/^\d+\.\d+\.\d+$/.test(type)) return type;
      console.error("Usage: node scripts/bump.mjs [patch|minor|major|X.Y.Z]");
      process.exit(1);
  }
}

const type = process.argv[2];
if (!type) {
  console.error("Usage: node scripts/bump.mjs [patch|minor|major|X.Y.Z]");
  process.exit(1);
}

const current = currentVersion();
const next = bumpVersion(current, type);

console.log(`Bumping ${current} -> ${next}`);

for (const [path, pattern, replace] of files) {
  const fullPath = resolve(ROOT, path);
  try {
    let content = readFileSync(fullPath, "utf8");
    if (!pattern.test(content)) {
      console.log(`skip ${path}: pattern not found`);
      continue;
    }
    content = content.replace(pattern, replace(next));
    writeFileSync(fullPath, content);
    console.log(`updated ${path}`);
  } catch (err) {
    console.log(`skip ${path}: ${err.message}`);
  }
}

console.log("");
console.log("Next steps:");
console.log(`  git tag v${next}`);
console.log(`  git push origin v${next}`);
console.log("  GitHub Actions will build GitHub, npm, PyPI, Homebrew, deb, rpm, and Windows artifacts.");
