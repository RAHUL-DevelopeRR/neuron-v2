import unittest

from inventory import Warehouse


class GradeWarehouse(unittest.TestCase):
    def test_restock_is_capped(self):
        warehouse = Warehouse(max_capacity=10)
        self.assertEqual(warehouse.restock("a", 6), 6)
        self.assertEqual(warehouse.restock("a", 6), 10)
        self.assertEqual(warehouse.level("a"), 10)

    def test_restock_below_capacity_adds(self):
        warehouse = Warehouse(max_capacity=10)
        self.assertEqual(warehouse.restock("a", 3), 3)
        self.assertEqual(warehouse.restock("a", 4), 7)

    def test_returns_are_not_capped(self):
        warehouse = Warehouse(max_capacity=10)
        warehouse.restock("a", 10)
        self.assertEqual(warehouse.receive_return("a", 5), 15)

    def test_validation_is_kept(self):
        warehouse = Warehouse()
        for method in (warehouse.restock, warehouse.receive_return, warehouse.ship):
            with self.assertRaises(ValueError):
                method("a", 0)

    def test_ship_is_unchanged(self):
        warehouse = Warehouse()
        warehouse.restock("a", 5)
        self.assertEqual(warehouse.ship("a", 2), 3)
        with self.assertRaises(ValueError):
            warehouse.ship("a", 9)


if __name__ == "__main__":
    unittest.main()
