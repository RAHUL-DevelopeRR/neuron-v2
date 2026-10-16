#!/usr/bin/env node
// Builds THIRD_PARTY_NOTICES.md for the native `neuron` binary.
//
// The crate list comes from `cargo metadata --format-version 1 --offline` and
// covers the non-dev dependency graph of the rusty-claude-cli package for every
// release target. Each crate's license and notice files are read from its
// source directory in the local cargo registry, so run `cargo fetch --locked`
// first on a machine that has never built the workspace.
//
// Usage:
//   node scripts/generate-third-party-notices.mjs [--output FILE|-]
//     [--manifest-path rust/Cargo.toml] [--package rusty-claude-cli]
//     [--target TRIPLE]... [--metadata FILE]... [--skills-dir DIR]
//     [--no-license-texts] [--online]

import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {existsSync, readdirSync, readFileSync, statSync, writeFileSync} from 'node:fs';
import path from 'node:path';
import {fileURLToPath, pathToFileURL} from 'node:url';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

export const RELEASE_TARGETS = Object.freeze([
  'x86_64-unknown-linux-gnu',
  'aarch64-unknown-linux-gnu',
  'aarch64-apple-darwin',
  'x86_64-apple-darwin',
  'x86_64-pc-windows-msvc',
]);

const LICENSE_FILE_PATTERN = /^(licen[cs]e|copying|notice|unlicense|copyright)([-._].*)?$/i;

function usage() {
  return `Usage: node scripts/generate-third-party-notices.mjs [options]

Options:
  --output FILE          Write to FILE (default THIRD_PARTY_NOTICES.md; "-" for stdout)
  --manifest-path PATH   Cargo workspace manifest (default rust/Cargo.toml)
  --package NAME         Root package (default rusty-claude-cli)
  --target TRIPLE        Release target to include; repeatable (default: all release targets)
  --metadata FILE        Read cargo metadata JSON from FILE instead of running cargo; repeatable
  --skills-dir DIR       Bundled skills directory (default rust/crates/runtime/bundled-skills)
  --no-license-texts     List crates without reproducing license files
  --online               Allow cargo to use the network (default --offline)
  -h, --help             Show this help
`;
}

export function parseArgs(argv) {
  const options = {
    output: 'THIRD_PARTY_NOTICES.md',
    manifestPath: path.join(REPO_ROOT, 'rust', 'Cargo.toml'),
    packageName: 'rusty-claude-cli',
    targets: [],
    metadataFiles: [],
    skillsDir: path.join(REPO_ROOT, 'rust', 'crates', 'runtime', 'bundled-skills'),
    licenseTexts: true,
    offline: true,
    help: false,
  };
  const valueFor = (index, flag) => {
    const value = argv[index + 1];
    if (value === undefined || value.startsWith('--')) throw new Error(`${flag} needs a value`);
    return value;
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    switch (argument) {
      case '--output':
        options.output = valueFor(index, argument);
        index += 1;
        break;
      case '--manifest-path':
        options.manifestPath = path.resolve(valueFor(index, argument));
        index += 1;
        break;
      case '--package':
        options.packageName = valueFor(index, argument);
        index += 1;
        break;
      case '--target':
        options.targets.push(valueFor(index, argument));
        index += 1;
        break;
      case '--metadata':
        options.metadataFiles.push(path.resolve(valueFor(index, argument)));
        index += 1;
        break;
      case '--skills-dir':
        options.skillsDir = path.resolve(valueFor(index, argument));
        index += 1;
        break;
      case '--no-license-texts':
        options.licenseTexts = false;
        break;
      case '--online':
        options.offline = false;
        break;
      case '-h':
      case '--help':
        options.help = true;
        break;
      default:
        throw new Error(`unknown argument: ${argument}`);
    }
  }
  if (options.targets.length === 0) options.targets = [...RELEASE_TARGETS];
  return options;
}

export function runCargoMetadata({manifestPath, target, offline}) {
  const args = ['metadata', '--format-version', '1', '--locked', '--manifest-path', manifestPath];
  if (offline) args.push('--offline');
  if (target) args.push('--filter-platform', target);
  const stdout = execFileSync('cargo', args, {
    encoding: 'utf8',
    maxBuffer: 256 * 1024 * 1024,
    stdio: ['ignore', 'pipe', 'inherit'],
  });
  return JSON.parse(stdout);
}

