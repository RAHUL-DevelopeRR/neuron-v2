// Prices are in cents to avoid floating-point rounding.

export const DEFAULT_TAX_RATE = 0.08;

/** Sum of `price * quantity` over `items`, plus tax, rounded to whole cents. */
export function computeOrderTotal(items, taxRate = DEFAULT_TAX_RATE) {
  const subtotal = items.reduce((sum, item) => sum + item.price * item.quantity, 0);
  return Math.round(subtotal * (1 + taxRate));
}

export function formatCents(cents) {
  return `$${(cents / 100).toFixed(2)}`;
}
