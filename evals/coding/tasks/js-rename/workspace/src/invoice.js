import { calcTotal, formatCents } from './pricing.js';

export function invoiceLines(items, taxRate) {
  const lines = items.map(
    (item) => `${item.name} x${item.quantity}: ${formatCents(item.price * item.quantity)}`,
  );
  // calcTotal includes tax.
  lines.push(`Total: ${formatCents(calcTotal(items, taxRate))}`);
  return lines;
}
