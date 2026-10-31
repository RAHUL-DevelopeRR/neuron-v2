"""Stock levels per SKU for one warehouse."""


class Warehouse:
    def __init__(self, max_capacity=100):
        self.max_capacity = max_capacity
        self.stock = {}

    def level(self, sku):
        return self.stock.get(sku, 0)

    def receive_return(self, sku, qty):
        if qty <= 0:
            raise ValueError("qty must be positive")
        current = self.stock.get(sku, 0)
        self.stock[sku] = current + qty
        return self.stock[sku]

    def restock(self, sku, qty):
        if qty <= 0:
            raise ValueError("qty must be positive")
        current = self.stock.get(sku, 0)
        self.stock[sku] = min(current + qty, self.max_capacity)
        return self.stock[sku]

    def ship(self, sku, qty):
        if qty <= 0:
            raise ValueError("qty must be positive")
        current = self.stock.get(sku, 0)
        if qty > current:
            raise ValueError(f"only {current} of {sku} in stock")
        self.stock[sku] = current - qty
        return self.stock[sku]
