import { test } from 'node:test';
import assert from 'node:assert/strict';

import { computeOrderTotal, Cart, invoiceLines } from '../src/index.js';

test('computeOrderTotal adds tax', () => {
  assert.equal(computeOrderTotal([{ price: 1000, quantity: 2 }], 0.1), 2200);
});

test('cart total uses computeOrderTotal', () => {
  const cart = new Cart().add('pen', 150, 2).add('pad', 300);
  assert.equal(cart.total(0), 600);
});

test('invoice ends with the total', () => {
  const lines = invoiceLines([{ name: 'pen', price: 150, quantity: 2 }], 0);
  assert.equal(lines.at(-1), 'Total: $3.00');
});
