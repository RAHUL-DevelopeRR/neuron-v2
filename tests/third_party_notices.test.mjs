// node --test tests/third_party_notices.test.mjs
import assert from 'node:assert/strict';
import {mkdirSync, mkdtempSync, rmSync, writeFileSync} from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {test} from 'node:test';
import {
  RELEASE_TARGETS,
  collectDependencies,
  collectNpmPackages,
  findLicenseFiles,
  mergePackages,
  parseArgs,
  readBundledSkills,
  readUi,
  renderNotices,
} from '../scripts/generate-third-party-notices.mjs';

/** A UI package with node_modules: a -> b (nested), optional c (installed), optional d (not). */
function uiFixture(t) {
  const root = mkdtempSync(path.join(os.tmpdir(), 'notices-ui-'));
  t.after(() => rmSync(root, {recursive: true, force: true}));
  const write = (file, value) => {
    mkdirSync(path.dirname(path.join(root, file)), {recursive: true});
    writeFileSync(path.join(root, file), typeof value === 'string' ? value : JSON.stringify(value));
  };
  write('package.json', {name: 'neuron-tui', license: 'Apache-2.0', dependencies: {a: '1.0.0'}, devDependencies: {dev: '1.0.0'}});
  write('LICENSE', 'Apache text');
  write('NOTICE', 'Codebuff notice');
  write('node_modules/a/package.json', {
    name: 'a',
    version: '1.0.0',
    license: 'MIT',
    repository: {url: 'git+https://github.com/x/a.git'},
    dependencies: {b: '2.0.0'},
    optionalDependencies: {c: '1.0.0', d: '1.0.0'},
  });
  write('node_modules/a/LICENSE', 'MIT text');
  write('node_modules/a/node_modules/b/package.json', {name: 'b', version: '2.0.0', licenses: [{type: 'ISC'}]});
  write('node_modules/c/package.json', {name: 'c', version: '1.0.0', license: 'MIT'});
  write('node_modules/c/LICENSE.md', 'MIT text\r\n');
  write('node_modules/dev/package.json', {name: 'dev', version: '1.0.0', license: 'GPL-3.0'});
  return root;
}

test('the UI notices cover its production npm closure, nested and optional packages', (t) => {
  const root = uiFixture(t);
  const packages = collectNpmPackages(root);
  assert.deepEqual(
    packages.map((pkg) => [pkg.name, pkg.version, pkg.license]),
    [['a', '1.0.0', 'MIT'], ['b', '2.0.0', 'ISC'], ['c', '1.0.0', 'MIT']],
  );
  assert.equal(packages[0].repository, 'https://github.com/x/a');

  const document = renderNotices({
    rootName: 'rusty-claude-cli',
    rootVersion: '1.0.0',
    targets: ['x86_64-unknown-linux-gnu'],
    packages: [],
    skills: [],
    licenseFiles: () => [],
    includeTexts: true,
    ui: {...readUi(root), path: 'ui/neuron-tui'},
  });
  assert.match(document, /## Full-screen UI \(`neuron-tui`\)/);
  assert.match(document, /Apache License 2\.0/);
  assert.match(document, /### npm packages \(3\)/);
  assert.match(document, /\| a \| 1\.0\.0 \| MIT \| https:\/\/github\.com\/x\/a \|/);
  assert.match(document, /### ui\/neuron-tui\/NOTICE\n\n```text\nCodebuff notice\n```/);
  // a's LICENSE and c's LICENSE.md share one block after CRLF normalisation.
  assert.match(document, /#### LICENSE, LICENSE\.md\n\nUsed by: a 1\.0\.0, c 1\.0\.0/);
  assert.match(document, /- b 2\.0\.0: ISC/);
  assert.doesNotMatch(document, /GPL-3\.0/);
});

test('a UI without node_modules is an error, not a silent omission', (t) => {
  const root = uiFixture(t);
  rmSync(path.join(root, 'node_modules'), {recursive: true, force: true});
  assert.throws(() => collectNpmPackages(root), /bun install --frozen-lockfile/);
  assert.equal(parseArgs(['--no-ui']).uiDir, null);
});

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
