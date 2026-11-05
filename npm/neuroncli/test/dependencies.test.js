import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import path from 'node:path';
import {test} from 'node:test';
import {PACKAGE_ROOT} from './helpers.js';

const read = (file) => readFileSync(path.join(PACKAGE_ROOT, file), 'utf8');

// codebase-memory-mcp's package only downloads a release binary in a postinstall
// script. Neuron fetches that binary itself and checks its pinned sha256, so the
// package (and its script) must not come with Neuron.
test('no dependency brings an install script, and Codebase memory is not an npm dependency', () => {
  const manifest = JSON.parse(read('package.json'));
  assert.deepEqual(Object.keys(manifest.dependencies), ['@playwright/mcp']);

  const lock = JSON.parse(read('package-lock.json'));
  for (const [where, entry] of Object.entries(lock.packages)) {
    if (where === '') continue;
    assert.ok(!where.includes('codebase-memory-mcp'), where);
    assert.notEqual(entry.hasInstallScript, true, `${where} runs an install script`);
    assert.match(entry.integrity, /^sha512-/, where);
  }
  assert.doesNotMatch(read('bin/neuron.js'), /NEURON_CODEBASE_MCP_ENTRY =/);
});
