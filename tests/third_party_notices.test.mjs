// node --test tests/third_party_notices.test.mjs
import assert from 'node:assert/strict';
import {mkdirSync, mkdtempSync, rmSync, writeFileSync} from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {test} from 'node:test';
import {
  RELEASE_TARGETS,
  collectDependencies,
  findLicenseFiles,
  mergePackages,
  parseArgs,
  readBundledSkills,
  renderNotices,
} from '../scripts/generate-third-party-notices.mjs';

function crate(root, name, version, fields = {}, files = {}) {
  const directory = path.join(root, `${name}-${version}`);
  mkdirSync(directory, {recursive: true});
  writeFileSync(path.join(directory, 'Cargo.toml'), `[package]\nname = "${name}"\n`);
  for (const [file, text] of Object.entries(files)) writeFileSync(path.join(directory, file), text);
  return {
    id: `registry+https://github.com/rust-lang/crates.io-index#${name}@${version}`,
    name,
    version,
    license: 'MIT',
    repository: `https://github.com/example/${name}`,
    manifest_path: path.join(directory, 'Cargo.toml'),
    ...fields,
  };
}

function fixture(t) {
  const root = mkdtempSync(path.join(os.tmpdir(), 'notices-'));
  t.after(() => rmSync(root, {recursive: true, force: true}));
  const cli = {id: 'path+file:///ws/crates/rusty-claude-cli#6.3.0', name: 'rusty-claude-cli', version: '6.3.0', license: 'MIT'};
  const runtime = {id: 'path+file:///ws/crates/runtime#6.3.0', name: 'runtime', version: '6.3.0', license: 'MIT'};
  const serde = crate(root, 'serde', '1.0.0', {license: 'MIT OR Apache-2.0'}, {'LICENSE-MIT': 'MIT text\r\n', 'LICENSE-APACHE': 'Apache text'});
  const itoa = crate(root, 'itoa', '1.0.0', {}, {LICENSE: 'MIT text\n'});
  const cc = crate(root, 'cc', '1.2.0', {}, {'LICENSE-MIT': 'cc MIT text'});
  const tempfile = crate(root, 'tempfile', '3.0.0', {}, {LICENSE: 'dev only'});
  const odd = crate(root, 'odd', '0.1.0', {license: null, license_file: 'COPYING', repository: null});
  const pipes = crate(root, 'pipes', '0.2.0', {license: 'A|B', repository: null});
  const edge = (pkg, kind = null) => ({name: pkg.name, pkg: pkg.id, dep_kinds: [{kind, target: null}]});
  const metadata = {
    packages: [cli, runtime, serde, itoa, cc, tempfile, odd, pipes],
    workspace_members: [cli.id, runtime.id],
    resolve: {
      nodes: [
        {id: cli.id, deps: [edge(runtime), edge(serde), edge(tempfile, 'dev')]},
        {id: runtime.id, deps: [edge(itoa), edge(cc, 'build'), edge(odd)]},
        {id: serde.id, deps: [edge(pipes)]},
        {id: itoa.id, deps: []},
        {id: cc.id, deps: []},
        {id: tempfile.id, deps: []},
        {id: odd.id, deps: []},
        {id: pipes.id, deps: []},
      ],
    },
  };
  return {root, metadata};
}

test('collects the non-dev dependency graph without workspace members', (t) => {
  const {metadata} = fixture(t);
  const names = collectDependencies(metadata, 'rusty-claude-cli').map((pkg) => pkg.name).sort();
  assert.deepEqual(names, ['cc', 'itoa', 'odd', 'pipes', 'serde']);
  assert.throws(() => collectDependencies(metadata, 'serde'), /not a workspace member/);
});

test('merges per-target lists, de-duplicates and sorts', (t) => {
  const {metadata} = fixture(t);
  const list = collectDependencies(metadata, 'rusty-claude-cli');
  const merged = mergePackages([list, list.slice(0, 2)]);
  assert.equal(merged.length, list.length);
  assert.deepEqual(merged.map((pkg) => pkg.name), ['cc', 'itoa', 'odd', 'pipes', 'serde']);
});

test('renders a crate table, skills and de-duplicated license texts', (t) => {
  const {root, metadata} = fixture(t);
  const skillsDir = path.join(root, 'skills');
  mkdirSync(path.join(skillsDir, 'licenses'), {recursive: true});
  writeFileSync(
    path.join(skillsDir, 'SOURCES.json'),
    JSON.stringify({gstack: {repository: 'https://github.com/garrytan/gstack', commit: 'abc123', license: 'MIT'}}),
  );
  writeFileSync(path.join(skillsDir, 'licenses', 'gstack.txt'), 'gstack MIT text');
  const document = renderNotices({
    rootName: 'rusty-claude-cli',
    rootVersion: '6.3.0',
    targets: ['x86_64-unknown-linux-gnu'],
    packages: mergePackages([collectDependencies(metadata, 'rusty-claude-cli')]),
    skills: readBundledSkills(skillsDir),
    licenseFiles: findLicenseFiles,
    includeTexts: true,
  });
  assert.match(document, /^# Third-party notices/);
  assert.match(document, /## Rust crates \(5\)/);
  assert.match(document, /\| serde \| 1\.0\.0 \| MIT OR Apache-2\.0 \| https:\/\/github\.com\/example\/serde \|/);
  assert.match(document, /\| odd \| 0\.1\.0 \| see COPYING \| https:\/\/crates\.io\/crates\/odd \|/);
  assert.match(document, /\| pipes \| 0\.2\.0 \| A\\\|B \|/);
  assert.doesNotMatch(document, /tempfile/);
  assert.match(document, /\| gstack \| MIT \| https:\/\/github\.com\/garrytan\/gstack \| abc123 \|/);
  // serde's LICENSE-MIT and itoa's LICENSE share one text block after CRLF normalisation.
  assert.match(document, /### LICENSE, LICENSE-MIT\n\nUsed by: itoa 1\.0\.0, serde 1\.0\.0\n\n```text\nMIT text\n```/);
  assert.match(document, /### Crates that ship no license file\n\nThe standard text[^\n]*\n\n- odd 0\.1\.0: see COPYING\n- pipes 0\.2\.0: A\|B/);
  assert.match(document, /### Bundled skill: gstack\n\n```text\ngstack MIT text\n```/);
});

test('argument parsing defaults to every release target', () => {
  const options = parseArgs(['--output', '-', '--no-license-texts', '--target', 'aarch64-apple-darwin']);
  assert.equal(options.output, '-');
  assert.equal(options.licenseTexts, false);
  assert.deepEqual(options.targets, ['aarch64-apple-darwin']);
  assert.deepEqual(parseArgs([]).targets, [...RELEASE_TARGETS]);
  assert.equal(RELEASE_TARGETS.length, 5);
  assert.throws(() => parseArgs(['--bogus']), /unknown argument/);
  assert.throws(() => parseArgs(['--output']), /needs a value/);
});
