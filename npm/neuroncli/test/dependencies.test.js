import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import path from 'node:path';
import {test} from 'node:test';
import {PACKAGE_ROOT} from './helpers.js';

const read = (file) => readFileSync(path.join(PACKAGE_ROOT, file), 'utf8');

// npm ignores the package-lock.json of a published package, so a dependency would be
// whatever tree npm resolves on the user's machine, unchecked by Neuron. The default MCP
// servers (Playwright, Codebase memory) are instead installed by Neuron from the
// downloads its catalog pins, so nothing comes with this package.
test('the package has no dependencies and passes no MCP server entry to Neuron', () => {
  const manifest = JSON.parse(read('package.json'));
  assert.equal(manifest.dependencies, undefined);
  assert.equal(manifest.optionalDependencies, undefined);

  const lock = JSON.parse(read('package-lock.json'));
  assert.deepEqual(Object.keys(lock.packages), ['']);
  assert.equal(lock.packages[''].dependencies, undefined);
  assert.doesNotMatch(read('bin/neuron.js'), /NEURON_(PLAYWRIGHT|CODEBASE)_MCP_ENTRY/);
  assert.match(read('bin/neuron.js'), /NEURON_MCP_NODE: process\.execPath/);
});