function isDistributedDependency(dependency) {
  // Dev-dependencies never reach the shipped binary; normal and build
  // dependencies (including proc-macros) are listed conservatively.
  const kinds = dependency.dep_kinds ?? [{kind: null}];
  return kinds.some((entry) => entry.kind !== 'dev');
}

/**
 * Returns the third-party packages reachable from `packageName` through
 * non-dev dependency edges, excluding workspace members.
 */
export function collectDependencies(metadata, packageName) {
  const packagesById = new Map(metadata.packages.map((pkg) => [pkg.id, pkg]));
  const workspace = new Set(metadata.workspace_members);
  const root = metadata.packages.find((pkg) => pkg.name === packageName && workspace.has(pkg.id));
  if (!root) throw new Error(`package ${packageName} is not a workspace member`);
  if (!metadata.resolve) throw new Error('cargo metadata output has no resolve graph');
  const nodesById = new Map(metadata.resolve.nodes.map((node) => [node.id, node]));

  const seen = new Set([root.id]);
  const queue = [root.id];
  const result = [];
  while (queue.length > 0) {
    const node = nodesById.get(queue.shift());
    if (!node) continue;
    for (const dependency of node.deps ?? []) {
      if (!isDistributedDependency(dependency) || seen.has(dependency.pkg)) continue;
      seen.add(dependency.pkg);
      queue.push(dependency.pkg);
      const pkg = packagesById.get(dependency.pkg);
      if (pkg && !workspace.has(pkg.id)) result.push(pkg);
    }
  }
  return result;
}

function repositoryOf(pkg) {
  return pkg.repository || pkg.homepage || `https://crates.io/crates/${pkg.name}`;
}

function licenseOf(pkg) {
  if (pkg.license) return pkg.license;
  if (pkg.license_file) return `see ${pkg.license_file}`;
  return 'UNKNOWN';
}

export function findLicenseFiles(pkg) {
  if (!pkg.manifest_path) return [];
  const directory = path.dirname(pkg.manifest_path);
  if (!existsSync(directory)) return [];
  const names = readdirSync(directory)
    .filter((name) => LICENSE_FILE_PATTERN.test(name))
    .filter((name) => statSync(path.join(directory, name)).isFile());
  if (pkg.license_file) {
    const declared = path.resolve(directory, pkg.license_file);
    const relative = path.relative(directory, declared);
    if (existsSync(declared) && !names.includes(relative)) names.push(relative);
  }
  return names.sort().map((name) => {
    const text = readFileSync(path.join(directory, name), 'utf8').replace(/\r\n?/g, '\n').trim();
    return {name, text};
  });
}

/** Merges several dependency lists (one per target) into a sorted, de-duplicated list. */
export function mergePackages(lists) {
  const merged = new Map();
  for (const list of lists) {
    for (const pkg of list) merged.set(`${pkg.name}@${pkg.version}`, pkg);
  }
  return [...merged.values()].sort(
    (left, right) => left.name.localeCompare(right.name) || left.version.localeCompare(right.version, undefined, {numeric: true}),
  );
}

function escapeCell(value) {
  return String(value).replace(/\|/g, '\\|').replace(/\n/g, ' ');
}

function fence(text) {
  let marker = '```';
  while (text.includes(marker)) marker += '`';
  return `${marker}text\n${text}\n${marker}`;
}

export function readBundledSkills(skillsDir) {
  const sourcesPath = path.join(skillsDir, 'SOURCES.json');
  if (!existsSync(sourcesPath)) return [];
  const sources = JSON.parse(readFileSync(sourcesPath, 'utf8'));
  return Object.entries(sources)
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([name, source]) => {
      const licensePath = path.join(skillsDir, 'licenses', `${name}.txt`);
      return {
        name,
        repository: source.repository,
        commit: source.commit,
        license: source.license,
        text: existsSync(licensePath) ? readFileSync(licensePath, 'utf8').replace(/\r\n?/g, '\n').trim() : null,
      };
    });
}

/**
 * Renders the notices document. `packages` must already be merged and sorted;
 * `licenseFiles(pkg)` returns `[{name, text}]` for a package.
 */
