import { calcTotal } from './pricing.js';

export class Cart {
  constructor() {
    this.items = [];
  }

  add(name, price, quantity = 1) {
    this.items.push({ name, price, quantity });
    return this;
  }

  total(taxRate) {
    return calcTotal(this.items, taxRate);
  }
}
