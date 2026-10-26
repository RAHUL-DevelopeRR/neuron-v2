/**
 * The presence vectors `neuron engine --share` (Rust) is tested against stay what the reducer and
 * `presenceOf` give. After a change to either, regenerate them and copy the file to
 * rust/crates/neuron-share/tests/fixtures/:
 *
 *   bun run test/fixtures/presence-vectors.ts > test/fixtures/presence-vectors.json
 */
import { expect, test } from 'bun:test'
import fs from 'fs'
import path from 'path'

import vectors from './fixtures/presence-vectors'

test('the committed presence vectors match the reducer', () => {
  const committed = JSON.parse(fs.readFileSync(path.join(import.meta.dir, 'fixtures/presence-vectors.json'), 'utf8'))
  expect(committed).toEqual(JSON.parse(JSON.stringify(vectors)))
})
