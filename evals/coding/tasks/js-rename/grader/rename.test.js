import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

import * as shop from '../src/index.js';

test('the new name is exported and works', () => {
  assert.equal(typeof shop.computeOrderTotal, 'function');
  assert.equal(shop.computeOrderTotal([{ price: 1000, quantity: 2 }], 0.1), 2200);
  assert.equal(shop.computeOrderTotal([{ price: 100, quantity: 1 }]), 108);
});

test('the old name is gone from the exports', () => {
  assert.equal('calcTotal' in shop, false);
});

test('callers still work', () => {
  const cart = new shop.Cart().add('pen', 150, 2).add('pad', 300);
  assert.equal(cart.total(0), 600);
  const lines = shop.invoiceLines([{ name: 'pen', price: 150, quantity: 2 }], 0);
  assert.equal(lines.at(-1), 'Total: $3.00');
});

test('no source or test file mentions the old name', () => {
  for (const dir of ['src', 'test']) {
    for (const name of readdirSync(dir)) {
      const text = readFileSync(join(dir, name), 'utf8');
      assert.equal(text.includes('calcTotal'), false, `${dir}/${name} still mentions calcTotal`);
    }
  }
});