export function renderNotices({rootName, rootVersion, targets, packages, skills, licenseFiles, includeTexts}) {
  const lines = [
    '# Third-party notices',
    '',
    `This file lists the third-party software in the \`neuron\` native binary built from`,
    `\`${rootName}\` ${rootVersion} for ${targets.map((target) => `\`${target}\``).join(', ')}.`,
    'It is generated by `scripts/generate-third-party-notices.mjs` from `cargo metadata`;',
    'do not edit it by hand. Neuron itself is MIT licensed; see LICENSE and NOTICE.',
    '',
    `## Rust crates (${packages.length})`,
    '',
    '| Crate | Version | License | Repository |',
    '| --- | --- | --- | --- |',
  ];
  for (const pkg of packages) {
    lines.push(`| ${escapeCell(pkg.name)} | ${escapeCell(pkg.version)} | ${escapeCell(licenseOf(pkg))} | ${escapeCell(repositoryOf(pkg))} |`);
  }

  const unknown = packages.filter((pkg) => licenseOf(pkg) === 'UNKNOWN');
  if (unknown.length > 0) {
    lines.push('', '### Crates without license metadata', '');
    for (const pkg of unknown) lines.push(`- ${pkg.name} ${pkg.version}`);
  }

  if (skills.length > 0) {
    lines.push('', '## Bundled skills', '', 'These instruction bundles are embedded in the binary.', '');
    lines.push('| Skill | License | Repository | Commit |', '| --- | --- | --- | --- |');
    for (const skill of skills) {
      lines.push(`| ${escapeCell(skill.name)} | ${escapeCell(skill.license)} | ${escapeCell(skill.repository)} | ${escapeCell(skill.commit)} |`);
    }
  }

  if (includeTexts) {
    const groups = new Map();
    const withoutFiles = [];
    for (const pkg of packages) {
      const files = licenseFiles(pkg);
      if (files.length === 0) {
        withoutFiles.push(pkg);
        continue;
      }
      for (const file of files) {
        const key = createHash('sha256').update(file.text).digest('hex');
        if (!groups.has(key)) groups.set(key, {text: file.text, names: new Set(), users: []});
        const group = groups.get(key);
        group.names.add(file.name);
        group.users.push(`${pkg.name} ${pkg.version}`);
      }
    }
    lines.push('', '## License and notice texts', '');
    lines.push('Identical texts are listed once with every crate that ships them.');
    for (const group of [...groups.values()].sort((left, right) => left.users[0].localeCompare(right.users[0]))) {
      lines.push('', `### ${[...group.names].sort().join(', ')}`, '', `Used by: ${group.users.join(', ')}`, '', fence(group.text));
    }
    if (withoutFiles.length > 0) {
      lines.push('', '### Crates that ship no license file', '');
      lines.push('The standard text of the SPDX license named for each crate applies.', '');
      for (const pkg of withoutFiles) lines.push(`- ${pkg.name} ${pkg.version}: ${licenseOf(pkg)}`);
    }
    for (const skill of skills.filter((entry) => entry.text)) {
      lines.push('', `### Bundled skill: ${skill.name}`, '', fence(skill.text));
    }
  }
  lines.push('');
  return lines.join('\n');
}

export function generate(options) {
  const metadataList = options.metadataFiles.length > 0
    ? options.metadataFiles.map((file) => JSON.parse(readFileSync(file, 'utf8')))
    : options.targets.map((target) => runCargoMetadata({manifestPath: options.manifestPath, target, offline: options.offline}));
  const lists = metadataList.map((metadata) => collectDependencies(metadata, options.packageName));
  const first = metadataList[0];
  const workspace = new Set(first.workspace_members);
  const root = first.packages.find((pkg) => pkg.name === options.packageName && workspace.has(pkg.id));
  return renderNotices({
    rootName: options.packageName,
    rootVersion: root.version,
    targets: options.targets,
    packages: mergePackages(lists),
    skills: readBundledSkills(options.skillsDir),
    licenseFiles: findLicenseFiles,
    includeTexts: options.licenseTexts,
  });
}

function main() {
  let options;
  try {
    options = parseArgs(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`${error.message}\n\n${usage()}`);
    process.exit(2);
  }
  if (options.help) {
    process.stdout.write(usage());
    return;
  }
  let document;
  try {
    document = generate(options);
  } catch (error) {
    process.stderr.write(`generate-third-party-notices: ${error.message}\n`);
    if (options.offline) {
      process.stderr.write('If crates are missing locally, run `cargo fetch --locked` in rust/ (or pass --online).\n');
    }
    process.exit(1);
  }
  if (options.output === '-') {
    process.stdout.write(document);
  } else {
    writeFileSync(options.output, document);
    process.stderr.write(`wrote ${options.output}\n`);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main();
}
