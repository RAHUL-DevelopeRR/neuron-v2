import { computeOrderTotal } from './pricing.js';

export class Cart {
  constructor() {
    this.items = [];
  }

  add(name, price, quantity = 1) {
    this.items.push({ name, price, quantity });
    return this;
  }

  total(taxRate) {
    return computeOrderTotal(this.items, taxRate);
  }
}
