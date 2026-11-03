import { computeOrderTotal, formatCents } from './pricing.js';

export function invoiceLines(items, taxRate) {
  const lines = items.map(
    (item) => `${item.name} x${item.quantity}: ${formatCents(item.price * item.quantity)}`,
  );
  // computeOrderTotal includes tax.
  lines.push(`Total: ${formatCents(computeOrderTotal(items, taxRate))}`);
  return lines;
}
